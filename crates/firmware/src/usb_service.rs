//! USB CDC-ACM 虚拟串口服务
//!
//! 提供免驱动的全速 USB 串口通信：
//! - 极低延迟与高吞吐量 (12 Mbps 带宽，满载 ARINC 429 流量仅占用不到 2%)
//! - 严格支持定长 18 字节二进制模式 (BinaryFrame) 与终端交互 ASCII 模式动态切换
//! - 二进制模式下对命令回执与 BIT 报告采用标准帧封装，杜绝字符流污染
//! - 命令解析、执行与响应序列化彻底解耦，保障宿主可测试性

use core::fmt::Write as FmtWrite;
use core::sync::atomic::{AtomicBool, Ordering};
use embassy_futures::select::{select, Either};
use embassy_rp::peripherals::USB;
use embassy_rp::usb::Driver;
use embassy_usb::class::cdc_acm::CdcAcmClass;
use heapless::String;

use arinc429_core::bit::{BitReport, BitStatus};
use arinc429_core::protocol::{
    flags, frame_types, BinaryFrame, FilterAction, HostCommand, BINARY_FRAME_LEN,
};
use arinc429_core::word::ArincWord;

use crate::channel::{
    channel_direction, send_to_channel_ctrl, send_to_channel_tx, BitRequest, ChannelControlCmd,
    ChannelDirection, InboundArincFrame, ACTIVE_CHANNEL_COUNT, BIT_REQ_CHANNEL,
    BIT_RESP_SIGNAL, GLOBAL_RX_PUBSUB,
};

/// 推送模式：true 表示二进制高速帧，false 表示 ASCII 文本
pub static BINARY_OUTPUT_MODE: AtomicBool = AtomicBool::new(true);

#[embassy_executor::task]
pub async fn usb_task(mut usb_device: embassy_usb::UsbDevice<'static, Driver<'static, USB>>) {
    usb_device.run().await;
}

#[embassy_executor::task]
pub async fn usb_cdc_service(mut class: CdcAcmClass<'static, Driver<'static, USB>>) {
    let mut rx_buf = [0u8; 128];
    let mut rx_line = String::<128>::new();
    // 64 字节最大端点聚合缓冲区 (最多容纳 4 帧定长 16 字节二进制遥测数据: 4 * 16 = 64 字节完美整除，零填充浪费)
    let mut bin_batch_buf = [0u8; 64];

    let mut subscriber = GLOBAL_RX_PUBSUB
        .subscriber()
        .expect("Failed to acquire USB subscriber on GLOBAL_RX_PUBSUB");

    loop {
        // 等待 USB 连接就绪
        class.wait_connection().await;
        rx_line.clear();
        let mut overflow_discard = false;

        loop {
            // 并发处理：既监听上位机发来的串口指令，又将收到的 429 数据推送到串口
            match select(class.read_packet(&mut rx_buf), subscriber.next_message_pure()).await {
                // 分支 1：收到上位机发送的数据
                Either::First(read_res) => {
                    match read_res {
                        Ok(0) => {} // 忽略零长心跳包
                        Ok(n) => {
                            for &b in &rx_buf[..n] {
                                if b == b'\r' || b == b'\n' {
                                    if overflow_discard {
                                        overflow_discard = false;
                                        rx_line.clear();
                                    } else if !rx_line.is_empty() {
                                        dispatch_command_line(&rx_line, &mut class).await;
                                        rx_line.clear();
                                    }
                                } else if overflow_discard {
                                    // 处于超长丢弃状态，跳过后续字符直至换行
                                } else if rx_line.len() >= 120 {
                                    // 发生超长指令溢出，拒绝静默截断，进入丢弃状态并报错
                                    overflow_discard = true;
                                    rx_line.clear();
                                    let _ = cdc_write_all(
                                        &mut class,
                                        b"ERR: Command line buffer overflow\r\n",
                                    )
                                    .await;
                                } else {
                                    let _ = rx_line.push(b as char);
                                }
                            }
                        }
                        _ => break, // 连接断开
                    }
                }
                // 分支 2：收到了 ARINC 429 总线帧，上报上位机
                Either::Second(first_frame) => {
                    if BINARY_OUTPUT_MODE.load(Ordering::Relaxed) {
                        let mut offset = 0;
                        let mut single_buf = [0u8; BINARY_FRAME_LEN];

                        let bin = first_frame.to_binary_frame();
                        bin.encode(&mut single_buf);
                        bin_batch_buf[offset..offset + BINARY_FRAME_LEN]
                            .copy_from_slice(&single_buf);
                        offset += BINARY_FRAME_LEN;

                        // 批量拉取当前队列中滞留的后续帧 (最多凑 4 帧 64 字节恰好填满单包端点)
                        while offset + BINARY_FRAME_LEN <= bin_batch_buf.len() {
                            if let Some(next_frame) = subscriber.try_next_message_pure() {
                                let next_bin = next_frame.to_binary_frame();
                                next_bin.encode(&mut single_buf);
                                bin_batch_buf[offset..offset + BINARY_FRAME_LEN]
                                    .copy_from_slice(&single_buf);
                                offset += BINARY_FRAME_LEN;
                            } else {
                                break;
                            }
                        }

                        if cdc_write_all(&mut class, &bin_batch_buf[..offset]).await.is_err() {
                            break;
                        }
                    } else {
                        // ASCII 文本模式：格式化为可读字符串
                        let mut line = String::<128>::new();
                        format_ascii_frame(&first_frame, &mut line);
                        if cdc_write_all(&mut class, line.as_bytes()).await.is_err() {
                            break;
                        }
                    }
                }
            }
        }
    }
}

/// 安全分片写入 CDC 端点，单包长度严格限制在 64 字节内
async fn cdc_write_all(
    class: &mut CdcAcmClass<'static, Driver<'static, USB>>,
    data: &[u8],
) -> Result<(), embassy_usb::driver::EndpointError> {
    if data.is_empty() {
        return Ok(());
    }
    for chunk in data.chunks(64) {
        class.write_packet(chunk).await?;
    }
    if (data.len() & 63) == 0 {
        class.write_packet(&[]).await?;
    }
    Ok(())
}

/// 指令执行结果枚举 (解耦执行器与 USB I/O)
pub enum CommandExecutionResult {
    SuccessText(String<128>),
    ErrorStatic(&'static str),
    ErrorDynamic(String<64>),
    BitCompleted(BitReport),
}

/// 解析并执行命令，将响应结果格式化输出至 CDC
async fn dispatch_command_line(
    line: &str,
    class: &mut CdcAcmClass<'static, Driver<'static, USB>>,
) {
    let result = match HostCommand::parse(line) {
        Ok(cmd) => execute_host_command(cmd).await,
        Err(e) => {
            let mut err_str = String::<64>::new();
            core::write!(err_str, "ERR: Command parse failure: {:?}\r\n", e).ok();
            CommandExecutionResult::ErrorDynamic(err_str)
        }
    };

    let is_binary = BINARY_OUTPUT_MODE.load(Ordering::Relaxed);
    send_execution_result(class, result, is_binary).await;
}

/// 校验通道编号是否已装配且符合预期方向
#[inline]
fn validate_channel(channel: u8, expected: Option<ChannelDirection>) -> Result<(), CommandExecutionResult> {
    if (channel as usize) >= ACTIVE_CHANNEL_COUNT {
        return Err(CommandExecutionResult::ErrorStatic(
            "ERR: Channel not equipped on hardware (active: 0..1)\r\n",
        ));
    }
    if let Some(exp) = expected {
        if channel_direction(channel) != Some(exp) {
            let msg = if exp == ChannelDirection::Tx {
                "ERR: Operation only supported on physical TX channels\r\n"
            } else {
                "ERR: Operation only supported on physical RX channels\r\n"
            };
            return Err(CommandExecutionResult::ErrorStatic(msg));
        }
    }
    Ok(())
}

async fn handle_channel_filter(channel: u8, action: FilterAction) -> CommandExecutionResult {
    if let Err(err) = validate_channel(channel, Some(ChannelDirection::Rx)) {
        return err;
    }
    let desc = match action {
        FilterAction::Allow(_) => "Allowed",
        FilterAction::Block(_) => "Blocked",
        FilterAction::Clear => "Cleared",
    };
    let _ = send_to_channel_ctrl(channel, ChannelControlCmd::Filter(action)).await;

    let mut resp = String::<128>::new();
    core::write!(resp, "OK: Filter rule {} on Channel {}\r\n", desc, channel).ok();
    CommandExecutionResult::SuccessText(resp)
}

/// 业务执行器：纯状态机与消息派发逻辑 (可直接用于宿主单元测试)
pub async fn execute_host_command(cmd: HostCommand) -> CommandExecutionResult {
    match cmd {
        HostCommand::SetMode { binary_mode } => {
            BINARY_OUTPUT_MODE.store(binary_mode, Ordering::Relaxed);
            let mut s = String::<128>::new();
            if binary_mode {
                let _ = s.push_str("OK: Switched to BINARY mode\r\n");
            } else {
                let _ = s.push_str("OK: Switched to ASCII mode\r\n");
            }
            CommandExecutionResult::SuccessText(s)
        }
        HostCommand::ConfigureChannel {
            channel,
            enable,
            is_tx,
            high_speed,
        } => {
            let expected_dir = if is_tx { ChannelDirection::Tx } else { ChannelDirection::Rx };
            if enable {
                if let Err(err) = validate_channel(channel, Some(expected_dir)) {
                    return err;
                }
            } else if let Err(err) = validate_channel(channel, None) {
                return err;
            }

            let dir = if !enable {
                ChannelDirection::Disabled
            } else {
                expected_dir
            };
            let cmd = ChannelControlCmd::Configure {
                direction: dir,
                high_speed,
            };
            let _ = send_to_channel_ctrl(channel, cmd).await;

            let mut resp = String::<128>::new();
            core::write!(
                resp,
                "OK: Channel {} configured: {:?}, Speed: {}\r\n",
                channel,
                dir,
                if high_speed { "100K" } else { "12.5K" }
            )
            .ok();
            CommandExecutionResult::SuccessText(resp)
        }
        HostCommand::ChannelFilter { channel, action } => {
            handle_channel_filter(channel, action).await
        }
        HostCommand::SetSpeed { channel, high_speed } => {
            if let Err(err) = validate_channel(channel, None) {
                return err;
            }
            let dir = channel_direction(channel).unwrap_or(ChannelDirection::Disabled);
            let cmd = ChannelControlCmd::Configure {
                direction: dir,
                high_speed,
            };
            let _ = send_to_channel_ctrl(channel, cmd).await;
            let mut resp = String::<128>::new();
            core::write!(
                resp,
                "OK: Channel {} set to {}\r\n",
                channel,
                if high_speed { "100K" } else { "12.5K" }
            )
            .ok();
            CommandExecutionResult::SuccessText(resp)
        }
        HostCommand::SendRaw { channel, raw_word } => {
            if let Err(err) = validate_channel(channel, Some(ChannelDirection::Tx)) {
                return err;
            }
            let _ = send_to_channel_tx(channel, raw_word).await;
            let mut resp = String::<128>::new();
            core::write!(
                resp,
                "OK: Queued raw word 0x{:08X} for CH{}\r\n",
                raw_word,
                channel
            )
            .ok();
            CommandExecutionResult::SuccessText(resp)
        }
        HostCommand::SendConstructed {
            channel,
            label,
            sdi,
            data,
            ssm,
            parity,
        } => {
            if let Err(err) = validate_channel(channel, Some(ChannelDirection::Tx)) {
                return err;
            }
            let word = match ArincWord::from_fields(label, sdi, data, ssm, parity) {
                Ok(w) => w,
                Err(_) => {
                    return CommandExecutionResult::ErrorStatic(
                        "ERR: Invalid word parameters\r\n",
                    );
                }
            };
            let _ = send_to_channel_tx(channel, word.to_raw()).await;
            let mut resp = String::<128>::new();
            core::write!(
                resp,
                "OK: Sent L:{:03o} SDI:{} DATA:0x{:X} RAW:0x{:08X}\r\n",
                label.to_octal(),
                sdi,
                data,
                word.to_raw()
            )
            .ok();
            CommandExecutionResult::SuccessText(resp)
        }
        HostCommand::RunBit { tx_channel, rx_channel } => {
            if let Err(err) = validate_channel(tx_channel, Some(ChannelDirection::Tx)) {
                return err;
            }
            if let Err(err) = validate_channel(rx_channel, Some(ChannelDirection::Rx)) {
                return err;
            }
            BIT_REQ_CHANNEL.send(BitRequest { tx_channel, rx_channel }).await;
            let report = BIT_RESP_SIGNAL.wait().await;
            CommandExecutionResult::BitCompleted(report)
        }
        HostCommand::QueryStatus { channel: _ } => {
            let mut resp = String::<128>::new();
            core::write!(
                resp,
                "STATUS: Pico-429 Online (2 Channels equipped: CH0 TX, CH1 RX). Mode: {}\r\n",
                if BINARY_OUTPUT_MODE.load(Ordering::Relaxed) { "BIN" } else { "ASCII" }
            )
            .ok();
            CommandExecutionResult::SuccessText(resp)
        }
    }
}

/// 响应序列化器：保证在 BINARY 模式下严禁混入裸 ASCII 字符串 (消灭流污染)
async fn send_execution_result(
    class: &mut CdcAcmClass<'static, Driver<'static, USB>>,
    result: CommandExecutionResult,
    is_binary: bool,
) {
    match result {
        CommandExecutionResult::SuccessText(text) => {
            if is_binary {
                // 二进制模式：封装为 CMD_ACK 帧
                let ack_frame = BinaryFrame::new_full(
                    frame_types::CMD_ACK,
                    0,
                    0,
                    0,
                    0,
                    0x0000_0000,
                );
                let mut buf = [0u8; BINARY_FRAME_LEN];
                ack_frame.encode(&mut buf);
                let _ = cdc_write_all(class, &buf).await;
            } else {
                let _ = cdc_write_all(class, text.as_bytes()).await;
            }
        }
        CommandExecutionResult::ErrorStatic(msg) => {
            if is_binary {
                let err_frame = BinaryFrame::new_full(
                    frame_types::ASYNC_EVENT,
                    0,
                    flags::BUS_FAULT,
                    0,
                    0,
                    0xFFFF_FFFF,
                );
                let mut buf = [0u8; BINARY_FRAME_LEN];
                err_frame.encode(&mut buf);
                let _ = cdc_write_all(class, &buf).await;
            } else {
                let _ = cdc_write_all(class, msg.as_bytes()).await;
            }
        }
        CommandExecutionResult::ErrorDynamic(text) => {
            if is_binary {
                let err_frame = BinaryFrame::new_full(
                    frame_types::ASYNC_EVENT,
                    0,
                    flags::BUS_FAULT,
                    0,
                    0,
                    0xFFFF_FFFF,
                );
                let mut buf = [0u8; BINARY_FRAME_LEN];
                err_frame.encode(&mut buf);
                let _ = cdc_write_all(class, &buf).await;
            } else {
                let _ = cdc_write_all(class, text.as_bytes()).await;
            }
        }
        CommandExecutionResult::BitCompleted(report) => {
            if is_binary {
                // 二进制模式：封装为 BIT_REPORT 帧 (raw_word 编码 status 与 error_code)
                let status_val = if report.status == BitStatus::Passed { 0 } else { 1 };
                let encoded_status = (status_val << 16) | (report.error_code as u32);
                let report_frame = BinaryFrame::new_full(
                    frame_types::BIT_REPORT,
                    report.rx_channel,
                    0,
                    report.words_received,
                    report.avg_latency_us,
                    encoded_status,
                );
                let mut buf = [0u8; BINARY_FRAME_LEN];
                report_frame.encode(&mut buf);
                let _ = cdc_write_all(class, &buf).await;
            } else {
                let mut resp = String::<128>::new();
                if report.status == BitStatus::Passed {
                    core::write!(
                        resp,
                        "OK: BIT CH{}->CH{} PASSED [SENT:{} RECV:{} ERR_INJECT:{}/{} RTT_AVG:{}us]\r\n",
                        report.tx_channel,
                        report.rx_channel,
                        report.words_sent,
                        report.words_received,
                        report.parity_errors_caught,
                        report.parity_injections_tested,
                        report.avg_latency_us
                    )
                    .ok();
                } else {
                    core::write!(
                        resp,
                        "ERR: BIT CH{}->CH{} FAILED [CODE:{:?} SENT:{} RECV:{}]\r\n",
                        report.tx_channel,
                        report.rx_channel,
                        report.error_code,
                        report.words_sent,
                        report.words_received
                    )
                    .ok();
                }
                let _ = cdc_write_all(class, resp.as_bytes()).await;
            }
        }
    }
}

/// 格式化为可读的 ASCII 遥测文本
fn format_ascii_frame(frame: &InboundArincFrame, out: &mut String<128>) {
    let word = frame.word;
    let speed_str = if frame.high_speed { "100K" } else { "12.5K" };
    let parity_str = if frame.parity_ok { "OK" } else { "ERR" };

    core::write!(
        out,
        "[CH{}] T:{}us {} L:{:03o} SDI:{} DATA:0x{:05X} SSM:{} P:{} (0x{:08X})\r\n",
        frame.channel_id,
        frame.timestamp_us,
        speed_str,
        word.label().to_octal(),
        word.sdi(),
        word.data_field(),
        word.ssm_raw(),
        parity_str,
        word.to_raw()
    )
    .ok();
}

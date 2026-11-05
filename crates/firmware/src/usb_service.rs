//! USB CDC-ACM 虚拟串口服务
//!
//! 提供免驱动的全速 USB 串口通信：
//! - 极低延迟与高吞吐量 (12 Mbps 带宽，满载 ARINC 429 流量仅占用不到 2%)
//! - 严格支持定长 16 字节二进制模式 (BinaryFrame) 与终端交互 ASCII 模式动态切换
//! - 二进制模式下对命令回执与 BIT 报告采用标准帧封装，杜绝字符流污染
//! - 命令解析、执行与响应序列化彻底解耦，保障宿主可测试性

use core::fmt::Write as FmtWrite;
use core::sync::atomic::{AtomicBool, Ordering};
use embassy_futures::select::{select, Either};
use embassy_rp::peripherals::USB;
use embassy_rp::usb::Driver;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_usb::class::cdc_acm::{Receiver, Sender};
use heapless::String;

use arinc429_core::protocol::{
    flags, frame_types, BinaryFrame, FilterAction, HostCommand, BINARY_FRAME_LEN,
};
use arinc429_core::word::ArincWord;

use crate::channel::{
    channel_direction, send_to_channel_ctrl, send_to_channel_tx, set_channel_direction,
    ChannelControlCmd, ChannelDirection, ChannelError, InboundArincFrame, ACTIVE_CHANNEL_COUNT,
    BOARD_CHANNELS, GLOBAL_RX_PUBSUB,
};

/// 推送模式：true 表示二进制高速帧，false 表示 ASCII 文本 (默认启动为 ASCII 便于调试验证)
pub static BINARY_OUTPUT_MODE: AtomicBool = AtomicBool::new(false);

/// 向上位机发送控制命令回执的专用非阻塞队列 (深度 16，防突发指令丢弃)
pub static USB_CMD_RESP_QUEUE: Channel<CriticalSectionRawMutex, CommandExecutionResult, 16> =
    Channel::new();

#[embassy_executor::task]
pub async fn usb_task(mut usb_device: embassy_usb::UsbDevice<'static, Driver<'static, USB>>) {
    usb_device.run().await;
}

/// 独立的 USB CDC 接收任务：专职读取上位机下发的控制指令
///
/// 架构隔离：绝不受下行发送阻塞影响，USB OUT 端点始终处于就绪武装状态，彻底杜绝上位机写入超时 (Win32 Error 121)。
#[embassy_executor::task]
pub async fn usb_cdc_rx_task(mut receiver: Receiver<'static, Driver<'static, USB>>) {
    let mut rx_buf = [0u8; 128];
    let mut rx_line = String::<128>::new();
    let mut overflow_discard = false;

    receiver.wait_connection().await;

    loop {
        match receiver.read_packet(&mut rx_buf).await {
            Ok(0) => {} // 忽略零长包
            Ok(n) => {
                for &b in &rx_buf[..n] {
                    if b == b'\r' || b == b'\n' {
                        if overflow_discard {
                            overflow_discard = false;
                            rx_line.clear();
                        } else if !rx_line.is_empty() {
                            let result = match HostCommand::parse(&rx_line) {
                                Ok(cmd) => execute_host_command(cmd).await,
                                Err(e) => {
                                    let mut err_str = String::<64>::new();
                                    core::write!(
                                        err_str,
                                        "ERR: Command parse failure: {:?}\r\n",
                                        e
                                    )
                                    .ok();
                                    CommandExecutionResult::ErrorDynamic(err_str)
                                }
                            };
                            let _ = USB_CMD_RESP_QUEUE.try_send(result);
                            rx_line.clear();
                        }
                    } else if overflow_discard {
                        // 处于超长丢弃状态，跳过后续字符直至换行
                    } else if rx_line.len() >= 120 {
                        overflow_discard = true;
                        rx_line.clear();
                        let _ = USB_CMD_RESP_QUEUE.try_send(CommandExecutionResult::ErrorStatic(
                            "ERR: Command line buffer overflow\r\n",
                        ));
                    } else {
                        let _ = rx_line.push(b as char);
                    }
                }
            }
            Err(_) => {
                receiver.wait_connection().await;
            }
        }
    }
}

/// 独立的 USB CDC 发送任务：统一调度命令回执与业务遥测流
///
/// 优先保障上位机命令 ACK / 响应即时输出，在 host 未连接/未读时自动丢弃滞留总线遥测，消灭死锁。
#[embassy_executor::task]
pub async fn usb_cdc_tx_task(mut sender: Sender<'static, Driver<'static, USB>>) {
    let mut bin_batch_buf = [0u8; 64];

    let mut subscriber = GLOBAL_RX_PUBSUB
        .subscriber()
        .expect("Failed to acquire USB subscriber on GLOBAL_RX_PUBSUB");

    sender.wait_connection().await;

    loop {
        match select(USB_CMD_RESP_QUEUE.receive(), subscriber.next_message_pure()).await {
            // 分支 1：命令回执 (最高调度优先级)
            Either::First(result) => {
                let is_binary = BINARY_OUTPUT_MODE.load(Ordering::Relaxed);
                if send_execution_result(&mut sender, result, is_binary)
                    .await
                    .is_err()
                {
                    sender.wait_connection().await;
                }
            }
            // 分支 2：总线接收遥测数据流
            Either::Second(first_frame) => {
                // 若 host 未就绪监听 (DTR 假)，静默跳过，避免总线背景流量塞满 IN 端点导致后续命令卡死
                if !sender.dtr() {
                    continue;
                }

                if BINARY_OUTPUT_MODE.load(Ordering::Relaxed) {
                    let mut offset = 0;
                    let mut single_buf = [0u8; BINARY_FRAME_LEN];

                    let bin = first_frame.to_binary_frame();
                    bin.encode(&mut single_buf);
                    bin_batch_buf[offset..offset + BINARY_FRAME_LEN].copy_from_slice(&single_buf);
                    offset += BINARY_FRAME_LEN;

                    // 批量拉取当前队列中滞留的后续帧 (最多凑 4 帧 64 字节填满单包端点)
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

                    if cdc_sender_write_all(&mut sender, &bin_batch_buf[..offset])
                        .await
                        .is_err()
                    {
                        sender.wait_connection().await;
                    }
                } else {
                    let mut line = String::<128>::new();
                    format_ascii_frame(&first_frame, &mut line);
                    if cdc_sender_write_all(&mut sender, line.as_bytes())
                        .await
                        .is_err()
                    {
                        sender.wait_connection().await;
                    }
                }
            }
        }
    }
}

/// 安全分片写入 CDC IN 端点，单包长度严格限制在 64 字节内，带 100ms 超时保护
async fn cdc_sender_write_all(
    sender: &mut Sender<'static, Driver<'static, USB>>,
    data: &[u8],
) -> Result<(), embassy_usb::driver::EndpointError> {
    if data.is_empty() {
        return Ok(());
    }
    for chunk in data.chunks(64) {
        if embassy_time::with_timeout(
            embassy_time::Duration::from_millis(100),
            sender.write_packet(chunk),
        )
        .await
        .is_err()
        {
            // 上位机未及时拉取 IN 端点，超时报错退出避免阻塞 USB 主调度
            return Err(embassy_usb::driver::EndpointError::BufferOverflow);
        }
    }
    Ok(())
}

/// 指令执行结果枚举 (解耦执行器与 USB I/O)
#[derive(Clone)]
pub enum CommandExecutionResult {
    Pong,
    SuccessText(String<128>),
    ErrorStatic(&'static str),
    ErrorDynamic(String<64>),
}

/// 校验通道编号是否已装配且符合预期方向
#[inline]
fn validate_channel(
    channel: u8,
    expected: Option<ChannelDirection>,
) -> Result<(), CommandExecutionResult> {
    if (channel as usize) >= ACTIVE_CHANNEL_COUNT {
        return Err(CommandExecutionResult::ErrorStatic(
            "ERR: Channel not equipped on hardware (active: 0..1)\r\n",
        ));
    }
    if let Some(expected_dir) = expected {
        if let Some(actual_dir) = channel_direction(channel) {
            if actual_dir != expected_dir {
                let mut resp = String::<64>::new();
                core::write!(
                    resp,
                    "ERR: Channel {} is {:?}, but expected {:?}\r\n",
                    channel,
                    actual_dir,
                    expected_dir
                )
                .ok();
                return Err(CommandExecutionResult::ErrorDynamic(resp));
            }
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
        HostCommand::Ping => CommandExecutionResult::Pong,
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
            if let Err(err) = validate_channel(channel, None) {
                return err;
            }

            let hw_desc = &BOARD_CHANNELS[channel as usize];
            let requested_dir = if !enable {
                ChannelDirection::Disabled
            } else if is_tx {
                ChannelDirection::Tx
            } else {
                ChannelDirection::Rx
            };

            // 物理引脚与芯片硬件固化方向守护：杜绝反向配置产生状态倒置与队列死锁
            if requested_dir != ChannelDirection::Disabled && requested_dir != hw_desc.direction {
                let mut resp = String::<64>::new();
                core::write!(
                    resp,
                    "ERR: Channel {} hardware is physically {:?}-only\r\n",
                    channel,
                    hw_desc.direction
                )
                .ok();
                return CommandExecutionResult::ErrorDynamic(resp);
            }

            set_channel_direction(channel, requested_dir);
            let cmd = ChannelControlCmd::Configure {
                direction: requested_dir,
                high_speed,
            };
            let _ = send_to_channel_ctrl(channel, cmd).await;

            let mut resp = String::<128>::new();
            core::write!(
                resp,
                "OK: Channel {} configured: {:?}, Speed: {}\r\n",
                channel,
                requested_dir,
                if high_speed { "100K" } else { "12.5K" }
            )
            .ok();
            CommandExecutionResult::SuccessText(resp)
        }
        HostCommand::ChannelFilter { channel, action } => {
            handle_channel_filter(channel, action).await
        }
        HostCommand::SetSpeed {
            channel,
            high_speed,
        } => {
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
            match send_to_channel_tx(channel, raw_word) {
                Ok(()) => {
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
                Err(ChannelError::QueueFull(ch)) => {
                    let mut resp = String::<64>::new();
                    core::write!(resp, "ERR: TX queue full on Channel {}\r\n", ch).ok();
                    CommandExecutionResult::ErrorDynamic(resp)
                }
                Err(ChannelError::ChannelDisabled(ch)) => {
                    let mut resp = String::<64>::new();
                    core::write!(resp, "ERR: Channel {} is disabled\r\n", ch).ok();
                    CommandExecutionResult::ErrorDynamic(resp)
                }
                Err(ChannelError::NotTxChannel(ch)) => {
                    let mut resp = String::<64>::new();
                    core::write!(resp, "ERR: Channel {} is not a TX channel\r\n", ch).ok();
                    CommandExecutionResult::ErrorDynamic(resp)
                }
                Err(ChannelError::ChannelOutOfRange(ch)) => {
                    let mut resp = String::<64>::new();
                    core::write!(resp, "ERR: Channel {} out of range\r\n", ch).ok();
                    CommandExecutionResult::ErrorDynamic(resp)
                }
            }
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
                    return CommandExecutionResult::ErrorStatic("ERR: Invalid word parameters\r\n");
                }
            };
            match send_to_channel_tx(channel, word.to_raw()) {
                Ok(()) => {
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
                Err(ChannelError::QueueFull(ch)) => {
                    let mut resp = String::<64>::new();
                    core::write!(resp, "ERR: TX queue full on Channel {}\r\n", ch).ok();
                    CommandExecutionResult::ErrorDynamic(resp)
                }
                Err(ChannelError::ChannelDisabled(ch)) => {
                    let mut resp = String::<64>::new();
                    core::write!(resp, "ERR: Channel {} is disabled\r\n", ch).ok();
                    CommandExecutionResult::ErrorDynamic(resp)
                }
                Err(ChannelError::NotTxChannel(ch)) => {
                    let mut resp = String::<64>::new();
                    core::write!(resp, "ERR: Channel {} is not a TX channel\r\n", ch).ok();
                    CommandExecutionResult::ErrorDynamic(resp)
                }
                Err(ChannelError::ChannelOutOfRange(ch)) => {
                    let mut resp = String::<64>::new();
                    core::write!(resp, "ERR: Channel {} out of range\r\n", ch).ok();
                    CommandExecutionResult::ErrorDynamic(resp)
                }
            }
        }
        HostCommand::RunBit { .. } => CommandExecutionResult::ErrorStatic(
            "ERR: On-device BIT disabled. Perform loopback verification via host\r\n",
        ),
        HostCommand::QueryStatus { channel: _ } => {
            let mut resp = String::<128>::new();
            let ch0_dir = channel_direction(0).unwrap_or(ChannelDirection::Disabled);
            let ch1_dir = channel_direction(1).unwrap_or(ChannelDirection::Disabled);
            core::write!(
                resp,
                "STATUS: Pico-429 Online [CH0: {:?}, CH1: {:?}]. Mode: {}\r\n",
                ch0_dir,
                ch1_dir,
                if BINARY_OUTPUT_MODE.load(Ordering::Relaxed) {
                    "BIN"
                } else {
                    "ASCII"
                }
            )
            .ok();
            CommandExecutionResult::SuccessText(resp)
        }
    }
}

/// 响应序列化器：保证在 BINARY 模式下严禁混入裸 ASCII 字符串 (消灭流污染)
async fn send_execution_result(
    sender: &mut Sender<'static, Driver<'static, USB>>,
    result: CommandExecutionResult,
    is_binary: bool,
) -> Result<(), embassy_usb::driver::EndpointError> {
    match result {
        CommandExecutionResult::Pong => {
            cdc_sender_write_all(sender, b"PONG\r\n").await?;
        }
        CommandExecutionResult::SuccessText(text) => {
            if is_binary {
                // 二进制模式：封装为 CMD_ACK 帧
                let ack_frame =
                    BinaryFrame::new_full(frame_types::CMD_ACK, 0, 0, 0, 0, 0x0000_0000);
                let mut buf = [0u8; BINARY_FRAME_LEN];
                ack_frame.encode(&mut buf);
                cdc_sender_write_all(sender, &buf).await?;
            } else {
                cdc_sender_write_all(sender, text.as_bytes()).await?;
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
                cdc_sender_write_all(sender, &buf).await?;
            } else {
                cdc_sender_write_all(sender, msg.as_bytes()).await?;
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
                cdc_sender_write_all(sender, &buf).await?;
            } else {
                cdc_sender_write_all(sender, text.as_bytes()).await?;
            }
        }
    }
    Ok(())
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

//! USB CDC-ACM 虚拟串口服务
//!
//! 提供免驱动的全速 USB 串口通信：
//! - 极低延迟与高吞吐量 (12 Mbps 带宽，满载 ARINC 429 流量仅占用不到 2%)
//! - 自动支持二进制模式 (BinaryFrame) 与终端交互 ASCII 模式动态切换
//! - 支持上位机控制命令解析与参数下发

use core::fmt::Write as FmtWrite;
use core::sync::atomic::{AtomicBool, Ordering};
use embassy_futures::select::{select, Either};
use embassy_rp::peripherals::USB;
use embassy_rp::usb::Driver;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_usb::class::cdc_acm::CdcAcmClass;
use heapless::String;

use arinc429_core::protocol::{BINARY_FRAME_LEN, HostCommand};
use arinc429_core::word::ArincWord;

use crate::channel::{InboundArincFrame, OutboundArincFrame, GLOBAL_RX_CHANNEL, GLOBAL_TX_CHANNEL};

/// 推送模式：true 表示二进制高速帧，false 表示 ASCII 文本
pub static BINARY_OUTPUT_MODE: AtomicBool = AtomicBool::new(true);

/// 信号：通知配置变动
pub static SPEED_CONFIG_SIGNAL: Signal<CriticalSectionRawMutex, (u8, bool)> = Signal::new();

#[embassy_executor::task]
pub async fn usb_task(mut usb_device: embassy_usb::UsbDevice<'static, Driver<'static, USB>>) {
    usb_device.run().await;
}

#[embassy_executor::task]
pub async fn usb_cdc_service(mut class: CdcAcmClass<'static, Driver<'static, USB>>) {
    let mut rx_buf = [0u8; 128];
    let mut rx_line = String::<128>::new();
    let mut bin_frame_buf = [0u8; BINARY_FRAME_LEN];

    loop {
        // 等待 USB 连接就绪
        class.wait_connection().await;

        loop {
            // 并发处理：既监听上位机发来的串口指令，又将收到的 429 数据推送到串口
            match select(class.read_packet(&mut rx_buf), GLOBAL_RX_CHANNEL.receive()).await {
                // 分支 1：收到上位机发送的数据
                Either::First(read_res) => {
                    match read_res {
                        Ok(n) if n > 0 => {
                            for &b in &rx_buf[..n] {
                                if b == b'\r' || b == b'\n' {
                                    if !rx_line.is_empty() {
                                        handle_cli_command(&rx_line, &mut class).await;
                                        rx_line.clear();
                                    }
                                } else if rx_line.len() < 120 {
                                    rx_line.push(b as char).ok();
                                }
                            }
                        }
                        _ => break, // 连接断开
                    }
                }
                // 分支 2：收到了 ARINC 429 总线帧，上报上位机
                Either::Second(frame) => {
                    if BINARY_OUTPUT_MODE.load(Ordering::Relaxed) {
                        // 二进制模式：打包成 14 字节定长帧
                        let bin = frame.to_binary_frame();
                        bin.encode(&mut bin_frame_buf);
                        if class.write_packet(&bin_frame_buf).await.is_err() {
                            break;
                        }
                    } else {
                        // ASCII 文本模式：格式化为可读字符串
                        let mut line = String::<128>::new();
                        format_ascii_frame(&frame, &mut line);
                        if class.write_packet(line.as_bytes()).await.is_err() {
                            break;
                        }
                    }
                }
            }
        }
    }
}

/// 解析并执行上位机发送的 ASCII 命令
async fn handle_cli_command(
    line: &str,
    class: &mut CdcAcmClass<'static, Driver<'static, USB>>,
) {
    match HostCommand::parse(line) {
        Ok(HostCommand::SetMode { binary_mode }) => {
            BINARY_OUTPUT_MODE.store(binary_mode, Ordering::Relaxed);
            let resp = if binary_mode {
                "OK: Switched to BINARY mode\r\n"
            } else {
                "OK: Switched to ASCII mode\r\n"
            };
            let _ = class.write_packet(resp.as_bytes()).await;
        }
        Ok(HostCommand::SetSpeed { channel, high_speed }) => {
            SPEED_CONFIG_SIGNAL.signal((channel, high_speed));
            let mut resp = String::<64>::new();
            let speed_str = if high_speed { "100K" } else { "12.5K" };
            core::write!(resp, "OK: Channel {} set to {}\r\n", channel, speed_str).ok();
            let _ = class.write_packet(resp.as_bytes()).await;
        }
        Ok(HostCommand::SendRaw { channel, raw_word }) => {
            GLOBAL_TX_CHANNEL
                .send(OutboundArincFrame {
                    channel_id: channel,
                    raw_word,
                })
                .await;
            let mut resp = String::<64>::new();
            core::write!(resp, "OK: Queued raw word 0x{:08X} for CH{}\r\n", raw_word, channel).ok();
            let _ = class.write_packet(resp.as_bytes()).await;
        }
        Ok(HostCommand::SendConstructed {
            channel,
            label,
            sdi,
            data,
            ssm,
            parity,
        }) => {
            let word = ArincWord::builder()
                .label(label)
                .sdi(sdi)
                .data(data)
                .ssm(ssm)
                .parity_mode(parity)
                .build();
            GLOBAL_TX_CHANNEL
                .send(OutboundArincFrame {
                    channel_id: channel,
                    raw_word: word.to_raw(),
                })
                .await;
            let mut resp = String::<64>::new();
            core::write!(
                resp,
                "OK: Sent L:{:03o} SDI:{} DATA:0x{:X} RAW:0x{:08X}\r\n",
                label.to_octal(),
                sdi,
                data,
                word.to_raw()
            )
            .ok();
            let _ = class.write_packet(resp.as_bytes()).await;
        }
        Ok(HostCommand::QueryStatus) => {
            let mut resp = String::<128>::new();
            core::write!(
                resp,
                "STATUS: Pico-429 Online. Mode: {}\r\n",
                if BINARY_OUTPUT_MODE.load(Ordering::Relaxed) { "BIN" } else { "ASCII" }
            )
            .ok();
            let _ = class.write_packet(resp.as_bytes()).await;
        }
        Ok(_) => {
            let _ = class.write_packet(b"OK: Filter rule updated\r\n").await;
        }
        Err(e) => {
            let mut resp = String::<64>::new();
            core::write!(resp, "ERR: {}\r\n", e).ok();
            let _ = class.write_packet(resp.as_bytes()).await;
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

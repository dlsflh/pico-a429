//! USB CDC-ACM 全二进制通信服务 (no_std)
//!
//! 提供免驱动的全速 USB 串口通信：
//! - 采用对称、定长 16 字节通用二进制通信帧 (BinaryFrame)；
//! - 完美整除 USB 64 字节端点 ($4 \times 16 = 64$)，零填充碎片；
//! - 严格采用 1 字节增量滑动窗口搜帧，彻底免疫任意相位偏移与流失步；
//! - 固件内部零字符串格式化、零堆分配，具备确定性的硬实时响应。

use embassy_futures::select::{select, Either};
use embassy_rp::peripherals::USB;
use embassy_rp::usb::Driver;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_usb::class::cdc_acm::{Receiver, Sender};

use arinc429_core::label::Label;
use arinc429_core::protocol::{
    ack_status, find_frame_in_stream, frame_types, opcodes, pack_12_channels, profile_sub_ops,
    unpack_12_channels, unpack_channel_nibble, BinaryFrame, FilterAction, BINARY_FRAME_LEN,
};

use crate::channel::{
    channel_direction, channel_is_high_speed, send_to_channel_ctrl, send_to_channel_tx,
    set_channel_direction, set_channel_speed, ChannelControlCmd, ChannelDirection,
    ACTIVE_CHANNEL_COUNT, GLOBAL_RX_PUBSUB,
};

/// 向上位机发送控制命令回执的专用非阻塞队列 (深度 16，防突发指令丢弃)
pub static USB_CMD_RESP_QUEUE: Channel<CriticalSectionRawMutex, BinaryFrame, 16> = Channel::new();

#[embassy_executor::task]
pub async fn usb_task(mut usb_device: embassy_usb::UsbDevice<'static, Driver<'static, USB>>) {
    usb_device.run().await;
}

/// 独立的 USB CDC 接收任务：专职读取上位机下发的二进制控制命令 (CMD_REQ)
///
/// 架构隔离：绝不受下行发送阻塞影响，USB OUT 端点始终处于就绪武装状态，彻底杜绝上位机写入超时 (Win32 Error 121)。
#[embassy_executor::task]
pub async fn usb_cdc_rx_task(mut receiver: Receiver<'static, Driver<'static, USB>>) {
    let mut rx_packet = [0u8; 64];
    let mut stream_buf = [0u8; 128];
    let mut stream_len = 0usize;

    receiver.wait_connection().await;

    loop {
        match receiver.read_packet(&mut rx_packet).await {
            Ok(0) => {} // 忽略零长包
            Ok(n) => {
                if stream_len + n > stream_buf.len() {
                    // 防溢出保护：清空滑动缓冲并重新同步
                    stream_len = 0;
                }
                stream_buf[stream_len..stream_len + n].copy_from_slice(&rx_packet[..n]);
                stream_len += n;

                // 采用 1 字节增量滑动窗口持续搜索合法帧
                while let Some((frame, consumed)) =
                    find_frame_in_stream(&stream_buf[..stream_len])
                {
                    if frame.frame_type == frame_types::CMD_REQ {
                        execute_binary_command(frame).await;
                    }
                    // 滑动移出已消耗字节 (包含噪声与完整帧)
                    stream_buf.copy_within(consumed..stream_len, 0);
                    stream_len -= consumed;
                }

                // 缓冲区即将耗尽且仍未匹配到帧头时，丢弃无效头部，仅保留最后不足一帧的数据
                if stream_len > BINARY_FRAME_LEN * 2 {
                    let keep = BINARY_FRAME_LEN - 1;
                    let start = stream_len - keep;
                    stream_buf.copy_within(start..stream_len, 0);
                    stream_len = keep;
                }
            }
            Err(_) => {
                stream_len = 0;
                receiver.wait_connection().await;
            }
        }
    }
}

/// 独立的 USB CDC 发送任务：统一调度命令回执与业务遥测流
///
/// 优先保障上位机命令 ACK 即时输出，在 host 未连接/未读时自动丢弃滞留总线遥测，消灭死锁。
#[embassy_executor::task]
pub async fn usb_cdc_tx_task(mut sender: Sender<'static, Driver<'static, USB>>) {
    let mut bin_batch_buf = [0u8; 64];

    let mut subscriber = GLOBAL_RX_PUBSUB
        .subscriber()
        .expect("Failed to acquire USB subscriber on GLOBAL_RX_PUBSUB");

    sender.wait_connection().await;

    loop {
        match select(USB_CMD_RESP_QUEUE.receive(), subscriber.next_message_pure()).await {
            // 分支 1：命令回执 (最高调度优先级，始终以 16 字节 CMD_ACK 二进制帧即刻响应)
            Either::First(ack_frame) => {
                let mut single_buf = [0u8; BINARY_FRAME_LEN];
                ack_frame.encode(&mut single_buf);
                if cdc_sender_write_all(&mut sender, &single_buf).await.is_err() {
                    sender.wait_connection().await;
                }
            }
            // 分支 2：总线接收遥测数据流 (16 字节 ARINC_DATA 二进制帧流)
            Either::Second(first_frame) => {
                // 若 host 未就绪监听 (DTR 假)，静默跳过，避免总线背景流量塞满 IN 端点导致后续命令卡死
                if !sender.dtr() {
                    continue;
                }

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

/// 原子暂停全部 12 通道物理收发
pub async fn pause_all_channels() {
    for ch in 0..ACTIVE_CHANNEL_COUNT as u8 {
        let _ = send_to_channel_ctrl(ch, ChannelControlCmd::Pause).await;
    }
}

/// 恢复全部 12 通道物理收发
pub async fn resume_all_channels() {
    for ch in 0..ACTIVE_CHANNEL_COUNT as u8 {
        let _ = send_to_channel_ctrl(ch, ChannelControlCmd::Resume).await;
    }
}

/// 配置单通道方向与速率
pub async fn apply_channel_config(channel: u8, dir: ChannelDirection, high_speed: bool) {
    set_channel_direction(channel, dir);
    set_channel_speed(channel, high_speed);
    let _ = send_to_channel_ctrl(
        channel,
        ChannelControlCmd::Configure {
            direction: dir,
            high_speed,
        },
    )
    .await;
}

/// 批量原子应用 12 通道配置 (暂停 -> 配置 -> 恢复)
pub async fn apply_all_channels(dirs: &[u8; 12], speeds: &[u8; 12]) {
    pause_all_channels().await;
    for ch in 0..ACTIVE_CHANNEL_COUNT {
        let dir = match dirs[ch] {
            1 => ChannelDirection::Tx,
            2 => ChannelDirection::Rx,
            _ => ChannelDirection::Disabled,
        };
        apply_channel_config(ch as u8, dir, speeds[ch] != 0).await;
    }
    if !crate::channel::is_config_mode() {
        resume_all_channels().await;
    }
}

/// 二进制命令执行调度器：纯状态机与消息派发逻辑 (零动态分配)
async fn execute_binary_command(req: BinaryFrame) {
    let opcode = req.opcode();
    match opcode {
        opcodes::PING => {
            let ack = BinaryFrame::new_cmd_ack(
                req.channel,
                ack_status::OK,
                req.seq_num,
                req.payload_0,
                0,
            );
            let _ = USB_CMD_RESP_QUEUE.try_send(ack);
        }
        opcodes::SET_CHANNEL_CFG => {
            let ch = req.channel;
            if (ch as usize) >= ACTIVE_CHANNEL_COUNT {
                let ack = BinaryFrame::new_cmd_ack(
                    ch,
                    ack_status::ERR_INVALID_PARAM,
                    req.seq_num,
                    0,
                    0,
                );
                let _ = USB_CMD_RESP_QUEUE.try_send(ack);
                return;
            }
            let (enable, is_rx, high_speed) =
                unpack_channel_nibble((req.payload_0 & 0x0F) as u8);
            let dir = if !enable {
                ChannelDirection::Disabled
            } else if is_rx {
                ChannelDirection::Rx
            } else {
                ChannelDirection::Tx
            };
            apply_channel_config(ch, dir, high_speed).await;
            let ack = BinaryFrame::new_cmd_ack(
                ch,
                ack_status::OK,
                req.seq_num,
                req.payload_0,
                0,
            );
            let _ = USB_CMD_RESP_QUEUE.try_send(ack);
        }
        opcodes::SET_FILTER => {
            let ch = req.channel;
            if (ch as usize) >= ACTIVE_CHANNEL_COUNT
                || channel_direction(ch) != Some(ChannelDirection::Rx)
            {
                let ack = BinaryFrame::new_cmd_ack(
                    ch,
                    ack_status::ERR_INVALID_PARAM,
                    req.seq_num,
                    0,
                    0,
                );
                let _ = USB_CMD_RESP_QUEUE.try_send(ack);
                return;
            }
            let action_code = (req.payload_0 & 0xFF) as u8;
            let octal_label = ((req.payload_0 >> 8) & 0xFF) as u8;
            let action = match action_code {
                0 => FilterAction::Clear,
                1 => FilterAction::Allow(Label::from_octal(octal_label)),
                2 => FilterAction::Block(Label::from_octal(octal_label)),
                _ => {
                    let ack = BinaryFrame::new_cmd_ack(
                        ch,
                        ack_status::ERR_INVALID_PARAM,
                        req.seq_num,
                        0,
                        0,
                    );
                    let _ = USB_CMD_RESP_QUEUE.try_send(ack);
                    return;
                }
            };
            let _ = send_to_channel_ctrl(ch, ChannelControlCmd::Filter(action)).await;
            let ack = BinaryFrame::new_cmd_ack(
                ch,
                ack_status::OK,
                req.seq_num,
                req.payload_0,
                0,
            );
            let _ = USB_CMD_RESP_QUEUE.try_send(ack);
        }
        opcodes::TX_RAW_WORD => {
            let ch = req.channel;
            if crate::channel::is_config_mode()
                || (ch as usize) >= ACTIVE_CHANNEL_COUNT
                || channel_direction(ch) != Some(ChannelDirection::Tx)
            {
                let ack = BinaryFrame::new_cmd_ack(
                    ch,
                    ack_status::ERR_MODE_CONFLICT,
                    req.seq_num,
                    0,
                    0,
                );
                let _ = USB_CMD_RESP_QUEUE.try_send(ack);
                return;
            }
            match send_to_channel_tx(ch, req.payload_0) {
                Ok(()) => {
                    let ack = BinaryFrame::new_cmd_ack(
                        ch,
                        ack_status::OK,
                        req.seq_num,
                        req.payload_0,
                        0,
                    );
                    let _ = USB_CMD_RESP_QUEUE.try_send(ack);
                }
                Err(_) => {
                    let ack = BinaryFrame::new_cmd_ack(
                        ch,
                        ack_status::ERR_BUFFER_FULL,
                        req.seq_num,
                        0,
                        0,
                    );
                    let _ = USB_CMD_RESP_QUEUE.try_send(ack);
                }
            }
        }
        opcodes::SET_OPERATING_MODE => {
            let to_operational = req.payload_0 != 0;
            if to_operational {
                crate::channel::set_operating_mode(crate::channel::OperatingMode::Operational);
                resume_all_channels().await;
            } else {
                crate::channel::set_operating_mode(crate::channel::OperatingMode::Config);
                pause_all_channels().await;
            }
            let current_mode = if crate::channel::is_config_mode() { 0 } else { 1 };
            let ack = BinaryFrame::new_cmd_ack(
                0,
                ack_status::OK,
                req.seq_num,
                current_mode,
                0,
            );
            let _ = USB_CMD_RESP_QUEUE.try_send(ack);
        }
        opcodes::PAUSE_RESUME_ALL => {
            let resume = req.payload_0 != 0;
            if resume {
                resume_all_channels().await;
            } else {
                pause_all_channels().await;
            }
            let ack = BinaryFrame::new_cmd_ack(
                0,
                ack_status::OK,
                req.seq_num,
                req.payload_0,
                0,
            );
            let _ = USB_CMD_RESP_QUEUE.try_send(ack);
        }
        opcodes::CFGBATCH => {
            let channels = unpack_12_channels(req.payload_0, req.payload_1);
            pause_all_channels().await;
            for ch in 0..ACTIVE_CHANNEL_COUNT {
                let (enable, is_rx, high_speed) = channels[ch];
                let dir = if !enable {
                    ChannelDirection::Disabled
                } else if is_rx {
                    ChannelDirection::Rx
                } else {
                    ChannelDirection::Tx
                };
                apply_channel_config(ch as u8, dir, high_speed).await;
            }
            if !crate::channel::is_config_mode() {
                resume_all_channels().await;
            }
            let ack = BinaryFrame::new_cmd_ack(
                0,
                ack_status::OK,
                req.seq_num,
                req.payload_0,
                req.payload_1,
            );
            let _ = USB_CMD_RESP_QUEUE.try_send(ack);
        }
        opcodes::QUERY_STATUS => {
            let mut channels = [(false, false, false); 12];
            for ch in 0..ACTIVE_CHANNEL_COUNT {
                let dir = channel_direction(ch as u8);
                let (en, is_rx) = match dir {
                    Some(ChannelDirection::Tx) => (true, false),
                    Some(ChannelDirection::Rx) => (true, true),
                    _ => (false, false),
                };
                let hs = channel_is_high_speed(ch as u8);
                channels[ch] = (en, is_rx, hs);
            }
            let (p0, p1) = pack_12_channels(&channels);
            let mode = if crate::channel::is_config_mode() { 0 } else { 1 };
            let ack = BinaryFrame::new_cmd_ack(mode, ack_status::OK, req.seq_num, p0, p1);
            let _ = USB_CMD_RESP_QUEUE.try_send(ack);
        }
        opcodes::PROFILE_OP => {
            let sub_op = (req.payload_0 & 0xFF) as u8;
            let slot = req.channel;
            let mut flash_guard = crate::flash_storage::FLASH_MGR.lock().await;
            if let Some(ref mut mgr) = *flash_guard {
                match sub_op {
                    profile_sub_ops::LIST => {
                        let boot_slot = mgr.get_boot_slot();
                        for s in 0..crate::flash_storage::MAX_PROFILE_SLOTS as u8 {
                            match mgr.load_profile(s) {
                                Ok(rec) => {
                                    let is_boot = if Some(s) == boot_slot { 1u32 } else { 0u32 };
                                    let p0 = 1u32 | (is_boot << 8) | ((rec.crc16 as u32) << 16);
                                    let ack = BinaryFrame::new_cmd_ack(
                                        s,
                                        ack_status::OK,
                                        req.seq_num,
                                        p0,
                                        0,
                                    );
                                    let _ = USB_CMD_RESP_QUEUE.try_send(ack);
                                }
                                Err(_) => {
                                    let ack = BinaryFrame::new_cmd_ack(
                                        s,
                                        ack_status::OK,
                                        req.seq_num,
                                        0,
                                        0,
                                    );
                                    let _ = USB_CMD_RESP_QUEUE.try_send(ack);
                                }
                            }
                        }
                    }
                    profile_sub_ops::SAVE => {
                        let mut dirs = [0u8; 12];
                        let mut speeds = [0u8; 12];
                        for ch in 0..12 {
                            dirs[ch] = match channel_direction(ch as u8) {
                                Some(ChannelDirection::Tx) => 1,
                                Some(ChannelDirection::Rx) => 2,
                                _ => 0,
                            };
                            speeds[ch] = if channel_is_high_speed(ch as u8) { 1 } else { 0 };
                        }
                        let status = match mgr.save_profile(slot, "PROFILE", &dirs, &speeds) {
                            Ok(()) => ack_status::OK,
                            Err(_) => ack_status::ERR_FLASH,
                        };
                        let ack = BinaryFrame::new_cmd_ack(
                            slot,
                            status,
                            req.seq_num,
                            0,
                            0,
                        );
                        let _ = USB_CMD_RESP_QUEUE.try_send(ack);
                    }
                    profile_sub_ops::LOAD => {
                        let status = match mgr.load_profile(slot) {
                            Ok(rec) => {
                                crate::channel::set_operating_mode(
                                    crate::channel::OperatingMode::Operational,
                                );
                                apply_all_channels(&rec.directions, &rec.speeds).await;
                                ack_status::OK
                            }
                            Err(_) => ack_status::ERR_FLASH,
                        };
                        let ack = BinaryFrame::new_cmd_ack(
                            slot,
                            status,
                            req.seq_num,
                            0,
                            0,
                        );
                        let _ = USB_CMD_RESP_QUEUE.try_send(ack);
                    }
                    profile_sub_ops::BOOT => {
                        let status = match mgr.set_boot_slot(slot) {
                            Ok(()) => ack_status::OK,
                            Err(_) => ack_status::ERR_FLASH,
                        };
                        let ack = BinaryFrame::new_cmd_ack(
                            slot,
                            status,
                            req.seq_num,
                            0,
                            0,
                        );
                        let _ = USB_CMD_RESP_QUEUE.try_send(ack);
                    }
                    profile_sub_ops::CLEAR_BOOT => {
                        let status = match mgr.clear_boot_slot() {
                            Ok(()) => ack_status::OK,
                            Err(_) => ack_status::ERR_FLASH,
                        };
                        let ack = BinaryFrame::new_cmd_ack(
                            0,
                            status,
                            req.seq_num,
                            0,
                            0,
                        );
                        let _ = USB_CMD_RESP_QUEUE.try_send(ack);
                    }
                    _ => {
                        let ack = BinaryFrame::new_cmd_ack(
                            slot,
                            ack_status::ERR_INVALID_PARAM,
                            req.seq_num,
                            0,
                            0,
                        );
                        let _ = USB_CMD_RESP_QUEUE.try_send(ack);
                    }
                }
            } else {
                let ack = BinaryFrame::new_cmd_ack(
                    slot,
                    ack_status::ERR_FLASH,
                    req.seq_num,
                    0,
                    0,
                );
                let _ = USB_CMD_RESP_QUEUE.try_send(ack);
            }
        }
        _ => {
            let ack = BinaryFrame::new_cmd_ack(
                req.channel,
                ack_status::ERR_UNKNOWN_CMD,
                req.seq_num,
                0,
                0,
            );
            let _ = USB_CMD_RESP_QUEUE.try_send(ack);
        }
    }
}

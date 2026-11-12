//! USB CDC-ACM 全二进制通信服务 (no_std)
//!
//! 提供免驱动的全速 USB 串口通信：
//! - 采用对称、定长 16 字节通用二进制通信帧 (BinaryFrame)；
//! - 完美整除 USB 64 字节端点 ($4 \times 16 = 64$)，零填充碎片；
//! - 严格采用 1 字节增量滑动窗口搜帧，彻底免疫任意相位偏移与流失步；
//! - 固件内部零字符串格式化、零堆分配，具备确定性的硬实时响应。

use core::cell::RefCell;
use embassy_futures::select::{Either, select};
use embassy_rp::peripherals::USB;
use embassy_rp::usb::Driver;
use embassy_sync::blocking_mutex::Mutex as BlockingMutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_usb::class::cdc_acm::{Receiver, Sender};

use arinc429_core::label::Label;
use arinc429_core::protocol::{
    BINARY_FRAME_LEN, BinaryFrame, FilterAction, ack_status, find_frame_in_stream, frame_types,
    opcodes, pack_12_channels, profile_sub_ops, unpack_12_channels, unpack_channel_nibble,
};

use crate::channel::{
    ACTIVE_CHANNEL_COUNT, ChannelControlCmd, ChannelDirection, GLOBAL_RX_PUBSUB,
    apply_all_channels_batch, apply_channel_config, apply_profile, channel_direction,
    pause_all_channels, resume_all_channels, send_to_channel_ctrl, send_to_channel_tx,
};

/// 向上位机发送控制命令回执的专用非阻塞队列 (深度 32，防突发指令丢弃)
pub static USB_CMD_RESP_QUEUE: Channel<CriticalSectionRawMutex, BinaryFrame, 32> = Channel::new();

/// 暂存上位机下发的 32 字节配置方案名称 (线程安全临界区保护，消除 unsafe 裸指针)
static STAGED_PROFILE_NAME: BlockingMutex<CriticalSectionRawMutex, RefCell<[u8; 32]>> =
    BlockingMutex::new(RefCell::new([0u8; 32]));

#[embassy_executor::task]
pub async fn usb_task(mut usb_device: embassy_usb::UsbDevice<'static, Driver<'static, USB>>) {
    usb_device.run().await;
}

/// 独立的 USB CDC 接收任务：专职读取上位机下发的二进制控制命令与流式数据
///
/// 架构隔离：绝不受下行发送阻塞影响，USB OUT 端点始终处于就绪武装状态，彻底杜绝上位机写入超时 (Win32 Error 121)。
#[embassy_executor::task]
pub async fn usb_cdc_rx_task(mut receiver: Receiver<'static, Driver<'static, USB>>) {
    #[repr(align(4))]
    struct AlignedRxPacket([u8; 64]);
    #[repr(align(4))]
    struct AlignedStreamBuf([u8; 1024]);

    let mut rx_packet = AlignedRxPacket([0u8; 64]);
    let mut stream_buf = AlignedStreamBuf([0u8; 1024]);
    let mut stream_len = 0usize;

    receiver.wait_connection().await;

    loop {
        match receiver.read_packet(&mut rx_packet.0).await {
            Ok(0) => {} // 忽略零长包
            Ok(n) => {
                if stream_len + n > stream_buf.0.len() {
                    // 防溢出保护：清空滑动缓冲并重新同步
                    stream_len = 0;
                }
                stream_buf.0[stream_len..stream_len + n].copy_from_slice(&rx_packet.0[..n]);
                stream_len += n;

                // 游标推进式解析：单批数据内部无需频繁 copy_within，彻底消除多重内存拷贝
                let mut cursor = 0usize;
                while let Some((frame, consumed)) =
                    find_frame_in_stream(&stream_buf.0[cursor..stream_len])
                {
                    if frame.frame_type == frame_types::CMD_REQ {
                        execute_binary_command(frame).await;
                    } else if frame.frame_type == frame_types::STREAM_TX_DATA {
                        let _ = send_to_channel_tx(frame.channel, frame.stream_word());
                    }
                    cursor += consumed;
                }

                // 批量移出已消费的全部数据 (整批仅需至多一次 copy_within 或直接清零)
                if cursor == stream_len {
                    stream_len = 0;
                } else if cursor > 0 {
                    stream_buf.0.copy_within(cursor..stream_len, 0);
                    stream_len -= cursor;
                }

                // 缓冲区即将耗尽且仍未匹配到帧头时，丢弃无效头部，仅保留最后不足一帧的数据
                if stream_len > BINARY_FRAME_LEN * 2 {
                    let keep = BINARY_FRAME_LEN - 1;
                    let start = stream_len - keep;
                    stream_buf.0.copy_within(start..stream_len, 0);
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

/// 广播初始流量控制信用给上位机 (枚举完成自启动)
async fn broadcast_initial_credits(sender: &mut Sender<'static, Driver<'static, USB>>) {
    let mut buf = [0u8; 64];
    let mut offset = 0;
    for ch in 0..ACTIVE_CHANNEL_COUNT as u8 {
        let hs = crate::channel::channel_is_high_speed(ch);
        let ceiling = if hs { 128 } else { 32 };
        let frame = BinaryFrame::new_stream_credit(ch, 0, ceiling, 0);
        frame.encode_to_slice(&mut buf[offset..offset + BINARY_FRAME_LEN]);
        offset += BINARY_FRAME_LEN;
        if offset == 64 {
            let _ = cdc_sender_write_all(sender, &buf).await;
            offset = 0;
        }
    }
    if offset > 0 {
        let _ = cdc_sender_write_all(sender, &buf[..offset]).await;
    }
}

/// 扫描并聚合全部发生信用变动的通道，直接原位打包为 64B 批次发送
async fn flush_dirty_credits(sender: &mut Sender<'static, Driver<'static, USB>>) {
    let mask = crate::channel::CREDIT_DIRTY_MASK.swap(0, core::sync::atomic::Ordering::AcqRel);
    if mask == 0 {
        return;
    }
    let mut buf = [0u8; 64];
    let mut offset = 0;
    for ch in 0..ACTIVE_CHANNEL_COUNT as u8 {
        if (mask & (1 << ch)) != 0 {
            let consumed = crate::channel::CUMULATIVE_CONSUMED_COUNTER[ch as usize]
                .load(core::sync::atomic::Ordering::Relaxed);
            let dropped = crate::channel::DROPPED_TX_WORDS[ch as usize]
                .load(core::sync::atomic::Ordering::Relaxed);
            let total_handled = consumed.wrapping_add(dropped);
            crate::channel::LAST_REPORTED_HANDLED[ch as usize]
                .store(total_handled, core::sync::atomic::Ordering::Relaxed);

            let hs = crate::channel::channel_is_high_speed(ch);
            let ceiling = if hs { 128 } else { 32 };
            let frame = BinaryFrame::new_stream_credit(ch, 0, ceiling, total_handled);
            frame.encode_to_slice(&mut buf[offset..offset + BINARY_FRAME_LEN]);
            offset += BINARY_FRAME_LEN;
            if offset == 64 {
                let _ = cdc_sender_write_all(sender, &buf).await;
                offset = 0;
            }
        }
    }
    if offset > 0 {
        let _ = cdc_sender_write_all(sender, &buf[..offset]).await;
    }
}

/// 独立的 USB CDC 发送任务：统一调度命令回执、流量信用与业务遥测流
///
/// 优先保障上位机命令 ACK 即时输出，在 host 未连接/未读时自动丢弃滞留总线遥测，消灭死锁。
#[embassy_executor::task]
pub async fn usb_cdc_tx_task(mut sender: Sender<'static, Driver<'static, USB>>) {
    #[repr(align(4))]
    struct AlignedBatchBuffer([u8; 64]);

    let mut bin_batch_buf = AlignedBatchBuffer([0u8; 64]);

    let mut subscriber = GLOBAL_RX_PUBSUB
        .subscriber()
        .expect("Failed to acquire USB subscriber on GLOBAL_RX_PUBSUB");

    sender.wait_connection().await;
    broadcast_initial_credits(&mut sender).await;

    loop {
        // 先排空累积的流控信用帧
        flush_dirty_credits(&mut sender).await;

        match select(USB_CMD_RESP_QUEUE.receive(), subscriber.next_message_pure()).await {
            // 分支 1：命令回执 (最高调度优先级，始终以 16 字节 CMD_ACK 二进制帧即刻响应)
            Either::First(ack_frame) => {
                let mut single_buf = [0u8; BINARY_FRAME_LEN];
                ack_frame.encode_to_slice(&mut single_buf);
                if cdc_sender_write_all(&mut sender, &single_buf)
                    .await
                    .is_err()
                {
                    sender.wait_connection().await;
                    broadcast_initial_credits(&mut sender).await;
                }
            }
            // 分支 2：总线接收遥测数据流 (16 字节 ARINC_DATA 二进制帧流)
            Either::Second(first_frame) => {
                // 若 host 未就绪监听 (DTR 假)，静默跳过，避免总线背景流量塞满 IN 端点导致后续命令卡死
                if !sender.dtr() {
                    continue;
                }

                let mut offset = 0;
                let bin = first_frame.to_binary_frame();
                // 零拷贝优化：直接原位直写至 4 字节自然对齐的端点批量缓冲区
                bin.encode_to_slice(&mut bin_batch_buf.0[offset..offset + BINARY_FRAME_LEN]);
                offset += BINARY_FRAME_LEN;

                // 批量拉取当前队列中滞留的后续帧 (最多凑 4 帧 64 字节填满单包端点，原位直写消除多重拷贝)
                while offset + BINARY_FRAME_LEN <= bin_batch_buf.0.len() {
                    if let Some(next_frame) = subscriber.try_next_message_pure() {
                        let next_bin = next_frame.to_binary_frame();
                        next_bin.encode_to_slice(
                            &mut bin_batch_buf.0[offset..offset + BINARY_FRAME_LEN],
                        );
                        offset += BINARY_FRAME_LEN;
                    } else {
                        break;
                    }
                }

                if cdc_sender_write_all(&mut sender, &bin_batch_buf.0[..offset])
                    .await
                    .is_err()
                {
                    sender.wait_connection().await;
                    broadcast_initial_credits(&mut sender).await;
                }
            }
        }
    }
}

/// 安全分片写入 CDC IN 端点，单包长度严格限制在 64 字节内，带 300ms 超时保护
async fn cdc_sender_write_all(
    sender: &mut Sender<'static, Driver<'static, USB>>,
    data: &[u8],
) -> Result<(), embassy_usb::driver::EndpointError> {
    if data.is_empty() {
        return Ok(());
    }
    for chunk in data.chunks(64) {
        if embassy_time::with_timeout(
            embassy_time::Duration::from_millis(300),
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
                let ack =
                    BinaryFrame::new_cmd_ack(ch, ack_status::ERR_INVALID_PARAM, req.seq_num, 0, 0);
                let _ = USB_CMD_RESP_QUEUE.try_send(ack);
                return;
            }
            let (enable, is_rx, high_speed) = unpack_channel_nibble((req.payload_0 & 0x0F) as u8);
            let dir = if !enable {
                ChannelDirection::Disabled
            } else if is_rx {
                ChannelDirection::Rx
            } else {
                ChannelDirection::Tx
            };
            apply_channel_config(ch, dir, high_speed).await;
            let ack = BinaryFrame::new_cmd_ack(ch, ack_status::OK, req.seq_num, req.payload_0, 0);
            let _ = USB_CMD_RESP_QUEUE.try_send(ack);
        }
        opcodes::SET_FILTER => {
            let ch = req.channel;
            if (ch as usize) >= ACTIVE_CHANNEL_COUNT
                || channel_direction(ch) != Some(ChannelDirection::Rx)
            {
                let ack =
                    BinaryFrame::new_cmd_ack(ch, ack_status::ERR_INVALID_PARAM, req.seq_num, 0, 0);
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
            let updated_filter = crate::channel::update_channel_filter(ch, action);
            let _ =
                send_to_channel_ctrl(ch, ChannelControlCmd::SetFullFilter(updated_filter)).await;
            let ack = BinaryFrame::new_cmd_ack(ch, ack_status::OK, req.seq_num, req.payload_0, 0);
            let _ = USB_CMD_RESP_QUEUE.try_send(ack);
        }
        opcodes::TX_RAW_WORD => {
            let ch = req.channel;
            if crate::channel::is_config_mode()
                || (ch as usize) >= ACTIVE_CHANNEL_COUNT
                || channel_direction(ch) != Some(ChannelDirection::Tx)
            {
                let ack =
                    BinaryFrame::new_cmd_ack(ch, ack_status::ERR_MODE_CONFLICT, req.seq_num, 0, 0);
                let _ = USB_CMD_RESP_QUEUE.try_send(ack);
                return;
            }
            match send_to_channel_tx(ch, req.payload_0) {
                Ok(()) => {
                    let ack =
                        BinaryFrame::new_cmd_ack(ch, ack_status::OK, req.seq_num, req.payload_0, 0);
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
            let current_mode = if crate::channel::is_config_mode() {
                0
            } else {
                1
            };
            let ack = BinaryFrame::new_cmd_ack(0, ack_status::OK, req.seq_num, current_mode, 0);
            let _ = USB_CMD_RESP_QUEUE.try_send(ack);
        }
        opcodes::CFGBATCH => {
            let channels = unpack_12_channels(req.payload_0, req.payload_1);
            apply_all_channels_batch(&channels).await;
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
            let (p0, p1) = crate::channel::pack_runtime_channels();
            let mode = if crate::channel::is_config_mode() {
                0
            } else {
                1
            };
            let ack = BinaryFrame::new_cmd_ack(mode, ack_status::OK, req.seq_num, p0, p1);
            let _ = USB_CMD_RESP_QUEUE.try_send(ack);
        }
        opcodes::PROFILE_OP => {
            let sub_op = (req.payload_0 & 0xFF) as u8;
            let slot = req.channel;

            // 严格状态互锁：Flash 扇区擦写将关闭全局中断 40~100ms，必须仅在 CONFIG 静默维护模式下允许执行，
            // 严禁在 OPERATIONAL 运行模式下执行，以物理杜绝 PIO FIFO 溢出与实时总线丢帧
            let is_mutating_flash_op = matches!(
                sub_op,
                profile_sub_ops::SAVE
                    | profile_sub_ops::BOOT
                    | profile_sub_ops::CLEAR_BOOT
                    | profile_sub_ops::CLEAR
                    | profile_sub_ops::CLEAR_ALL
            );
            if is_mutating_flash_op && !crate::channel::is_config_mode() {
                let ack = BinaryFrame::new_cmd_ack(
                    slot,
                    ack_status::ERR_MODE_CONFLICT,
                    req.seq_num,
                    0,
                    0,
                );
                let _ = USB_CMD_RESP_QUEUE.try_send(ack);
                return;
            }

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
                        let dirs = crate::channel::current_directions();
                        let speeds = crate::channel::current_speeds();
                        let (filter_modes, filter_bitmaps) =
                            crate::channel::get_channel_filters_snapshot();
                        let mut name = [0u8; 32];
                        STAGED_PROFILE_NAME.lock(|cell| {
                            name.copy_from_slice(&*cell.borrow());
                        });
                        let status = match mgr.save_profile(
                            slot,
                            &name,
                            &dirs,
                            &speeds,
                            &filter_modes,
                            &filter_bitmaps,
                        ) {
                            Ok(()) => ack_status::OK,
                            Err(_) => ack_status::ERR_FLASH,
                        };
                        let ack = BinaryFrame::new_cmd_ack(slot, status, req.seq_num, 0, 0);
                        let _ = USB_CMD_RESP_QUEUE.try_send(ack);
                    }
                    profile_sub_ops::LOAD => {
                        let status = match mgr.load_profile(slot) {
                            Ok(rec) => {
                                crate::channel::set_operating_mode(
                                    crate::channel::OperatingMode::Operational,
                                );
                                apply_profile(&rec).await;
                                ack_status::OK
                            }
                            Err(_) => ack_status::ERR_FLASH,
                        };
                        let ack = BinaryFrame::new_cmd_ack(slot, status, req.seq_num, 0, 0);
                        let _ = USB_CMD_RESP_QUEUE.try_send(ack);
                    }
                    profile_sub_ops::BOOT => {
                        let status = match mgr.set_boot_slot(slot) {
                            Ok(()) => ack_status::OK,
                            Err(_) => ack_status::ERR_FLASH,
                        };
                        let ack = BinaryFrame::new_cmd_ack(slot, status, req.seq_num, 0, 0);
                        let _ = USB_CMD_RESP_QUEUE.try_send(ack);
                    }
                    profile_sub_ops::CLEAR_BOOT => {
                        let status = match mgr.clear_boot_slot() {
                            Ok(()) => ack_status::OK,
                            Err(_) => ack_status::ERR_FLASH,
                        };
                        let ack = BinaryFrame::new_cmd_ack(0, status, req.seq_num, 0, 0);
                        let _ = USB_CMD_RESP_QUEUE.try_send(ack);
                    }
                    profile_sub_ops::CLEAR => {
                        let status = match mgr.clear_slot(slot) {
                            Ok(()) => ack_status::OK,
                            Err(_) => ack_status::ERR_FLASH,
                        };
                        let ack = BinaryFrame::new_cmd_ack(slot, status, req.seq_num, 0, 0);
                        let _ = USB_CMD_RESP_QUEUE.try_send(ack);
                    }
                    profile_sub_ops::CLEAR_ALL => {
                        let status = match mgr.clear_all() {
                            Ok(()) => ack_status::OK,
                            Err(_) => ack_status::ERR_FLASH,
                        };
                        let ack = BinaryFrame::new_cmd_ack(0, status, req.seq_num, 0, 0);
                        let _ = USB_CMD_RESP_QUEUE.try_send(ack);
                    }
                    profile_sub_ops::GET_CHANNELS => match mgr.load_profile(slot) {
                        Ok(rec) => {
                            let mut channels = [(false, false, false); 12];
                            for (ch, item) in channels.iter_mut().enumerate() {
                                let en = rec.directions[ch] != 0;
                                let rx = rec.directions[ch] == 2;
                                let hs = rec.speeds[ch] == 1;
                                *item = (en, rx, hs);
                            }
                            let (p0, p1) = pack_12_channels(&channels);
                            let ack =
                                BinaryFrame::new_cmd_ack(slot, ack_status::OK, req.seq_num, p0, p1);
                            let _ = USB_CMD_RESP_QUEUE.try_send(ack);
                        }
                        Err(_) => {
                            let ack = BinaryFrame::new_cmd_ack(
                                slot,
                                ack_status::ERR_FLASH,
                                req.seq_num,
                                0,
                                0,
                            );
                            let _ = USB_CMD_RESP_QUEUE.try_send(ack);
                        }
                    },
                    profile_sub_ops::SET_NAME_CHUNK => {
                        let chunk_idx = ((req.payload_0 >> 8) & 0x07) as usize;
                        let offset = chunk_idx * 4;
                        let bytes = req.payload_1.to_le_bytes();
                        STAGED_PROFILE_NAME.lock(|cell| {
                            cell.borrow_mut()[offset..offset + 4].copy_from_slice(&bytes);
                        });
                        let ack = BinaryFrame::new_cmd_ack(
                            slot,
                            ack_status::OK,
                            req.seq_num,
                            req.payload_0,
                            0,
                        );
                        let _ = USB_CMD_RESP_QUEUE.try_send(ack);
                    }
                    profile_sub_ops::GET_NAME_CHUNK => {
                        let chunk_idx = ((req.payload_0 >> 8) & 0x07) as usize;
                        match mgr.load_profile(slot) {
                            Ok(rec) => {
                                let offset = chunk_idx * 4;
                                let chunk_val = u32::from_le_bytes([
                                    rec.name[offset],
                                    rec.name[offset + 1],
                                    rec.name[offset + 2],
                                    rec.name[offset + 3],
                                ]);
                                let ack = BinaryFrame::new_cmd_ack(
                                    slot,
                                    ack_status::OK,
                                    req.seq_num,
                                    req.payload_0,
                                    chunk_val,
                                );
                                let _ = USB_CMD_RESP_QUEUE.try_send(ack);
                            }
                            Err(_) => {
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
                let ack = BinaryFrame::new_cmd_ack(slot, ack_status::ERR_FLASH, req.seq_num, 0, 0);
                let _ = USB_CMD_RESP_QUEUE.try_send(ack);
            }
        }
        opcodes::ABORT_TX => {
            let ch = req.channel;
            if (ch as usize) < ACTIVE_CHANNEL_COUNT {
                let _ = send_to_channel_ctrl(ch, ChannelControlCmd::PurgeTx).await;
                let ack = BinaryFrame::new_cmd_ack(ch, ack_status::OK, req.seq_num, 0, 0);
                let _ = USB_CMD_RESP_QUEUE.try_send(ack);
            } else {
                let ack =
                    BinaryFrame::new_cmd_ack(ch, ack_status::ERR_INVALID_PARAM, req.seq_num, 0, 0);
                let _ = USB_CMD_RESP_QUEUE.try_send(ack);
            }
        }
        opcodes::REBOOT_TO_BOOTSEL => {
            let ack = BinaryFrame::new_cmd_ack(0, ack_status::OK, req.seq_num, 0, 0);
            let _ = USB_CMD_RESP_QUEUE.try_send(ack);
            embassy_time::Timer::after_millis(100).await;
            embassy_rp::rom_data::reset_to_usb_boot(0, 0);
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

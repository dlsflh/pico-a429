use crate::telemetry::{CurrentValueTable, TelemetryRingBuffers};
use crate::transport::scheduler::{ChannelSpeed, StreamScheduler};
use anyhow::{Context, Result, anyhow};
use arinc429_core::protocol::{
    BINARY_FRAME_LEN, BinaryFrame, ack_status, find_frame_in_stream, frame_types, opcodes,
    pack_12_channels, profile_sub_ops,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{broadcast, mpsc, oneshot};
use tracing::{error, info, warn};

enum PendingResponder {
    Single(oneshot::Sender<BinaryFrame>),
    Multi(mpsc::Sender<BinaryFrame>),
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ProfileSlotInfo {
    pub slot: u8,
    pub valid: bool,
    pub is_boot: bool,
    pub crc16: u16,
    pub name: String,
}

pub struct DeviceManager {
    port_name: String,
    scheduler: Arc<StreamScheduler>,
    ring_buffers: Arc<TelemetryRingBuffers>,
    cvt: Arc<CurrentValueTable>,
    cmd_tx: mpsc::Sender<BinaryFrame>,
    stream_tx: mpsc::Sender<BinaryFrame>,
    pending_cmds: Arc<Mutex<HashMap<u16, PendingResponder>>>,
    connected: Arc<AtomicBool>,
    frame_broadcast: broadcast::Sender<BinaryFrame>,
}

impl DeviceManager {
    pub async fn connect(
        port_name: Option<&str>,
        ring_buffers: Arc<TelemetryRingBuffers>,
        cvt: Arc<CurrentValueTable>,
    ) -> Result<Arc<Self>> {
        let port_path = match port_name {
            Some(p) => p.to_string(),
            None => Self::detect_pico_port()?,
        };

        info!("Opening serial connection to Pico-429 on {}", port_path);
        let mut port = serialport::new(&port_path, 115_200)
            .timeout(Duration::from_millis(10))
            .open()
            .with_context(|| format!("Failed to open serial port {}", port_path))?;

        // 显式拉高 DTR/RTS 信号，通知 Pico CDC-ACM 固件上位机已准备好接收遥测数据流 (匹配 embassy-usb sender.dtr() 保护门)
        if let Err(e) = port.write_data_terminal_ready(true) {
            warn!("Failed to assert DTR: {}", e);
        }
        if let Err(e) = port.write_request_to_send(true) {
            warn!("Failed to assert RTS: {}", e);
        }

        let scheduler = Arc::new(StreamScheduler::new());
        let pending_cmds: Arc<Mutex<HashMap<u16, PendingResponder>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let connected = Arc::new(AtomicBool::new(true));

        let (cmd_tx, mut cmd_rx) = mpsc::channel::<BinaryFrame>(256);
        let (stream_tx, mut stream_rx) = mpsc::channel::<BinaryFrame>(4096);
        let (frame_broadcast, _) = broadcast::channel::<BinaryFrame>(2048);

        let mut reader_port = port
            .try_clone()
            .context("Failed to clone serial port for reading")?;
        let mut writer_port = port;

        let r_buffers = ring_buffers.clone();
        let r_cvt = cvt.clone();
        let r_scheduler = scheduler.clone();
        let r_pending = pending_cmds.clone();
        let r_connected = connected.clone();
        let r_broadcast = frame_broadcast.clone();

        // Background reader thread
        std::thread::Builder::new()
            .name("pico429-serial-reader".to_string())
            .spawn(move || {
                let mut raw_buf = [0u8; 1024];
                let mut stream_buf = Vec::with_capacity(4096);

                while r_connected.load(Ordering::Relaxed) {
                    let avail = match reader_port.bytes_to_read() {
                        Ok(n) => n as usize,
                        Err(e) => {
                            error!("Serial error checking bytes_to_read: {}", e);
                            r_connected.store(false, Ordering::Release);
                            break;
                        }
                    };

                    if avail == 0 {
                        std::thread::sleep(Duration::from_micros(100));
                        continue;
                    }

                    let to_read = avail.min(raw_buf.len());
                    match reader_port.read(&mut raw_buf[..to_read]) {
                        Ok(0) => {
                            std::thread::yield_now();
                        }
                        Ok(n) => {
                            stream_buf.extend_from_slice(&raw_buf[..n]);
                            let mut cursor = 0;
                            while let Some((frame, consumed)) =
                                find_frame_in_stream(&stream_buf[cursor..])
                            {
                                cursor += consumed;
                                match frame.frame_type {
                                    frame_types::ARINC_DATA => {
                                        r_buffers.push(frame);
                                        r_cvt.update_from_frame(&frame);
                                        let _ = r_broadcast.send(frame);
                                    }
                                    frame_types::STREAM_CREDIT => {
                                        r_scheduler.on_credit_frame(
                                            frame.channel,
                                            frame.credit_available_slots(),
                                            frame.credit_cumulative_handled(),
                                        );
                                    }
                                    frame_types::CMD_ACK => {
                                        let seq = frame.seq_num;
                                        let mut pending = r_pending.lock().unwrap();
                                        if let Some(entry) = pending.get_mut(&seq) {
                                            match entry {
                                                PendingResponder::Single(_) => {
                                                    if let Some(PendingResponder::Single(sender)) =
                                                        pending.remove(&seq)
                                                    {
                                                        let _ = sender.send(frame);
                                                    }
                                                }
                                                PendingResponder::Multi(sender) => {
                                                    let _ = sender.try_send(frame);
                                                }
                                            }
                                        }
                                    }
                                    frame_types::ASYNC_EVENT => {
                                        warn!(
                                            "Pico-429 ASYNC_EVENT: channel={}, flags={}",
                                            frame.channel, frame.flags
                                        );
                                    }
                                    _ => {}
                                }
                            }
                            if cursor > 0 {
                                stream_buf.drain(..cursor);
                            }
                            if stream_buf.len() > 8192 {
                                stream_buf.drain(..stream_buf.len() - BINARY_FRAME_LEN);
                            }
                        }
                        Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => {}
                        Err(e) => {
                            error!("Serial read error: {}", e);
                            r_connected.store(false, Ordering::Release);
                            break;
                        }
                    }
                }
            })
            .context("Failed to spawn serial reader thread")?;

        // Background writer thread
        let w_connected = connected.clone();
        tokio::spawn(async move {
            let mut batch_buf = [0u8; 64];
            let mut batch_count = 0usize;

            while w_connected.load(Ordering::Relaxed) {
                // Check Lane 0 (Priority Control) first
                if let Ok(cmd_frame) = cmd_rx.try_recv() {
                    let mut single = [0u8; BINARY_FRAME_LEN];
                    cmd_frame.encode_to_slice(&mut single);
                    if let Err(e) = writer_port.write_all(&single) {
                        error!("Failed to write priority command: {}", e);
                    }
                    continue;
                }

                // Batch Lane 1 (Streaming Data)
                tokio::select! {
                    Some(cmd_frame) = cmd_rx.recv() => {
                        let mut single = [0u8; BINARY_FRAME_LEN];
                        cmd_frame.encode_to_slice(&mut single);
                        let _ = writer_port.write_all(&single);
                    }
                    Some(stream_frame) = stream_rx.recv() => {
                        let offset = batch_count * BINARY_FRAME_LEN;
                        stream_frame.encode_to_slice(&mut batch_buf[offset..offset + BINARY_FRAME_LEN]);
                        batch_count += 1;

                        if batch_count == 4 {
                            let _ = writer_port.write_all(&batch_buf);
                            batch_count = 0;
                        }
                    }
                    _ = tokio::time::sleep(Duration::from_micros(250)) => {
                        if batch_count > 0 {
                            let len = batch_count * BINARY_FRAME_LEN;
                            let _ = writer_port.write_all(&batch_buf[..len]);
                            batch_count = 0;
                        }
                    }
                }
            }
        });

        Ok(Arc::new(Self {
            port_name: port_path,
            scheduler,
            ring_buffers,
            cvt,
            cmd_tx,
            stream_tx,
            pending_cmds,
            connected,
            frame_broadcast,
        }))
    }

    pub fn subscribe_frames(&self) -> broadcast::Receiver<BinaryFrame> {
        self.frame_broadcast.subscribe()
    }

    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }

    pub fn detect_pico_port() -> Result<String> {
        let ports = serialport::available_ports().context("Failed to list serial ports")?;
        for p in ports {
            if let serialport::SerialPortType::UsbPort(usb_info) = p.port_type {
                // Raspberry Pi vendor ID 0x2E8A
                if usb_info.vid == 0x2E8A {
                    return Ok(p.port_name);
                }
            }
        }
        // Fallback to first available port if none with Pi VID
        let available = serialport::available_ports()?;
        if let Some(first) = available.first() {
            Ok(first.port_name.clone())
        } else {
            Err(anyhow!("No serial ports found. Please connect Pico-429."))
        }
    }

    pub async fn send_command(
        &self,
        channel: u8,
        opcode: u8,
        payload_0: u32,
        payload_1: u32,
        timeout: Duration,
    ) -> Result<BinaryFrame> {
        let seq = self.scheduler.next_seq();
        let frame = BinaryFrame::new_cmd_req(channel, opcode, seq, payload_0, payload_1);

        let (resp_tx, resp_rx) = oneshot::channel();
        {
            let mut pending = self.pending_cmds.lock().unwrap();
            pending.insert(seq, PendingResponder::Single(resp_tx));
        }

        self.cmd_tx
            .send(frame)
            .await
            .map_err(|_| anyhow!("Writer task stopped"))?;

        match tokio::time::timeout(timeout, resp_rx).await {
            Ok(Ok(ack)) => {
                if ack.ack_status() == ack_status::OK {
                    Ok(ack)
                } else {
                    let err_desc = match ack.ack_status() {
                        ack_status::ERR_UNKNOWN_CMD => "ERR_UNKNOWN_CMD (未知指令)",
                        ack_status::ERR_INVALID_PARAM => "ERR_INVALID_PARAM (非法参数)",
                        ack_status::ERR_MODE_CONFLICT => {
                            "ERR_MODE_CONFLICT (模式冲突：固件当前模式拒绝此操作，Flash擦写需在CONFIG模式下进行，发送需在RUN模式下进行)"
                        }
                        ack_status::ERR_FLASH => "ERR_FLASH (Flash 存储读写错误)",
                        ack_status::ERR_BUFFER_FULL => "ERR_BUFFER_FULL (硬件缓冲区已满)",
                        _ => "Unknown error",
                    };
                    Err(anyhow!(
                        "Device rejected command (status 0x{:02X}): {}",
                        ack.ack_status(),
                        err_desc
                    ))
                }
            }
            Ok(Err(_)) => Err(anyhow!("Command response dropped by reader task")),
            Err(_) => {
                let mut pending = self.pending_cmds.lock().unwrap();
                pending.remove(&seq);
                Err(anyhow!("Command timed out after {:?}", timeout))
            }
        }
    }

    pub async fn send_command_multi(
        &self,
        channel: u8,
        opcode: u8,
        payload_0: u32,
        payload_1: u32,
        timeout: Duration,
        expected_count: usize,
    ) -> Result<Vec<BinaryFrame>> {
        let seq = self.scheduler.next_seq();
        let frame = BinaryFrame::new_cmd_req(channel, opcode, seq, payload_0, payload_1);

        let (resp_tx, mut resp_rx) = mpsc::channel(expected_count.max(8));
        {
            let mut pending = self.pending_cmds.lock().unwrap();
            pending.insert(seq, PendingResponder::Multi(resp_tx));
        }

        self.cmd_tx
            .send(frame)
            .await
            .map_err(|_| anyhow!("Writer task stopped"))?;

        let mut results = Vec::with_capacity(expected_count);
        let deadline = tokio::time::Instant::now() + timeout;
        while results.len() < expected_count {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            match tokio::time::timeout(remaining, resp_rx.recv()).await {
                Ok(Some(ack)) => {
                    results.push(ack);
                }
                _ => break,
            }
        }

        {
            let mut pending = self.pending_cmds.lock().unwrap();
            pending.remove(&seq);
        }

        Ok(results)
    }

    pub async fn ping(&self) -> Result<u32> {
        let ack = self
            .send_command(0, opcodes::PING, 0x12345678, 0, Duration::from_millis(500))
            .await?;
        Ok(ack.payload_0)
    }

    pub async fn query_status(&self) -> Result<(u8, [(bool, bool, bool); 12])> {
        let ack = self
            .send_command(0, opcodes::QUERY_STATUS, 0, 0, Duration::from_millis(500))
            .await?;
        let mode = ack.channel;
        let channels = arinc429_core::protocol::unpack_12_channels(ack.payload_0, ack.payload_1);
        Ok((mode, channels))
    }

    pub async fn set_operating_mode(&self, mode: u8) -> Result<()> {
        self.send_command(
            0,
            opcodes::SET_OPERATING_MODE,
            mode as u32,
            0,
            Duration::from_millis(500),
        )
        .await?;
        Ok(())
    }

    pub async fn configure_channel(
        &self,
        channel: u8,
        enable: bool,
        is_rx: bool,
        high_speed: bool,
    ) -> Result<()> {
        let nibble = arinc429_core::protocol::pack_channel_nibble(enable, is_rx, high_speed);
        self.send_command(
            channel,
            opcodes::SET_CHANNEL_CFG,
            nibble as u32,
            0,
            Duration::from_millis(500),
        )
        .await?;
        let speed = if high_speed {
            ChannelSpeed::High100k
        } else {
            ChannelSpeed::Low12k5
        };
        self.scheduler.channel(channel).set_speed(speed);
        Ok(())
    }

    pub async fn configure_filter(&self, channel: u8, action: u8, label: u8) -> Result<()> {
        let p0 = (action as u32) | ((label as u32) << 8);
        self.send_command(
            channel,
            opcodes::SET_FILTER,
            p0,
            0,
            Duration::from_millis(500),
        )
        .await?;
        Ok(())
    }

    pub async fn profile_list(&self) -> Result<Vec<ProfileSlotInfo>> {
        let acks = self
            .send_command_multi(
                0,
                opcodes::PROFILE_OP,
                profile_sub_ops::LIST as u32,
                0,
                Duration::from_millis(500),
                4,
            )
            .await?;

        let mut slots = Vec::new();
        for ack in acks {
            let slot = ack.channel;
            let valid = (ack.payload_0 & 0x01) != 0;
            let is_boot = ((ack.payload_0 >> 8) & 0x01) != 0;
            let crc16 = ((ack.payload_0 >> 16) & 0xFFFF) as u16;
            let name = if valid {
                self.profile_get_name(slot).await.unwrap_or_default()
            } else {
                String::new()
            };
            slots.push(ProfileSlotInfo {
                slot,
                valid,
                is_boot,
                crc16,
                name,
            });
        }
        slots.sort_by_key(|s| s.slot);
        Ok(slots)
    }

    pub async fn profile_get_name(&self, slot: u8) -> Result<String> {
        let mut name_bytes = [0u8; 32];
        for chunk_idx in 0..8 {
            let p0 = (profile_sub_ops::GET_NAME_CHUNK as u32) | ((chunk_idx as u32) << 8);
            let ack = self
                .send_command(slot, opcodes::PROFILE_OP, p0, 0, Duration::from_millis(300))
                .await?;
            let bytes = ack.payload_1.to_le_bytes();
            let offset = chunk_idx * 4;
            name_bytes[offset..offset + 4].copy_from_slice(&bytes);
        }
        let end = name_bytes.iter().position(|&b| b == 0).unwrap_or(32);
        let s = String::from_utf8_lossy(&name_bytes[..end])
            .trim()
            .to_string();
        Ok(s)
    }

    pub async fn profile_save(&self, slot: u8, name: &str) -> Result<()> {
        if slot >= 4 {
            return Err(anyhow!("Invalid profile slot {}, must be 0..3", slot));
        }
        let (orig_mode, _) = self.query_status().await?;
        if orig_mode != 0 {
            self.set_operating_mode(0).await?;
        }

        let mut name_bytes = [0u8; 32];
        let name_slice = name.as_bytes();
        let copy_len = name_slice.len().min(32);
        name_bytes[..copy_len].copy_from_slice(&name_slice[..copy_len]);

        for chunk_idx in 0..8 {
            let offset = chunk_idx * 4;
            let val = u32::from_le_bytes([
                name_bytes[offset],
                name_bytes[offset + 1],
                name_bytes[offset + 2],
                name_bytes[offset + 3],
            ]);
            let p0 = (profile_sub_ops::SET_NAME_CHUNK as u32) | ((chunk_idx as u32) << 8);
            self.send_command(
                slot,
                opcodes::PROFILE_OP,
                p0,
                val,
                Duration::from_millis(300),
            )
            .await?;
        }

        let res = self
            .send_command(
                slot,
                opcodes::PROFILE_OP,
                profile_sub_ops::SAVE as u32,
                0,
                Duration::from_millis(1500),
            )
            .await;

        if orig_mode != 0 {
            let _ = self.set_operating_mode(orig_mode).await;
        }

        res.map(|_| ())
    }

    pub async fn profile_load(&self, slot: u8) -> Result<()> {
        if slot >= 4 {
            return Err(anyhow!("Invalid profile slot {}, must be 0..3", slot));
        }
        self.send_command(
            slot,
            opcodes::PROFILE_OP,
            profile_sub_ops::LOAD as u32,
            0,
            Duration::from_millis(500),
        )
        .await?;
        Ok(())
    }

    pub async fn profile_boot(&self, slot: u8) -> Result<()> {
        if slot >= 4 {
            return Err(anyhow!("Invalid profile slot {}, must be 0..3", slot));
        }
        let (orig_mode, _) = self.query_status().await?;
        if orig_mode != 0 {
            self.set_operating_mode(0).await?;
        }
        let res = self
            .send_command(
                slot,
                opcodes::PROFILE_OP,
                profile_sub_ops::BOOT as u32,
                0,
                Duration::from_millis(1500),
            )
            .await;
        if orig_mode != 0 {
            let _ = self.set_operating_mode(orig_mode).await;
        }
        res.map(|_| ())
    }

    pub async fn profile_clear_boot(&self) -> Result<()> {
        let (orig_mode, _) = self.query_status().await?;
        if orig_mode != 0 {
            self.set_operating_mode(0).await?;
        }
        let res = self
            .send_command(
                0,
                opcodes::PROFILE_OP,
                profile_sub_ops::CLEAR_BOOT as u32,
                0,
                Duration::from_millis(1500),
            )
            .await;
        if orig_mode != 0 {
            let _ = self.set_operating_mode(orig_mode).await;
        }
        res.map(|_| ())
    }

    pub async fn profile_clear(&self, slot: u8) -> Result<()> {
        if slot >= 4 {
            return Err(anyhow!("Invalid profile slot {}, must be 0..3", slot));
        }
        let (orig_mode, _) = self.query_status().await?;
        if orig_mode != 0 {
            self.set_operating_mode(0).await?;
        }
        let res = self
            .send_command(
                slot,
                opcodes::PROFILE_OP,
                profile_sub_ops::CLEAR as u32,
                0,
                Duration::from_millis(1500),
            )
            .await;
        if orig_mode != 0 {
            let _ = self.set_operating_mode(orig_mode).await;
        }
        res.map(|_| ())
    }

    pub async fn apply_profile_toml(
        &self,
        profile: &crate::profile::ParsedProfile,
        save_slot: Option<u8>,
    ) -> Result<()> {
        // 1. 批量配置 12 个物理通道的方向与波特率
        let (p0, p1) = pack_12_channels(&profile.channels);
        self.send_command(0, opcodes::CFGBATCH, p0, p1, Duration::from_millis(500))
            .await?;

        for (ch, &(_, _, high_speed)) in profile.channels.iter().enumerate() {
            let speed = if high_speed {
                ChannelSpeed::High100k
            } else {
                ChannelSpeed::Low12k5
            };
            self.scheduler.channel(ch as u8).set_speed(speed);
        }

        // 2. 配置过滤规则
        for (ch, (action, labels)) in &profile.filters {
            match action {
                0 => {
                    self.configure_filter(*ch, 0, 0).await?;
                }
                1 => {
                    self.configure_filter(*ch, 0, 0).await?;
                    for &lbl in labels {
                        self.configure_filter(*ch, 1, lbl).await?;
                    }
                }
                2 => {
                    self.configure_filter(*ch, 0, 0).await?;
                    for &lbl in labels {
                        self.configure_filter(*ch, 2, lbl).await?;
                    }
                }
                _ => {}
            }
        }

        // 3. 若指定存储槽位或 profile 中指定了 boot_slot，持久化至片上 Flash
        let target_slot = save_slot.or(profile.boot_slot);
        if let Some(slot) = target_slot {
            self.profile_save(slot, &profile.name).await?;
            if profile.boot_slot == Some(slot) {
                self.profile_boot(slot).await?;
            }
        }

        // 4. 激活固件为 Operational 运行模式
        self.set_operating_mode(1).await?;
        Ok(())
    }

    pub async fn apply_profile_batch(&self, channels: &[(bool, bool, bool); 12]) -> Result<()> {
        let (p0, p1) = arinc429_core::protocol::pack_12_channels(channels);
        self.send_command(0, opcodes::CFGBATCH, p0, p1, Duration::from_millis(500))
            .await?;
        for (ch, &(_, _, high_speed)) in channels.iter().enumerate() {
            let speed = if high_speed {
                ChannelSpeed::High100k
            } else {
                ChannelSpeed::Low12k5
            };
            self.scheduler.channel(ch as u8).set_speed(speed);
        }
        Ok(())
    }

    pub async fn abort_stream(&self, channel: u8) -> Result<()> {
        self.scheduler.abort_channel(channel);
        let _ = self
            .send_command(channel, opcodes::ABORT_TX, 0, 0, Duration::from_millis(300))
            .await?;
        Ok(())
    }

    pub async fn send_word(&self, channel: u8, word: u32) -> Result<()> {
        self.send_command(
            channel,
            opcodes::TX_RAW_WORD,
            word,
            0,
            Duration::from_millis(500),
        )
        .await?;
        Ok(())
    }

    pub async fn send_stream_word(&self, channel: u8, word: u32) -> Result<()> {
        self.scheduler
            .channel(channel)
            .acquire_credit()
            .await
            .map_err(|e| anyhow!(e))?;
        let seq = self.scheduler.next_seq();
        let frame = BinaryFrame::new_stream_tx(channel, seq, word, 0, true);
        self.stream_tx
            .send(frame)
            .await
            .map_err(|_| anyhow!("Writer task stopped"))?;
        Ok(())
    }

    pub async fn stream_words(&self, channel: u8, words: &[u32], rate_hz: u32) -> Result<usize> {
        let interval = if rate_hz > 0 {
            Duration::from_micros(1_000_000 / rate_hz as u64)
        } else {
            Duration::from_micros(360) // default 100k
        };

        let mut sent = 0;
        let mut next_tick = tokio::time::Instant::now();

        for &word in words {
            self.scheduler
                .channel(channel)
                .acquire_credit()
                .await
                .map_err(|e| anyhow!(e))?;
            let seq = self.scheduler.next_seq();
            let frame = BinaryFrame::new_stream_tx(channel, seq, word, 0, true);
            self.stream_tx
                .send(frame)
                .await
                .map_err(|_| anyhow!("Writer task stopped"))?;
            sent += 1;

            next_tick += interval;
            tokio::time::sleep_until(next_tick).await;
        }

        Ok(sent)
    }

    pub fn scheduler(&self) -> &StreamScheduler {
        &self.scheduler
    }

    pub fn ring_buffers(&self) -> &TelemetryRingBuffers {
        &self.ring_buffers
    }

    pub fn cvt(&self) -> &CurrentValueTable {
        &self.cvt
    }

    pub fn port_name(&self) -> &str {
        &self.port_name
    }
}

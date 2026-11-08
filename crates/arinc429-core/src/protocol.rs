//! 上位机全二进制通信协议模块 (no_std)
//!
//! 采用对称、定长 16 字节高性能二进制帧 (BinaryFrame):
//! - 完美整除 USB 64 字节端点 ($4 \times 16 = 64$)，零填充碎片；
//! - 统一支持全双工通信：下行遥测 (ARINC_DATA)、下行事件 (ASYNC_EVENT)、下行回执 (CMD_ACK)、上行控制 (CMD_REQ)；
//! - 严格采用 1 字节增量滑动窗口搜索，保障抗流失步与即时自同步；
//! - 零动态内存分配，零字符串格式化，保证硬实时确定性。

use crate::label::Label;

pub const BINARY_MAGIC: [u8; 2] = [0x41, 0x29]; // "A)"
pub const BINARY_FRAME_LEN: usize = 16;

/// 帧类型枚举与常量 (高 4 位表示，0~15)
pub mod frame_types {
    /// 正常 ARINC 429 接收/发送字数据载荷
    pub const ARINC_DATA: u8 = 0x01;
    /// 上位机配置或控制命令执行确认 (ACK / NAK)
    pub const CMD_ACK: u8 = 0x02;
    /// 异步告警与硬件总线故障事件
    pub const ASYNC_EVENT: u8 = 0x03;
    /// 上位机下发的二进制控制命令请求
    pub const CMD_REQ: u8 = 0x04;
}

/// 状态标志位 (用于 ARINC_DATA 帧的 flags 字节)
pub mod flags {
    pub const PARITY_ERROR: u8 = 1 << 0;
    pub const SPEED_100K: u8 = 1 << 1; // 1: 100k (高速), 0: 12.5k (低速)
    pub const GAP_ERROR: u8 = 1 << 2;
    pub const IS_TX_ECHO: u8 = 1 << 3; // 该帧为发送回显还是接收捕获
    pub const BUS_FAULT: u8 = 1 << 4; // 双引脚同高或持续高电平故障
}

/// 控制命令操作码 (用于 CMD_REQ 帧的 flags 字节)
pub mod opcodes {
    pub const PING: u8 = 0x01;
    pub const SET_CHANNEL_CFG: u8 = 0x02;
    pub const SET_FILTER: u8 = 0x03;
    pub const TX_RAW_WORD: u8 = 0x04;
    pub const SET_OPERATING_MODE: u8 = 0x05;
    pub const PAUSE_RESUME_ALL: u8 = 0x06;
    pub const CFGBATCH: u8 = 0x07;
    pub const QUERY_STATUS: u8 = 0x08;
    pub const PROFILE_OP: u8 = 0x09;
    pub const REBOOT_TO_BOOTSEL: u8 = 0x0A;
}

/// 命令执行状态码 (用于 CMD_ACK 帧的 flags 字节)
pub mod ack_status {
    pub const OK: u8 = 0x00;
    pub const ERR_UNKNOWN_CMD: u8 = 0x01;
    pub const ERR_INVALID_PARAM: u8 = 0x02;
    pub const ERR_MODE_CONFLICT: u8 = 0x03;
    pub const ERR_FLASH: u8 = 0x04;
    pub const ERR_BUFFER_FULL: u8 = 0x05;
}

/// Profile 操作子动作 (用于 PROFILE_OP 帧 payload_0 的最低字节)
pub mod profile_sub_ops {
    pub const LIST: u8 = 0x00;
    pub const SAVE: u8 = 0x01;
    pub const LOAD: u8 = 0x02;
    pub const BOOT: u8 = 0x03;
    pub const CLEAR_BOOT: u8 = 0x04;
    pub const CLEAR: u8 = 0x05;
    pub const CLEAR_ALL: u8 = 0x06;
    pub const GET_CHANNELS: u8 = 0x07;
    pub const SET_NAME_CHUNK: u8 = 0x08;
    pub const GET_NAME_CHUNK: u8 = 0x09;
}

/// 过滤操作动作
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterAction {
    Allow(Label),
    Block(Label),
    Clear,
}

/// 定长 16 字节的高性能通用二进制帧 (完美整除 64 字节 USB CDC 端点)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct BinaryFrame {
    pub frame_type: u8,
    pub channel: u8,
    pub flags: u8, // Opcode for CMD_REQ; Status for CMD_ACK; Flags for ARINC_DATA
    pub seq_num: u16,
    pub payload_0: u32, // ARINC timestamp_us / Param 0
    pub payload_1: u32, // ARINC raw_word / Param 1
}

/// 二进制反序列化解码错误枚举
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum DecodeError {
    BufferTooShort,
    InvalidMagic,
    CrcMismatch { expected: u16, actual: u16 },
}

impl BinaryFrame {
    /// 创建带包序号与指定帧类型的完整帧
    pub const fn new_full(
        frame_type: u8,
        channel: u8,
        flags: u8,
        seq_num: u16,
        payload_0: u32,
        payload_1: u32,
    ) -> Self {
        Self {
            frame_type: frame_type & 0x0F,
            channel: channel & 0x0F,
            flags,
            seq_num,
            payload_0,
            payload_1,
        }
    }

    /// 创建控制命令请求帧 (CMD_REQ)
    pub const fn new_cmd_req(
        channel: u8,
        opcode: u8,
        seq_num: u16,
        payload_0: u32,
        payload_1: u32,
    ) -> Self {
        Self {
            frame_type: frame_types::CMD_REQ,
            channel: channel & 0x0F,
            flags: opcode,
            seq_num,
            payload_0,
            payload_1,
        }
    }

    /// 创建控制命令确认/应答帧 (CMD_ACK)
    pub const fn new_cmd_ack(
        channel: u8,
        status: u8,
        seq_num: u16,
        payload_0: u32,
        payload_1: u32,
    ) -> Self {
        Self {
            frame_type: frame_types::CMD_ACK,
            channel: channel & 0x0F,
            flags: status,
            seq_num,
            payload_0,
            payload_1,
        }
    }

    /// 获取时间戳别名 (针对 ARINC_DATA)
    pub fn timestamp_us(&self) -> u32 {
        self.payload_0
    }

    /// 获取 ARINC 原始字别名 (针对 ARINC_DATA)
    pub fn raw_word(&self) -> u32 {
        self.payload_1
    }

    /// 获取操作码 (针对 CMD_REQ)
    pub fn opcode(&self) -> u8 {
        self.flags
    }

    /// 获取应答状态码 (针对 CMD_ACK)
    pub fn ack_status(&self) -> u8 {
        self.flags
    }

    /// 序列化为 16 字节定长数据
    pub fn encode(&self, out: &mut [u8; BINARY_FRAME_LEN]) {
        out[0] = BINARY_MAGIC[0];
        out[1] = BINARY_MAGIC[1];

        // 字节 2: 高 4 位为 frame_type，低 4 位为 channel (支持 0~15 路通道)
        out[2] = ((self.frame_type & 0x0F) << 4) | (self.channel & 0x0F);
        out[3] = self.flags;

        let seq_bytes = self.seq_num.to_le_bytes();
        out[4..6].copy_from_slice(&seq_bytes);

        let p0_bytes = self.payload_0.to_le_bytes();
        out[6..10].copy_from_slice(&p0_bytes);

        let p1_bytes = self.payload_1.to_le_bytes();
        out[10..14].copy_from_slice(&p1_bytes);

        // 计算 CRC-16 (覆盖 type_and_channel 到 payload_1 共 12 字节: 索引 2..14)
        let crc = crc16_ccitt(&out[2..14]);
        let crc_bytes = crc.to_le_bytes();
        out[14..16].copy_from_slice(&crc_bytes);
    }

    /// 从切片反序列化
    pub fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
        if buf.len() < BINARY_FRAME_LEN {
            return Err(DecodeError::BufferTooShort);
        }
        if buf[0] != BINARY_MAGIC[0] || buf[1] != BINARY_MAGIC[1] {
            return Err(DecodeError::InvalidMagic);
        }

        let expected_crc = u16::from_le_bytes([buf[14], buf[15]]);
        let actual_crc = crc16_ccitt(&buf[2..14]);
        if expected_crc != actual_crc {
            return Err(DecodeError::CrcMismatch {
                expected: expected_crc,
                actual: actual_crc,
            });
        }

        let type_and_ch = buf[2];
        let frame_type = (type_and_ch >> 4) & 0x0F;
        let channel = type_and_ch & 0x0F;
        let flags = buf[3];
        let seq_num = u16::from_le_bytes([buf[4], buf[5]]);
        let payload_0 = u32::from_le_bytes([buf[6], buf[7], buf[8], buf[9]]);
        let payload_1 = u32::from_le_bytes([buf[10], buf[11], buf[12], buf[13]]);

        Ok(Self {
            frame_type,
            channel,
            flags,
            seq_num,
            payload_0,
            payload_1,
        })
    }
}

/// CRC-16-CCITT (Poly: 0x1021, Init: 0xFFFF)
pub fn crc16_ccitt(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &byte in data {
        crc ^= (byte as u16) << 8;
        for _ in 0..8 {
            if (crc & 0x8000) != 0 {
                crc = (crc << 1) ^ 0x1021;
            } else {
                crc <<= 1;
            }
        }
    }
    crc
}

/// 将单个通道的配置打包为 4 位 nibble
///
/// - bit 0: enable (0=Disable, 1=Enable)
/// - bit 1: direction (0=Tx, 1=Rx)
/// - bit 2: speed (0=12.5k, 1=100k)
/// - bit 3: 保留 0
pub const fn pack_channel_nibble(enable: bool, is_rx: bool, high_speed: bool) -> u8 {
    if !enable {
        0
    } else {
        1 | (if is_rx { 2 } else { 0 }) | (if high_speed { 4 } else { 0 })
    }
}

/// 将 4 位 nibble 解包为单个通道配置 (enable, is_rx, high_speed)
pub const fn unpack_channel_nibble(nibble: u8) -> (bool, bool, bool) {
    let enable = (nibble & 0x01) != 0;
    let is_rx = (nibble & 0x02) != 0;
    let high_speed = (nibble & 0x04) != 0;
    (enable, is_rx, high_speed)
}

/// 将 12 通道配置编码为两个 32 位载荷字 (Payload 0: CH0..7, Payload 1: CH8..11)
pub fn pack_12_channels(channels: &[(bool, bool, bool); 12]) -> (u32, u32) {
    let mut p0 = 0u32;
    let mut p1 = 0u32;
    for ch in 0..8 {
        let (en, rx, hs) = channels[ch];
        p0 |= (pack_channel_nibble(en, rx, hs) as u32) << (ch * 4);
    }
    for ch in 8..12 {
        let (en, rx, hs) = channels[ch];
        p1 |= (pack_channel_nibble(en, rx, hs) as u32) << ((ch - 8) * 4);
    }
    (p0, p1)
}

/// 将两个 32 位载荷字解包为 12 通道配置
pub fn unpack_12_channels(p0: u32, p1: u32) -> [(bool, bool, bool); 12] {
    let mut channels = [(false, false, false); 12];
    for ch in 0..8 {
        let nibble = ((p0 >> (ch * 4)) & 0x0F) as u8;
        channels[ch] = unpack_channel_nibble(nibble);
    }
    for ch in 8..12 {
        let nibble = ((p1 >> ((ch - 8) * 4)) & 0x0F) as u8;
        channels[ch] = unpack_channel_nibble(nibble);
    }
    channels
}

pub const PROFILE_MAGIC: u16 = 0x4239; // "B9"
pub const PROFILE_RECORD_LEN: usize = 512;

/// 单槽位配置记录 (512 字节确定性紧凑结构，完整支持 12 通道配置与 LabelFilter 持久化)
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct ProfileRecord {
    pub magic: u16,
    pub slot: u8,
    pub is_valid: u8,
    pub name: [u8; 32],
    pub directions: [u8; 12], // 0: Disabled, 1: Tx, 2: Rx
    pub speeds: [u8; 12],     // 0: Low (12.5k), 1: High (100k)
    pub filter_modes: [u8; 12], // 0: AcceptAll, 1: Whitelist, 2: Blacklist
    pub filter_bitmaps: [[u32; 8]; 12], // 12 通道 256 位掩码
    pub crc16: u16,
}

impl Default for ProfileRecord {
    fn default() -> Self {
        Self {
            magic: PROFILE_MAGIC,
            slot: 0,
            is_valid: 0,
            name: [0; 32],
            directions: [0; 12],
            speeds: [1; 12],
            filter_modes: [0; 12],
            filter_bitmaps: [[0; 8]; 12],
            crc16: 0,
        }
    }
}

impl ProfileRecord {
    /// 序列化为 512 字节小端序字节数组
    pub fn to_bytes(&self) -> [u8; PROFILE_RECORD_LEN] {
        let mut buf = [0u8; PROFILE_RECORD_LEN];
        buf[0..2].copy_from_slice(&self.magic.to_le_bytes());
        buf[2] = self.slot;
        buf[3] = self.is_valid;
        buf[4..36].copy_from_slice(&self.name);
        buf[36..48].copy_from_slice(&self.directions);
        buf[48..60].copy_from_slice(&self.speeds);
        buf[60..72].copy_from_slice(&self.filter_modes);

        // 72..76 保留对齐
        let mut offset = 76;
        for ch in 0..12 {
            for word in 0..8 {
                let bytes = self.filter_bitmaps[ch][word].to_le_bytes();
                buf[offset..offset + 4].copy_from_slice(&bytes);
                offset += 4;
            }
        }
        // offset: 76 + 12 * 32 = 460. 460..510 为保留字段
        buf[510..512].copy_from_slice(&self.crc16.to_le_bytes());
        buf
    }

    /// 从 512 字节切片反序列化
    pub fn from_bytes(buf: &[u8; PROFILE_RECORD_LEN]) -> Self {
        let magic = u16::from_le_bytes([buf[0], buf[1]]);
        let slot = buf[2];
        let is_valid = buf[3];
        let mut name = [0u8; 32];
        name.copy_from_slice(&buf[4..36]);
        let mut directions = [0u8; 12];
        directions.copy_from_slice(&buf[36..48]);
        let mut speeds = [0u8; 12];
        speeds.copy_from_slice(&buf[48..60]);
        let mut filter_modes = [0u8; 12];
        filter_modes.copy_from_slice(&buf[60..72]);

        let mut filter_bitmaps = [[0u32; 8]; 12];
        let mut offset = 76;
        for ch in 0..12 {
            for word in 0..8 {
                filter_bitmaps[ch][word] = u32::from_le_bytes([
                    buf[offset],
                    buf[offset + 1],
                    buf[offset + 2],
                    buf[offset + 3],
                ]);
                offset += 4;
            }
        }
        let crc16 = u16::from_le_bytes([buf[510], buf[511]]);
        Self {
            magic,
            slot,
            is_valid,
            name,
            directions,
            speeds,
            filter_modes,
            filter_bitmaps,
            crc16,
        }
    }

    pub fn compute_crc(&self) -> u16 {
        let bytes = self.to_bytes();
        crc16_ccitt(&bytes[..PROFILE_RECORD_LEN - 2])
    }

    pub fn verify(&self) -> bool {
        self.magic == PROFILE_MAGIC && self.is_valid == 1 && self.compute_crc() == self.crc16
    }

    /// 获取特定通道的 LabelFilter 实例
    pub fn get_channel_filter(&self, channel: usize) -> crate::filter::LabelFilter {
        if channel >= 12 {
            return crate::filter::LabelFilter::new_accept_all();
        }
        let mode = crate::filter::FilterMode::from_u8(self.filter_modes[channel]);
        crate::filter::LabelFilter::from_raw(mode, self.filter_bitmaps[channel])
    }
}

/// 在字节切片中采用 1 字节增量滑动窗口搜索第一个合法的 16 字节帧
///
/// 返回 `Some((frame, consumed_bytes))`，其中 `consumed_bytes` 为包含跳过的噪音字节在内的全部消耗长度；
/// 若切片内无完整有效帧，返回 `None`。
pub fn find_frame_in_stream(buf: &[u8]) -> Option<(BinaryFrame, usize)> {
    if buf.len() < BINARY_FRAME_LEN {
        return None;
    }
    for i in 0..=(buf.len() - BINARY_FRAME_LEN) {
        if buf[i] == BINARY_MAGIC[0] && buf[i + 1] == BINARY_MAGIC[1] {
            if let Ok(frame) = BinaryFrame::decode(&buf[i..i + BINARY_FRAME_LEN]) {
                return Some((frame, i + BINARY_FRAME_LEN));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_binary_frame_roundtrip() {
        let frame = BinaryFrame::new_full(
            frame_types::CMD_REQ,
            3,
            opcodes::SET_CHANNEL_CFG,
            42,
            0x12345678,
            0x9ABCDEF0,
        );

        let mut buf = [0u8; BINARY_FRAME_LEN];
        frame.encode(&mut buf);

        assert_eq!(buf[0], 0x41);
        assert_eq!(buf[1], 0x29);
        assert_eq!(buf[2], (frame_types::CMD_REQ << 4) | 3);
        assert_eq!(buf[3], opcodes::SET_CHANNEL_CFG);

        let decoded = BinaryFrame::decode(&buf).expect("Decode should succeed");
        assert_eq!(decoded, frame);
        assert_eq!(decoded.opcode(), opcodes::SET_CHANNEL_CFG);
        assert_eq!(decoded.channel, 3);
        assert_eq!(decoded.seq_num, 42);
        assert_eq!(decoded.payload_0, 0x12345678);
        assert_eq!(decoded.payload_1, 0x9ABCDEF0);
    }

    #[test]
    fn test_crc_error_detection() {
        let frame = BinaryFrame::new_cmd_ack(1, ack_status::OK, 100, 0x11223344, 0x55667788);
        let mut buf = [0u8; BINARY_FRAME_LEN];
        frame.encode(&mut buf);

        // 翻转载荷中某个比特
        buf[8] ^= 0x01;

        match BinaryFrame::decode(&buf) {
            Err(DecodeError::CrcMismatch { .. }) => {}
            other => panic!("Expected CrcMismatch, got {:?}", other),
        }
    }

    #[test]
    fn test_nibble_packing_roundtrip() {
        let mut channels = [(false, false, false); 12];
        channels[0] = (true, false, true); // CH0: Enable, Tx, 100k
        channels[1] = (true, true, false); // CH1: Enable, Rx, 12.5k
        channels[7] = (true, true, true);  // CH7: Enable, Rx, 100k
        channels[8] = (true, false, false); // CH8: Enable, Tx, 12.5k
        channels[11] = (true, true, true); // CH11: Enable, Rx, 100k

        let (p0, p1) = pack_12_channels(&channels);
        let unpacked = unpack_12_channels(p0, p1);

        assert_eq!(channels, unpacked);
    }

    #[test]
    fn test_sliding_window_resync() {
        // 构造带有 3 字节噪声前缀的字节流
        let frame = BinaryFrame::new_cmd_req(0, opcodes::PING, 1, 0xAABBCCDD, 0);
        let mut frame_bytes = [0u8; BINARY_FRAME_LEN];
        frame.encode(&mut frame_bytes);

        let mut stream = [0u8; 3 + BINARY_FRAME_LEN];
        stream[0] = 0xFF; // Noise
        stream[1] = 0x41; // Partial false magic
        stream[2] = 0x00; // Noise
        stream[3..].copy_from_slice(&frame_bytes);

        let result = find_frame_in_stream(&stream);
        assert!(result.is_some());
        let (found_frame, consumed) = result.unwrap();
        assert_eq!(found_frame, frame);
        assert_eq!(consumed, 3 + BINARY_FRAME_LEN);
    }

    #[test]
    fn test_profile_record_roundtrip() {
        let mut rec = ProfileRecord::default();
        rec.slot = 1;
        rec.is_valid = 1;
        rec.name[..5].copy_from_slice(b"Bench");
        rec.directions[0] = 1; // Tx
        rec.directions[1] = 2; // Rx
        rec.speeds[0] = 1;     // High
        rec.speeds[1] = 0;     // Low

        // 配置 CH1 为白名单，允许 0o310 和 0o203
        rec.filter_modes[1] = 1;
        let mut f1 = crate::filter::LabelFilter::new_whitelist();
        f1.allow_label(crate::label::Label::from_octal(0o310)).unwrap();
        f1.allow_label(crate::label::Label::from_octal(0o203)).unwrap();
        rec.filter_bitmaps[1] = f1.bitmap();

        rec.crc16 = rec.compute_crc();
        assert!(rec.verify());

        let bytes = rec.to_bytes();
        let restored = ProfileRecord::from_bytes(&bytes);
        assert_eq!(restored, rec);
        assert!(restored.verify());

        // 验证通道过滤器正确还原
        let restored_f1 = restored.get_channel_filter(1);
        assert_eq!(restored_f1.mode(), crate::filter::FilterMode::Whitelist);
        assert_eq!(restored_f1.bitmap(), f1.bitmap());
        assert!(restored_f1.matches(crate::label::Label::from_octal(0o310), true));
        assert!(restored_f1.matches(crate::label::Label::from_octal(0o203), true));
        assert!(!restored_f1.matches(crate::label::Label::from_octal(0o377), true));
    }
}

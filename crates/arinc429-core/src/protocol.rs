//! 上位机通信协议模块 (no_std)
//!
//! 支持两种通信形态：
//! 1. 高速定长 16 字节二进制遥测帧 (BinaryFrame): 完美整除 USB 64 字节端点 ($4 \times 16 = 64$，零填充碎片)；
//!    包含帧类型、4 位通道号、状态标志、16 位递增包序号、微秒级时间戳、32 位 ARINC 字与 CRC16；
//! 2. 交互式 ASCII 命令与格式化输出: 适用于终端调试助手 (PuTTY/minicom/串口助手)，支持直接文本交互与配置。

use crate::label::Label;
use crate::parity::ParityMode;

pub const BINARY_MAGIC: [u8; 2] = [0x41, 0x29]; // "A)"
pub const BINARY_FRAME_LEN: usize = 16;

/// 帧类型枚举与常量 (高 4 位表示，0~15)
pub mod frame_types {
    /// 正常 ARINC 429 接收/发送字数据载荷
    pub const ARINC_DATA: u8 = 0x01;
    /// 上位机配置或控制命令执行确认 (ACK / NAK)
    pub const CMD_ACK: u8    = 0x02;
    /// 异步告警与硬件总线故障事件
    pub const ASYNC_EVENT: u8 = 0x03;
    /// 内建自测试 (BIT) 诊断评估报告
    pub const BIT_REPORT: u8  = 0x04;
}

/// 状态标志位
pub mod flags {
    pub const PARITY_ERROR: u8 = 1 << 0;
    pub const SPEED_100K: u8   = 1 << 1; // 1: 100k (高速), 0: 12.5k (低速)
    pub const GAP_ERROR: u8    = 1 << 2;
    pub const IS_TX_ECHO: u8   = 1 << 3; // 该帧为发送回显还是接收捕获
    pub const BUS_FAULT: u8    = 1 << 4; // 双引脚同高或持续高电平故障
}

/// 定长 16 字节的高性能二进制遥测帧 (完美整除 64 字节 USB CDC 端点)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct BinaryFrame {
    pub frame_type: u8,
    pub channel: u8,
    pub flags: u8,
    pub seq_num: u16,
    pub timestamp_us: u32,
    pub raw_word: u32,
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
    /// 创建标准 ARINC 数据帧 (序号默认为 0)
    pub const fn new(channel: u8, flags: u8, timestamp_us: u32, raw_word: u32) -> Self {
        Self {
            frame_type: frame_types::ARINC_DATA,
            channel: channel & 0x0F,
            flags,
            seq_num: 0,
            timestamp_us,
            raw_word,
        }
    }

    /// 创建带包序号与指定帧类型的完整帧
    pub const fn new_full(
        frame_type: u8,
        channel: u8,
        flags: u8,
        seq_num: u16,
        timestamp_us: u32,
        raw_word: u32,
    ) -> Self {
        Self {
            frame_type: frame_type & 0x0F,
            channel: channel & 0x0F,
            flags,
            seq_num,
            timestamp_us,
            raw_word,
        }
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

        let ts_bytes = self.timestamp_us.to_le_bytes();
        out[6..10].copy_from_slice(&ts_bytes);

        let word_bytes = self.raw_word.to_le_bytes();
        out[10..14].copy_from_slice(&word_bytes);

        // 计算 CRC-16 (覆盖 type_and_channel 到 raw_word 共 12 字节: 索引 2..14)
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
        let timestamp_us = u32::from_le_bytes([buf[6], buf[7], buf[8], buf[9]]);
        let raw_word = u32::from_le_bytes([buf[10], buf[11], buf[12], buf[13]]);

        Ok(Self {
            frame_type,
            channel,
            flags,
            seq_num,
            timestamp_us,
            raw_word,
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

/// 上位机发给固件的控制命令
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostCommand {
    /// 动态配置通道: CHCFG <ch> <ENABLE|DISABLE> [TX|RX] [100K|12K5]
    ConfigureChannel {
        channel: u8,
        enable: bool,
        is_tx: bool,
        high_speed: bool,
    },
    /// 动态配置通道过滤: CHCFG <ch> FILTER <ALLOW|BLOCK|CLEAR> [octal]
    ChannelFilter {
        channel: u8,
        action: FilterAction,
    },
    /// 发送构造的 ARINC 字: TX <ch> <octal_label> <sdi> <data_hex> <ssm>
    SendConstructed {
        channel: u8,
        label: Label,
        sdi: u8,
        data: u32,
        ssm: u8,
        parity: ParityMode,
    },
    /// 发送原始 32 位十六进制字: TXRAW <ch> <hex_word>
    SendRaw {
        channel: u8,
        raw_word: u32,
    },
    /// 切换波特率: CHCFG <ch> SPEED <100K|12K5>
    SetSpeed {
        channel: u8,
        high_speed: bool,
    },
    /// 切换推送模式: MODE <BIN|ASCII>
    SetMode {
        binary_mode: bool,
    },
    /// 执行内建自测试: BIT <tx_ch> <rx_ch>
    RunBit {
        tx_channel: u8,
        rx_channel: u8,
    },
    /// 查询状态: STATUS [ch]
    QueryStatus {
        channel: Option<u8>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterAction {
    Allow(Label),
    Block(Label),
    Clear,
}

/// 命令解析错误枚举
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum CommandParseError {
    EmptyCommand,
    UnknownCommand,
    MissingArgument(&'static str),
    InvalidChannel,
    InvalidSpeed,
    InvalidHexWord,
    InvalidOctalLabel,
    InvalidSdi(u8),
    InvalidSsm(u8),
    InvalidDataHex,
    DataOutOfRange(u32),
    UnknownSubAction,
}

fn strip_hex_prefix(s: &str) -> &str {
    if let Some(stripped) = s.strip_prefix("0x") {
        stripped
    } else if let Some(stripped) = s.strip_prefix("0X") {
        stripped
    } else {
        s
    }
}

impl HostCommand {
    /// 解析单行文本指令 (忽略前后空白，不区分大小写)
    pub fn parse(line: &str) -> Result<Self, CommandParseError> {
        let mut parts = line.split_whitespace();
        let cmd = parts.next().ok_or(CommandParseError::EmptyCommand)?;

        if cmd.eq_ignore_ascii_case("STATUS") {
            let ch_opt = parts.next().and_then(|s| s.parse::<u8>().ok());
            return Ok(Self::QueryStatus { channel: ch_opt });
        }

        if cmd.eq_ignore_ascii_case("CHCFG") {
            let ch_str = parts.next().ok_or(CommandParseError::MissingArgument("channel"))?;
            let channel = ch_str.parse::<u8>().map_err(|_| CommandParseError::InvalidChannel)?;
            let action = parts.next().ok_or(CommandParseError::MissingArgument("action"))?;

            if action.eq_ignore_ascii_case("DISABLE") {
                return Ok(Self::ConfigureChannel {
                    channel,
                    enable: false,
                    is_tx: false,
                    high_speed: true,
                });
            } else if action.eq_ignore_ascii_case("ENABLE") {
                let dir_str = parts.next().ok_or(CommandParseError::MissingArgument("direction (TX/RX)"))?;
                let speed_str = parts.next().ok_or(CommandParseError::MissingArgument("speed (100K/12K5)"))?;
                let is_tx = dir_str.eq_ignore_ascii_case("TX");
                let high_speed = speed_str.eq_ignore_ascii_case("100K");
                return Ok(Self::ConfigureChannel {
                    channel,
                    enable: true,
                    is_tx,
                    high_speed,
                });
            } else if action.eq_ignore_ascii_case("SPEED") {
                let speed_str = parts.next().ok_or(CommandParseError::MissingArgument("speed (100K/12K5)"))?;
                let high_speed = speed_str.eq_ignore_ascii_case("100K");
                return Ok(Self::SetSpeed { channel, high_speed });
            } else if action.eq_ignore_ascii_case("FILTER") {
                let sub = parts.next().ok_or(CommandParseError::MissingArgument("ALLOW/BLOCK/CLEAR"))?;
                if sub.eq_ignore_ascii_case("CLEAR") {
                    return Ok(Self::ChannelFilter {
                        channel,
                        action: FilterAction::Clear,
                    });
                }
                let lbl_str = parts.next().ok_or(CommandParseError::MissingArgument("octal label"))?;
                let label = Label::parse_str(lbl_str).map_err(|_| CommandParseError::InvalidOctalLabel)?;
                if sub.eq_ignore_ascii_case("ALLOW") {
                    return Ok(Self::ChannelFilter {
                        channel,
                        action: FilterAction::Allow(label),
                    });
                } else if sub.eq_ignore_ascii_case("BLOCK") {
                    return Ok(Self::ChannelFilter {
                        channel,
                        action: FilterAction::Block(label),
                    });
                } else {
                    return Err(CommandParseError::UnknownSubAction);
                }
            } else {
                return Err(CommandParseError::UnknownSubAction);
            }
        }

        if cmd.eq_ignore_ascii_case("BIT") {
            let tx_str = parts.next().ok_or(CommandParseError::MissingArgument("tx_ch"))?;
            let rx_str = parts.next().ok_or(CommandParseError::MissingArgument("rx_ch"))?;
            let tx_channel = tx_str.parse::<u8>().map_err(|_| CommandParseError::InvalidChannel)?;
            let rx_channel = rx_str.parse::<u8>().map_err(|_| CommandParseError::InvalidChannel)?;
            return Ok(Self::RunBit { tx_channel, rx_channel });
        }

        if cmd.eq_ignore_ascii_case("MODE") {
            let mode_str = parts.next().ok_or(CommandParseError::MissingArgument("BIN/ASCII"))?;
            let is_bin = mode_str.eq_ignore_ascii_case("BIN");
            return Ok(Self::SetMode { binary_mode: is_bin });
        }

        if cmd.eq_ignore_ascii_case("TXRAW") {
            let ch_str = parts.next().ok_or(CommandParseError::MissingArgument("channel"))?;
            let hex_token = parts.next().ok_or(CommandParseError::MissingArgument("hex_word"))?;
            let channel = ch_str.parse::<u8>().map_err(|_| CommandParseError::InvalidChannel)?;
            let hex_str = strip_hex_prefix(hex_token);
            let raw_word = u32::from_str_radix(hex_str, 16).map_err(|_| CommandParseError::InvalidHexWord)?;
            return Ok(Self::SendRaw { channel, raw_word });
        }

        if cmd.eq_ignore_ascii_case("TX") {
            let ch_str = parts.next().ok_or(CommandParseError::MissingArgument("channel"))?;
            let label_str = parts.next().ok_or(CommandParseError::MissingArgument("octal label"))?;
            let sdi_str = parts.next().ok_or(CommandParseError::MissingArgument("sdi"))?;
            let data_str = parts.next().ok_or(CommandParseError::MissingArgument("data_hex"))?;
            let ssm_str = parts.next().ok_or(CommandParseError::MissingArgument("ssm"))?;

            let channel = ch_str.parse::<u8>().map_err(|_| CommandParseError::InvalidChannel)?;
            let label = Label::parse_str(label_str).map_err(|_| CommandParseError::InvalidOctalLabel)?;
            let sdi = sdi_str.parse::<u8>().map_err(|_| CommandParseError::InvalidSdi(0))?;
            if sdi > 3 {
                return Err(CommandParseError::InvalidSdi(sdi));
            }
            let hex_str = strip_hex_prefix(data_str);
            let data = u32::from_str_radix(hex_str, 16).map_err(|_| CommandParseError::InvalidDataHex)?;
            if data > 0x0007_FFFF {
                return Err(CommandParseError::DataOutOfRange(data));
            }
            let ssm = ssm_str.parse::<u8>().map_err(|_| CommandParseError::InvalidSsm(0))?;
            if ssm > 3 {
                return Err(CommandParseError::InvalidSsm(ssm));
            }

            return Ok(Self::SendConstructed {
                channel,
                label,
                sdi,
                data,
                ssm,
                parity: ParityMode::Odd,
            });
        }

        Err(CommandParseError::UnknownCommand)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_binary_frame_codec() {
        let frame = BinaryFrame::new_full(frame_types::ARINC_DATA, 1, flags::SPEED_100K, 42, 1234567, 0x812345C8);
        let mut buf = [0u8; BINARY_FRAME_LEN];
        assert_eq!(buf.len(), 16);
        frame.encode(&mut buf);

        let decoded = BinaryFrame::decode(&buf).expect("Decode should succeed");
        assert_eq!(decoded, frame);
        assert_eq!(decoded.channel, 1);
        assert_eq!(decoded.seq_num, 42);
        assert_eq!(decoded.frame_type, frame_types::ARINC_DATA);
    }

    #[test]
    fn test_crc_error() {
        let frame = BinaryFrame::new(1, 0, 100, 0x12345678);
        let mut buf = [0u8; BINARY_FRAME_LEN];
        frame.encode(&mut buf);

        // 篡改载荷字节
        buf[6] ^= 0xFF;
        assert!(BinaryFrame::decode(&buf).is_err());
    }

    #[test]
    fn test_parse_commands() {
        let cmd1 = HostCommand::parse("CHCFG 0 SPEED 100K").unwrap();
        assert_eq!(cmd1, HostCommand::SetSpeed { channel: 0, high_speed: true });

        let cmd2 = HostCommand::parse("TXRAW 1 0x812345C8").unwrap();
        assert_eq!(cmd2, HostCommand::SendRaw { channel: 1, raw_word: 0x812345C8 });

        let cmd3 = HostCommand::parse("CHCFG 0 FILTER ALLOW 310").unwrap();
        assert_eq!(
            cmd3,
            HostCommand::ChannelFilter {
                channel: 0,
                action: FilterAction::Allow(Label::from_octal(0o310)),
            }
        );

        let cmd4 = HostCommand::parse("BIT 0 1").unwrap();
        assert_eq!(cmd4, HostCommand::RunBit { tx_channel: 0, rx_channel: 1 });

        // 测试新增的 CHCFG 命令
        let cmd5 = HostCommand::parse("CHCFG 2 ENABLE RX 12K5").unwrap();
        assert_eq!(
            cmd5,
            HostCommand::ConfigureChannel {
                channel: 2,
                enable: true,
                is_tx: false,
                high_speed: false,
            }
        );

        let cmd6 = HostCommand::parse("CHCFG 3 DISABLE").unwrap();
        assert_eq!(
            cmd6,
            HostCommand::ConfigureChannel {
                channel: 3,
                enable: false,
                is_tx: false,
                high_speed: true,
            }
        );

        let cmd7 = HostCommand::parse("CHCFG 0 FILTER ALLOW 203").unwrap();
        assert_eq!(
            cmd7,
            HostCommand::ChannelFilter {
                channel: 0,
                action: FilterAction::Allow(Label::from_octal(0o203)),
            }
        );

        let cmd8 = HostCommand::parse("STATUS 1").unwrap();
        assert_eq!(cmd8, HostCommand::QueryStatus { channel: Some(1) });

        let cmd9 = HostCommand::parse("MODE BIN").unwrap();
        assert_eq!(cmd9, HostCommand::SetMode { binary_mode: true });

        let cmd10 = HostCommand::parse("CHCFG 1 FILTER BLOCK 377").unwrap();
        assert_eq!(
            cmd10,
            HostCommand::ChannelFilter {
                channel: 1,
                action: FilterAction::Block(Label::from_octal(0o377)),
            }
        );

        let cmd11 = HostCommand::parse("CHCFG 1 FILTER CLEAR").unwrap();
        assert_eq!(
            cmd11,
            HostCommand::ChannelFilter {
                channel: 1,
                action: FilterAction::Clear,
            }
        );
    }

    #[test]
    fn test_parse_commands_robustness_and_errors() {
        assert_eq!(HostCommand::parse(""), Err(CommandParseError::EmptyCommand));
        assert_eq!(HostCommand::parse("UNKNOWN_CMD"), Err(CommandParseError::UnknownCommand));

        // SDI / SSM 越界必须返回对应错误
        assert_eq!(HostCommand::parse("TX 0 310 4 0x12 2"), Err(CommandParseError::InvalidSdi(4)));
        assert_eq!(HostCommand::parse("TX 0 310 1 0x12 4"), Err(CommandParseError::InvalidSsm(4)));
        assert_eq!(
            HostCommand::parse("TX 0 310 1 0x80000 2"),
            Err(CommandParseError::DataOutOfRange(0x80000))
        );

        // CHCFG 语法错误检测
        assert_eq!(
            HostCommand::parse("CHCFG"),
            Err(CommandParseError::MissingArgument("channel"))
        );
        assert_eq!(
            HostCommand::parse("CHCFG abc"),
            Err(CommandParseError::InvalidChannel)
        );
        assert_eq!(
            HostCommand::parse("CHCFG 0 UNKNOWN"),
            Err(CommandParseError::UnknownSubAction)
        );
        assert_eq!(
            HostCommand::parse("CHCFG 0 ENABLE"),
            Err(CommandParseError::MissingArgument("direction (TX/RX)"))
        );
    }

    #[test]
    fn test_binary_frame_decode_error_types() {
        // 缓冲区长度不足
        assert_eq!(BinaryFrame::decode(&[0x41]), Err(DecodeError::BufferTooShort));

        // Magic 错误
        let mut invalid_magic = [0u8; BINARY_FRAME_LEN];
        invalid_magic[0] = 0x00;
        invalid_magic[1] = 0x00;
        assert_eq!(BinaryFrame::decode(&invalid_magic), Err(DecodeError::InvalidMagic));
    }
}

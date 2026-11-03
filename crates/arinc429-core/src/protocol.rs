//! 上位机通信协议模块
//!
//! 支持两种通信形态：
//! 1. 高速定长二进制帧 (BinaryFrame): 14 字节定长，包含 CRC16、微秒级时间戳、通道号与状态位，适用于高速上位机分析仪；
//! 2. 交互式 ASCII 命令与格式化输出: 适用于终端调试助手 (PuTTY/minicom/串口助手)，支持直接文本交互与配置。

use crate::label::Label;
use crate::parity::ParityMode;

pub const BINARY_MAGIC: [u8; 2] = [0x41, 0x29]; // "A)"
pub const BINARY_FRAME_LEN: usize = 14;

/// 状态标志位
pub mod flags {
    pub const PARITY_ERROR: u8 = 1 << 0;
    pub const SPEED_100K: u8    = 1 << 1; // 1: 100k (高速), 0: 12.5k (低速)
    pub const GAP_ERROR: u8     = 1 << 2;
    pub const IS_TX_ECHO: u8    = 1 << 3; // 该帧为发送回显还是接收捕获
}

/// 定长 14 字节的高性能二进制遥测帧
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct BinaryFrame {
    pub channel: u8,
    pub flags: u8,
    pub timestamp_us: u32,
    pub raw_word: u32,
}

impl BinaryFrame {
    pub fn new(channel: u8, flags: u8, timestamp_us: u32, raw_word: u32) -> Self {
        Self {
            channel,
            flags,
            timestamp_us,
            raw_word,
        }
    }

    /// 序列化为 14 字节定长数据
    pub fn encode(&self, out: &mut [u8; BINARY_FRAME_LEN]) {
        out[0] = BINARY_MAGIC[0];
        out[1] = BINARY_MAGIC[1];
        out[2] = self.channel;
        out[3] = self.flags;

        let ts_bytes = self.timestamp_us.to_le_bytes();
        out[4..8].copy_from_slice(&ts_bytes);

        let word_bytes = self.raw_word.to_le_bytes();
        out[8..12].copy_from_slice(&word_bytes);

        // 计算 CRC-16 (覆盖 channel 到 raw_word 共 10 字节)
        let crc = crc16_ccitt(&out[2..12]);
        let crc_bytes = crc.to_le_bytes();
        out[12..14].copy_from_slice(&crc_bytes);
    }

    /// 从 14 字节切片反序列化
    pub fn decode(buf: &[u8]) -> Result<Self, &'static str> {
        if buf.len() < BINARY_FRAME_LEN {
            return Err("Buffer too short");
        }
        if buf[0] != BINARY_MAGIC[0] || buf[1] != BINARY_MAGIC[1] {
            return Err("Invalid magic bytes");
        }

        let expected_crc = u16::from_le_bytes([buf[12], buf[13]]);
        let actual_crc = crc16_ccitt(&buf[2..12]);
        if expected_crc != actual_crc {
            return Err("CRC16 mismatch");
        }

        let channel = buf[2];
        let flags = buf[3];
        let timestamp_us = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]);
        let raw_word = u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]);

        Ok(Self {
            channel,
            flags,
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

/// 上位机发给固件的 ASCII 控制命令
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostCommand {
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
    /// 切换波特率: SPEED <ch> <100K|12K5>
    SetSpeed {
        channel: u8,
        high_speed: bool,
    },
    /// 配置白名单过滤: FILTER <ch> ALLOW <octal_label>
    FilterAllow {
        channel: u8,
        label: Label,
    },
    /// 配置黑名单过滤: FILTER <ch> BLOCK <octal_label>
    FilterBlock {
        channel: u8,
        label: Label,
    },
    /// 清除过滤规则: FILTER <ch> CLEAR
    FilterClear {
        channel: u8,
    },
    /// 切换推送模式: MODE <BIN|ASCII>
    SetMode {
        binary_mode: bool,
    },
    /// 查询状态: STATUS
    QueryStatus,
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
    pub fn parse(line: &str) -> Result<Self, &'static str> {
        let mut parts = line.split_whitespace();
        let cmd = parts.next().ok_or("Empty command")?;

        if cmd.eq_ignore_ascii_case("STATUS") {
            return Ok(Self::QueryStatus);
        }

        if cmd.eq_ignore_ascii_case("MODE") {
            let mode_str = parts.next().ok_or("Usage: MODE <BIN|ASCII>")?;
            let is_bin = mode_str.eq_ignore_ascii_case("BIN");
            return Ok(Self::SetMode { binary_mode: is_bin });
        }

        if cmd.eq_ignore_ascii_case("SPEED") {
            let ch_str = parts.next().ok_or("Usage: SPEED <ch> <100K|12K5>")?;
            let speed_str = parts.next().ok_or("Usage: SPEED <ch> <100K|12K5>")?;
            let channel = ch_str.parse::<u8>().map_err(|_| "Invalid channel")?;
            let high_speed = speed_str.eq_ignore_ascii_case("100K");
            return Ok(Self::SetSpeed { channel, high_speed });
        }

        if cmd.eq_ignore_ascii_case("TXRAW") {
            let ch_str = parts.next().ok_or("Usage: TXRAW <ch> <hex_word>")?;
            let hex_token = parts.next().ok_or("Usage: TXRAW <ch> <hex_word>")?;
            let channel = ch_str.parse::<u8>().map_err(|_| "Invalid channel")?;
            let hex_str = strip_hex_prefix(hex_token);
            let raw_word = u32::from_str_radix(hex_str, 16).map_err(|_| "Invalid hex word")?;
            return Ok(Self::SendRaw { channel, raw_word });
        }

        if cmd.eq_ignore_ascii_case("FILTER") {
            let ch_str = parts.next().ok_or("Usage: FILTER <ch> <ALLOW|BLOCK|CLEAR> [octal]")?;
            let action = parts.next().ok_or("Usage: FILTER <ch> <ALLOW|BLOCK|CLEAR> [octal]")?;
            let channel = ch_str.parse::<u8>().map_err(|_| "Invalid channel")?;
            if action.eq_ignore_ascii_case("CLEAR") {
                return Ok(Self::FilterClear { channel });
            }
            let label_str = parts.next().ok_or("Missing octal label for ALLOW/BLOCK")?;
            let label = Label::parse_str(label_str)?;
            if action.eq_ignore_ascii_case("ALLOW") {
                return Ok(Self::FilterAllow { channel, label });
            } else if action.eq_ignore_ascii_case("BLOCK") {
                return Ok(Self::FilterBlock { channel, label });
            } else {
                return Err("Unknown FILTER action");
            }
        }

        if cmd.eq_ignore_ascii_case("TX") {
            // TX <ch> <octal_label> <sdi> <data_hex> <ssm>
            let ch_str = parts.next().ok_or("Usage: TX <ch> <octal_label> <sdi> <data_hex> <ssm>")?;
            let label_str = parts.next().ok_or("Usage: TX <ch> <octal_label> <sdi> <data_hex> <ssm>")?;
            let sdi_str = parts.next().ok_or("Usage: TX <ch> <octal_label> <sdi> <data_hex> <ssm>")?;
            let data_str = parts.next().ok_or("Usage: TX <ch> <octal_label> <sdi> <data_hex> <ssm>")?;
            let ssm_str = parts.next().ok_or("Usage: TX <ch> <octal_label> <sdi> <data_hex> <ssm>")?;

            let channel = ch_str.parse::<u8>().map_err(|_| "Invalid channel")?;
            let label = Label::parse_str(label_str)?;
            let sdi = sdi_str.parse::<u8>().map_err(|_| "Invalid sdi")?;
            if sdi > 3 {
                return Err("SDI out of range (0-3)");
            }
            let hex_str = strip_hex_prefix(data_str);
            let data = u32::from_str_radix(hex_str, 16).map_err(|_| "Invalid data hex")?;
            if data > 0x0007_FFFF {
                return Err("Data out of range (max 19 bits: 0x7FFFF)");
            }
            let ssm = ssm_str.parse::<u8>().map_err(|_| "Invalid ssm")?;
            if ssm > 3 {
                return Err("SSM out of range (0-3)");
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

        Err("Unknown command")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_binary_frame_codec() {
        let frame = BinaryFrame::new(1, flags::SPEED_100K, 1234567, 0x812345C8);
        let mut buf = [0u8; BINARY_FRAME_LEN];
        frame.encode(&mut buf);

        let decoded = BinaryFrame::decode(&buf).expect("Decode should succeed");
        assert_eq!(decoded, frame);
    }

    #[test]
    fn test_crc_error() {
        let frame = BinaryFrame::new(1, 0, 100, 0x12345678);
        let mut buf = [0u8; BINARY_FRAME_LEN];
        frame.encode(&mut buf);

        // 故意篡改 1 字节
        buf[5] ^= 0xFF;
        assert!(BinaryFrame::decode(&buf).is_err());
    }

    #[test]
    fn test_parse_commands() {
        let cmd1 = HostCommand::parse("SPEED 0 100K").unwrap();
        assert_eq!(cmd1, HostCommand::SetSpeed { channel: 0, high_speed: true });

        let cmd2 = HostCommand::parse("TXRAW 1 0x812345C8").unwrap();
        assert_eq!(cmd2, HostCommand::SendRaw { channel: 1, raw_word: 0x812345C8 });

        let cmd3 = HostCommand::parse("FILTER 0 ALLOW 310").unwrap();
        assert_eq!(cmd3, HostCommand::FilterAllow { channel: 0, label: Label::from_octal(0o310) });
    }

    #[test]
    fn test_parse_commands_robustness() {
        // >= 9 tokens: should not panic
        let res = HostCommand::parse("STATUS extra1 extra2 extra3 extra4 extra5 extra6 extra7 extra8 extra9");
        assert_eq!(res, Ok(HostCommand::QueryStatus));

        // 0X hex prefix in TXRAW and TX
        let cmd_txraw = HostCommand::parse("TXRAW 0 0X12345678").unwrap();
        assert_eq!(cmd_txraw, HostCommand::SendRaw { channel: 0, raw_word: 0x12345678 });

        let cmd_tx = HostCommand::parse("TX 0 310 1 0XABCD 2").unwrap();
        assert_eq!(cmd_tx, HostCommand::SendConstructed {
            channel: 0,
            label: Label::from_octal(0o310),
            sdi: 1,
            data: 0xABCD,
            ssm: 2,
            parity: ParityMode::Odd,
        });

        // SDI/SSM bounds validation (0..=3)
        assert!(HostCommand::parse("TX 0 310 4 0x12 2").is_err());
        assert!(HostCommand::parse("TX 0 310 1 0x12 4").is_err());
        assert!(HostCommand::parse("TX 0 310 0 0x12 0").is_ok());
        assert!(HostCommand::parse("TX 0 310 3 0x12 3").is_ok());
    }
}

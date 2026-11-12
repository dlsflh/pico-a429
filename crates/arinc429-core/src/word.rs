//! ARINC 429 32 位字核心模型 (no_std)
//!
//! 在固件中用于对 PIO 接收 FIFO 中的 32 位原始字进行高效零开销封装，
//! 并提供零拷贝的 Label 提取与物理奇校验验证。

use crate::label::Label;
use crate::parity::{ParityMode, apply_parity, check_parity};

/// ARINC 429 字构造错误枚举
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum ArincWordError {
    InvalidSdi(u8),
    InvalidData(u32),
    InvalidSsm(u8),
    InvalidParityMode,
}

impl core::fmt::Display for ArincWordError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidSdi(sdi) => write!(f, "SDI out of 2-bit range (0..3): {}", sdi),
            Self::InvalidData(data) => write!(
                f,
                "Data payload out of 19-bit range (0..0x7FFFF): 0x{:X}",
                data
            ),
            Self::InvalidSsm(ssm) => write!(f, "SSM out of 2-bit range (0..3): {}", ssm),
            Self::InvalidParityMode => {
                write!(f, "Invalid parity mode (expected 'odd', 'even', or 'none')")
            }
        }
    }
}

impl core::error::Error for ArincWordError {}

/// ARINC 429 标准 32 位字模型
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct ArincWord(pub u32);

impl ArincWord {
    /// 从原生 32 位值创建（通常直接来自 PIO 接收 FIFO）
    #[inline]
    pub const fn from_raw(raw: u32) -> Self {
        Self(raw)
    }

    /// 获取原生 32 位整数
    #[inline]
    pub const fn to_raw(self) -> u32 {
        self.0
    }

    /// 提取工程八进制 Label
    #[inline]
    pub fn label(self) -> Label {
        Label::from_raw_bus_byte((self.0 & 0xFF) as u8)
    }

    /// 提取 2 位源/目的标识符 SDI (Bits 9~10)
    #[inline]
    pub fn sdi(self) -> u8 {
        ((self.0 >> 8) & 0x03) as u8
    }

    /// 提取 19 位有效数据载荷 (Bits 11~29)
    #[inline]
    pub fn data(self) -> u32 {
        (self.0 >> 10) & 0x7FFFF
    }

    /// 提取 2 位符号/状态矩阵 SSM (Bits 30~31)
    #[inline]
    pub fn ssm(self) -> u8 {
        ((self.0 >> 29) & 0x03) as u8
    }

    /// 提取最高位奇偶校验位 (Bit 32 / bit 31)
    #[inline]
    pub fn parity_bit(self) -> bool {
        (self.0 & 0x8000_0000) != 0
    }

    /// 校验当前字的奇校验（ARINC 429 标准奇校验）
    #[inline]
    pub fn is_parity_valid(self) -> bool {
        check_parity(self.0, ParityMode::Odd)
    }

    /// 根据各字段编码生成 32 位 ARINC 字
    #[inline]
    pub fn from_fields(
        label_octal: u8,
        data: u32,
        sdi: u8,
        ssm: u8,
        parity_mode: ParityMode,
    ) -> Result<Self, ArincWordError> {
        if sdi > 3 {
            return Err(ArincWordError::InvalidSdi(sdi));
        }
        if data > 0x7FFFF {
            return Err(ArincWordError::InvalidData(data));
        }
        if ssm > 3 {
            return Err(ArincWordError::InvalidSsm(ssm));
        }
        let bus_label = Label::from_octal(label_octal).to_raw_bus_byte();
        let raw_31 = (bus_label as u32)
            | (((sdi & 0x03) as u32) << 8)
            | ((data & 0x7FFFF) << 10)
            | (((ssm & 0x03) as u32) << 29);
        Ok(Self(apply_parity(raw_31, parity_mode)))
    }
}

/// 快捷函数：通过字符串指定奇偶校验模式编码 ARINC 429 字
pub fn encode_word(
    label_octal: u8,
    data: u32,
    sdi: u8,
    ssm: u8,
    parity_str: &str,
) -> Result<u32, ArincWordError> {
    let mode = match parity_str {
        p if p.eq_ignore_ascii_case("odd") => ParityMode::Odd,
        p if p.eq_ignore_ascii_case("even") => ParityMode::Even,
        p if p.eq_ignore_ascii_case("none") => ParityMode::None,
        _ => return Err(ArincWordError::InvalidParityMode),
    };
    ArincWord::from_fields(label_octal, data, sdi, ssm, mode).map(|w| w.to_raw())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_word_raw_and_label_extraction() {
        // 八进制 310 (0xC8) 倒序后为 0x13
        let raw = 0xE048D013;
        let word = ArincWord::from_raw(raw);
        assert_eq!(word.to_raw(), raw);
        assert_eq!(word.label().to_octal(), 0o310);
        assert!(word.is_parity_valid());
    }

    #[test]
    fn test_encode_and_decode_symmetry() {
        let label = 0o310;
        let data = 0x12345;
        let sdi = 2;
        let ssm = 3;
        let raw = encode_word(label, data, sdi, ssm, "odd").unwrap();
        let word = ArincWord::from_raw(raw);

        assert_eq!(word.label().to_octal(), label);
        assert_eq!(word.sdi(), sdi);
        assert_eq!(word.data(), data);
        assert_eq!(word.ssm(), ssm);
        assert!(word.is_parity_valid());
    }
}

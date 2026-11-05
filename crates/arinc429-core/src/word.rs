//! ARINC 429 32位字核心模型与字段提取/打包 (no_std)
//!
//! ARINC 429 规范标准帧结构 (32 Bits):
//! - Bit 1 ~ 8:   Label (标号，传输时 LSB 先发，八进制显示需位反转)
//! - Bit 9 ~ 10:  SDI (Source/Destination Identifier，源/目的标识符)
//! - Bit 11 ~ 29: Data (19位数据负载)
//! - Bit 30 ~ 31: SSM (Sign/Status Matrix，符号/状态矩阵)
//! - Bit 32:      Parity (奇偶校验位，通常为奇校验)

use crate::label::Label;
use crate::parity::{apply_parity, check_parity, ParityMode};

/// 字构建与打包校验错误
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum WordBuildError {
    /// SDI 超出 2 位最大值 3
    InvalidSdi(u8),
    /// SSM 超出 2 位最大值 3
    InvalidSsm(u8),
    /// Data 载荷超出 19 位有效上限 0x7FFFF
    DataOverflow(u32),
}

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

    /// 从各逻辑字段直接组装合法 ARINC 字 (执行 Fail-Fast 校验)
    pub fn from_fields(
        label: Label,
        sdi: u8,
        data: u32,
        ssm: u8,
        parity_mode: ParityMode,
    ) -> Result<Self, WordBuildError> {
        if sdi > 3 {
            return Err(WordBuildError::InvalidSdi(sdi));
        }
        if ssm > 3 {
            return Err(WordBuildError::InvalidSsm(ssm));
        }
        if data > 0x0007_FFFF {
            return Err(WordBuildError::DataOverflow(data));
        }

        let bus_label = label.to_raw_bus_byte() as u32;
        let sdi_bits = (sdi as u32) << 8;
        let data_bits = data << 10;
        let ssm_bits = (ssm as u32) << 29;

        let raw_31 = bus_label | sdi_bits | data_bits | ssm_bits;
        Ok(Self(apply_parity(raw_31, parity_mode)))
    }

    /// 提取工程八进制 Label
    #[inline]
    pub fn label(self) -> Label {
        Label::from_raw_bus_byte((self.0 & 0xFF) as u8)
    }

    /// 提取 2 位 SDI (Bits 9~10)
    #[inline]
    pub fn sdi(self) -> u8 {
        ((self.0 >> 8) & 0x03) as u8
    }

    /// 提取 19 位 Data 负载 (Bits 11~29)
    #[inline]
    pub fn data_field(self) -> u32 {
        (self.0 >> 10) & 0x0007_FFFF
    }

    /// 提取 2 位原始 SSM (Bits 30~31)
    #[inline]
    pub fn ssm_raw(self) -> u8 {
        ((self.0 >> 29) & 0x03) as u8
    }

    /// 提取最高位 Parity (Bit 32)
    #[inline]
    pub fn parity_bit(self) -> bool {
        (self.0 & 0x8000_0000) != 0
    }

    /// 校验当前字的奇校验（ARINC 429 标准奇校验）
    #[inline]
    pub fn is_parity_valid(self) -> bool {
        check_parity(self.0, ParityMode::Odd)
    }

    /// 校验指定模式的奇偶校验
    #[inline]
    pub fn check_parity_mode(self, mode: ParityMode) -> bool {
        check_parity(self.0, mode)
    }

    /// 自动计算并填入合法的奇偶校验位
    pub fn with_computed_parity(self, mode: ParityMode) -> Self {
        Self(apply_parity(self.0, mode))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_word_from_fields_and_extraction() {
        let word =
            ArincWord::from_fields(Label::from_octal(0o310), 1, 0x1234, 0b11, ParityMode::Odd)
                .unwrap();

        assert_eq!(word.label().to_octal(), 0o310);
        assert_eq!(word.sdi(), 1);
        assert_eq!(word.data_field(), 0x1234);
        assert_eq!(word.ssm_raw(), 0b11);
        assert!(word.is_parity_valid());
    }

    #[test]
    fn test_word_fail_fast_validation() {
        assert_eq!(
            ArincWord::from_fields(Label(0), 4, 0, 0, ParityMode::Odd),
            Err(WordBuildError::InvalidSdi(4))
        );
        assert_eq!(
            ArincWord::from_fields(Label(0), 0, 0, 5, ParityMode::Odd),
            Err(WordBuildError::InvalidSsm(5))
        );
        assert_eq!(
            ArincWord::from_fields(Label(0), 0, 0x0008_0000, 0, ParityMode::Odd),
            Err(WordBuildError::DataOverflow(0x0008_0000))
        );
    }
}

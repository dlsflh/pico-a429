//! ARINC 429 32 位字核心模型 (no_std)
//!
//! 在固件中用于对 PIO 接收 FIFO 中的 32 位原始字进行高效零开销封装，
//! 并提供零拷贝的 Label 提取与物理奇校验验证。

use crate::label::Label;
use crate::parity::{check_parity, ParityMode};

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

    /// 校验当前字的奇校验（ARINC 429 标准奇校验）
    #[inline]
    pub fn is_parity_valid(self) -> bool {
        check_parity(self.0, ParityMode::Odd)
    }
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
}

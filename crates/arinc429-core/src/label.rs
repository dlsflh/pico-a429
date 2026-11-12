//! ARINC 429 Label 转换模块 (no_std)
//!
//! 在 ARINC 429 中：
//! - Label 占用 8-bit（第 1 ~ 8 位）。
//! - 物理线传输规则：Bit 1 (Label LSB) 最先在总线上发出，Bit 8 最后发出。
//! - 航空电子工程规范：Label 始终使用 3 位八进制数字书写（000 ~ 377）。
//!
//! 当硬件按 LSB 顺序接收移入寄存器时，Bit 1 落在寄存器低字节的 bit 0，Bit 8 落在 bit 7。
//! 因此：工程八进制数值与寄存器硬件位之间互为 **按位翻转（Bit Reversal）** 关系。

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Label(pub u8);

impl Label {
    /// 从工程师熟悉的八进制数值创建 Label (例如 0o310 或 Label::from_octal(0o310))
    #[inline]
    pub const fn from_octal(octal_val: u8) -> Self {
        Self(octal_val)
    }

    /// 从 32 位 ARINC 字的原始最低 8 位（物理线先入硬件低位）还原为工程八进制 Label
    #[inline]
    pub const fn from_raw_bus_byte(raw_byte: u8) -> Self {
        Self(raw_byte.reverse_bits())
    }

    /// 转换为工程八进制数值 (0~255, 对应 000~377 八进制)
    #[inline]
    pub const fn to_octal(self) -> u8 {
        self.0
    }

    /// 转换为物理总线寄存器字节 (按位翻转)
    #[inline]
    pub const fn to_raw_bus_byte(self) -> u8 {
        self.0.reverse_bits()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_label_octal_conversion() {
        // 航电经典例子：Label 0310 (八进制)
        // 移入寄存器最低位 (bit 0 = Bit 1, bit 7 = Bit 8):
        // 寄存器中的字节应为: 0b0001_0011 (十六进制 0x13 = 19)
        let restored = Label::from_raw_bus_byte(0x13);
        assert_eq!(restored.to_octal(), 0o310);

        // 测试全 0 与全 1
        assert_eq!(Label::from_raw_bus_byte(0x00).to_octal(), 0o000);
        assert_eq!(Label::from_raw_bus_byte(0xFF).to_octal(), 0o377);
    }
}

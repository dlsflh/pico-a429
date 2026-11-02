//! ARINC 429 Label 转换模块
//!
//! 在 ARINC 429 中：
//! - Label 占用 8-bit（第 1 ~ 8 位）。
//! - 物理线传输规则：Bit 1 (Label LSB) 最先在总线上发出，Bit 8 最后发出。
//! - 航空电子工程规范：Label 始终使用 3 位八进制数字书写（000 ~ 377）。
//!   其中：
//!   - 第一位八进制（最高位，0~3）：对应 Bit 8, Bit 7
//!   - 第二位八进制（中间位，0~7）：对应 Bit 6, Bit 5, Bit 4
//!   - 第三位八进制（最低位，0~7）：对应 Bit 3, Bit 2, Bit 1
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

    /// 转换为物理总线寄存器低 8 位需要的形式 (即翻转位后填入 word & 0xFF)
    #[inline]
    pub const fn to_raw_bus_byte(self) -> u8 {
        self.0.reverse_bits()
    }

    /// 解析八进制字符串（如 "310", "0310", "203"）
    pub fn parse_str(s: &str) -> Result<Self, &'static str> {
        let trimmed = s.trim();
        // 允许带或不带前导 0
        let val = u8::from_str_radix(trimmed, 8).map_err(|_| "Invalid octal label string")?;
        Ok(Self(val))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_label_octal_conversion() {
        // 航电经典例子：Label 0310 (八进制)
        // 0310_8 = 0b11_001_000 (即八进制 3, 1, 0, 十六进制 0xC8 = 200)
        // Bit 8=1, Bit 7=1, Bit 6=0, Bit 5=0, Bit 4=1, Bit 3=0, Bit 2=0, Bit 1=0
        // 在总线硬件上传输时，Bit 1 先发，Bit 8 后发。
        // 若移入寄存器最低位 (bit 0 = Bit 1, bit 7 = Bit 8):
        // 寄存器中的字节应为: 0b0001_0011 (十六进制 0x13 = 19)
        let label = Label::from_octal(0o310);
        assert_eq!(label.to_octal(), 0o310);
        assert_eq!(label.to_raw_bus_byte(), 0x13);

        let restored = Label::from_raw_bus_byte(0x13);
        assert_eq!(restored.to_octal(), 0o310);

        // 测试全 0: 000_8
        let label_0 = Label::from_octal(0o000);
        assert_eq!(label_0.to_raw_bus_byte(), 0x00);

        // 测试全 1: 377_8
        let label_max = Label::from_octal(0o377);
        assert_eq!(label_max.to_raw_bus_byte(), 0xFF);
    }

    #[test]
    fn test_label_parse_str() {
        assert_eq!(Label::parse_str("310").unwrap(), Label::from_octal(0o310));
        assert_eq!(Label::parse_str("0310").unwrap(), Label::from_octal(0o310));
        assert_eq!(Label::parse_str("203").unwrap(), Label::from_octal(0o203));
        assert!(Label::parse_str("400").is_err()); // 超过 377 (8位)
        assert!(Label::parse_str("888").is_err()); // 包含非八进制数字
    }
}

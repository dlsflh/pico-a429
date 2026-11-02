//! ARINC 429 32位字核心模型与字段提取/打包
//!
//! ARINC 429 规范标准帧结构 (32 Bits):
//! - Bit 1 ~ 8:   Label (标号，传输时 LSB 先发，八进制显示需位反转)
//! - Bit 9 ~ 10:  SDI (Source/Destination Identifier，源/目的标识符)
//! - Bit 11 ~ 29: Data (19位数据负载，BNR 或 BCD 或离散位)
//! - Bit 30 ~ 31: SSM (Sign/Status Matrix，符号/状态矩阵)
//! - Bit 32:      Parity (奇偶校验位，通常为奇校验)

use crate::label::Label;
use crate::parity::{apply_parity, check_parity, ParityMode};

/// BNR 模式下的 SSM 含义
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum SsmBnr {
    PlusNorthEastRight = 0b00,
    NoComputedData = 0b01,
    FunctionalTest = 0b10,
    MinusSouthWestLeft = 0b11,
}

impl SsmBnr {
    pub const fn from_bits(bits: u8) -> Self {
        match bits & 0x03 {
            0b00 => Self::PlusNorthEastRight,
            0b01 => Self::NoComputedData,
            0b10 => Self::FunctionalTest,
            _ => Self::MinusSouthWestLeft,
        }
    }
}

/// BCD 模式下的 SSM 含义
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum SsmBcd {
    PlusNorthEastRight = 0b00,
    NoComputedData = 0b01,
    FunctionalTest = 0b10,
    NormalOperation = 0b11,
}

impl SsmBcd {
    pub const fn from_bits(bits: u8) -> Self {
        match bits & 0x03 {
            0b00 => Self::PlusNorthEastRight,
            0b01 => Self::NoComputedData,
            0b10 => Self::FunctionalTest,
            _ => Self::NormalOperation,
        }
    }
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

    /// 解释为 BNR 格式的 SSM
    #[inline]
    pub fn ssm_bnr(self) -> SsmBnr {
        SsmBnr::from_bits(self.ssm_raw())
    }

    /// 解释为 BCD 格式的 SSM
    #[inline]
    pub fn ssm_bcd(self) -> SsmBcd {
        SsmBcd::from_bits(self.ssm_raw())
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

    /// 创建一个构建器便于组装 ARINC 429 字
    pub fn builder() -> ArincWordBuilder {
        ArincWordBuilder::new()
    }
}

/// 组装 ARINC 429 字的便利化构建器
#[derive(Debug, Clone, Copy)]
pub struct ArincWordBuilder {
    label: Label,
    sdi: u8,
    data: u32,
    ssm: u8,
    parity_mode: ParityMode,
}

impl Default for ArincWordBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl ArincWordBuilder {
    pub const fn new() -> Self {
        Self {
            label: Label(0),
            sdi: 0,
            data: 0,
            ssm: 0,
            parity_mode: ParityMode::Odd,
        }
    }

    pub fn label(mut self, label: Label) -> Self {
        self.label = label;
        self
    }

    pub fn octal_label(mut self, octal: u8) -> Self {
        self.label = Label::from_octal(octal);
        self
    }

    pub fn sdi(mut self, sdi: u8) -> Self {
        self.sdi = sdi & 0x03;
        self
    }

    pub fn data(mut self, data: u32) -> Self {
        self.data = data & 0x0007_FFFF;
        self
    }

    pub fn ssm(mut self, ssm: u8) -> Self {
        self.ssm = ssm & 0x03;
        self
    }

    pub fn parity_mode(mut self, mode: ParityMode) -> Self {
        self.parity_mode = mode;
        self
    }

    /// 构建最终可发送的 32 位 ARINC 字
    pub fn build(self) -> ArincWord {
        let bus_label = self.label.to_raw_bus_byte() as u32;
        let sdi_bits = (self.sdi as u32) << 8;
        let data_bits = (self.data as u32) << 10;
        let ssm_bits = (self.ssm as u32) << 29;

        let raw_31 = bus_label | sdi_bits | data_bits | ssm_bits;
        ArincWord(apply_parity(raw_31, self.parity_mode))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_word_builder_and_extraction() {
        // 构建一个包含八进制 Label 0310, SDI=1, Data=0x1234, SSM=3 (Minus/NO) 的 ARINC 字
        let word = ArincWord::builder()
            .octal_label(0o310)
            .sdi(1)
            .data(0x1234)
            .ssm(0b11)
            .parity_mode(ParityMode::Odd)
            .build();

        // 验证各字段提取是否一致
        assert_eq!(word.label().to_octal(), 0o310);
        assert_eq!(word.sdi(), 1);
        assert_eq!(word.data_field(), 0x1234);
        assert_eq!(word.ssm_raw(), 0b11);
        assert!(word.is_parity_valid());
    }

    #[test]
    fn test_parity_injection() {
        // 故意关闭校验位，验证校验失败
        let word_corrupt = ArincWord(0x0000_0000); // 0 个 1，奇校验应为 false
        assert!(!word_corrupt.is_parity_valid());

        // 修正为合法的奇校验
        let fixed = word_corrupt.with_computed_parity(ParityMode::Odd);
        assert!(fixed.is_parity_valid());
        assert_eq!(fixed.0, 0x8000_0000);
    }
}

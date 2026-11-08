//! 模拟前端 (Analog Front End, AFE) 抽象驱动 (面向扩展开放，面向修改封闭)
//!
//! 根据 `12_CHANNEL_DYNAMIC_ROADMAP.md` 规范，ARINC-429 物理层存在独立的高阻驱动使能 (TX_EN)
//! 与硬件模拟斜率控制 (SLR)。
//! 通过提供该 Trait 抽象，系统在开发板直驱回环与外接 74HC595 / PCA9555 高压驱动扩展板之间平滑切换。

pub trait AnalogFrontEnd {
    /// 激活或切断指定物理通道的差分高压驱动 (TX_EN)
    /// - true: 开启驱动输出 (推挽 ±10V)
    /// - false: 呈现高阻态断开 (Hi-Z > 100kΩ)
    fn set_tx_enable(&mut self, channel: u8, enable: bool);

    /// 配置物理通道模拟斜率控制线 (SLR)
    /// - true: 100 kbps 高速斜率 (1.5 µs)
    /// - false: 12.5 kbps 低速斜率 (10 µs)
    fn set_slew_rate(&mut self, channel: u8, high_speed: bool);

    /// 将影子状态寄存器原子刷写至外部硬件 (如通过 SPI/GPIO 锁存至 74HC595 级联芯片)
    fn flush(&mut self);
}

/// 开发板直连模式 (无外部 AFE 扩展板，GPIO 直连回环测试)
#[derive(Debug, Clone, Copy, Default)]
pub struct DirectGpioAfe;

impl AnalogFrontEnd for DirectGpioAfe {
    #[inline(always)]
    fn set_tx_enable(&mut self, _channel: u8, _enable: bool) {}

    #[inline(always)]
    fn set_slew_rate(&mut self, _channel: u8, _high_speed: bool) {}

    #[inline(always)]
    fn flush(&mut self) {}
}

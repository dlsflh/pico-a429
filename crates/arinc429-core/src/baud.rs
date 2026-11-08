//! ARINC 429 波特率与时钟分频计算 (no_std)
//!
//! ARINC 429 规范标准时钟指标：
//! - 高速 (High Speed): 100 kbps ± 1% (99.0 kHz ~ 101.0 kHz), 1 bit = 10.0 us
//! - 低速 (Low Speed): 12.5 kbps ± 1% (12.375 kHz ~ 12.625 kHz), 1 bit = 80.0 us
//!
//! 在 PIO 状态机微代码中，统一收发为 20 个 PIO 时钟周期 (1 bit = 20 PIO cycles)：
//! - 100 kbps 目标 PIO 时钟: 2,000,000 Hz (2 MHz)
//! - 12.5 kbps 目标 PIO 时钟: 250,000 Hz (250 kHz)
//! - 脉冲宽度: 10 周期 (5.0 us)；归零宽度: 10 周期 (5.0 us)；严格保证 50.0% 占空比

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum ArincSpeed {
    /// 高速 100 kbps (PIO 目标 2 MHz)
    High100k,
    /// 低速 12.5 kbps (PIO 目标 250 kHz)
    Low12k5,
}

impl ArincSpeed {
    /// 目标波特率 (bps)
    pub const fn baud_rate(self) -> u32 {
        match self {
            Self::High100k => 100_000,
            Self::Low12k5 => 12_500,
        }
    }

    /// PIO 状态机统一目标主频 (Hz) (1 bit = 20 PIO cycles)
    #[inline]
    pub const fn pio_target_hz(self) -> u32 {
        match self {
            Self::High100k => 2_000_000,
            Self::Low12k5 => 250_000,
        }
    }
}

/// 计算 RP2040/RP2350 PIO 8 位小数分频比的定点数表示 (整数部分 16 位，小数部分 8 位)
///
/// 返回值高 16 位为整数部分 (int)，低 8 位为小数部分 (frac, 0~255)
pub fn calculate_clkdiv_fixed(sys_clk_hz: u32, speed: ArincSpeed) -> (u16, u8) {
    let target_hz = speed.pio_target_hz() as u64;
    let sys_clk = sys_clk_hz as u64;

    // RP2040/RP2350 PIO clkdiv 格式：int (16-bit) + frac (8-bit)
    // 8 位定点：div = (sys_clk * 256 + target_hz / 2) / target_hz (带四舍五入舍入优化)
    let div_scaled = (sys_clk * 256 + target_hz / 2) / target_hz;
    let int_part = (div_scaled >> 8) as u16;
    let frac_part = (div_scaled & 0xFF) as u8;

    (int_part, frac_part)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 评估分频误差是否符合 ARINC 429 规范 (±1.0%)，用于测试与离线验证
    fn evaluate_baud_accuracy(sys_clk_hz: u32, speed: ArincSpeed) -> (f64, f64) {
        let (int_part, frac_part) = calculate_clkdiv_fixed(sys_clk_hz, speed);
        let div_val = (int_part as f64) + ((frac_part as f64) / 256.0);
        let actual_pio_hz = (sys_clk_hz as f64) / div_val;
        let actual_baud = actual_pio_hz / 20.0;
        let target_baud = speed.baud_rate() as f64;
        let error_pct = ((actual_baud - target_baud) / target_baud) * 100.0;

        (actual_baud, error_pct)
    }

    #[test]
    fn test_calculate_clkdiv_standard_clocks() {
        // 常见树莓派系统时钟频率：125 MHz, 133 MHz, 150 MHz
        let clks = [125_000_000, 133_000_000, 150_000_000];

        for &sys_clk in &clks {
            // 测试 100k 高速 (统一 20 周期/bit)
            let (actual_100k, err_100k) = evaluate_baud_accuracy(sys_clk, ArincSpeed::High100k);
            assert!(
                err_100k.abs() < 1.0,
                "100k baud error for sys_clk {} is {:.3}%, exceeding 1%",
                sys_clk,
                err_100k
            );
            assert!((actual_100k - 100_000.0).abs() < 1000.0);

            // 测试 12.5k 低速 (统一 20 周期/bit)
            let (actual_12k5, err_12k5) = evaluate_baud_accuracy(sys_clk, ArincSpeed::Low12k5);
            assert!(
                err_12k5.abs() < 1.0,
                "12.5k baud error for sys_clk {} is {:.3}%, exceeding 1%",
                sys_clk,
                err_12k5
            );
            assert!((actual_12k5 - 12_500.0).abs() < 125.0);
        }
    }
}

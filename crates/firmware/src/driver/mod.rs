pub mod rx;
pub mod tx;

pub use arinc429_core::baud::ArincSpeed;
pub use rx::Arinc429Rx;
pub use tx::Arinc429Tx;

use arinc429_core::baud::calculate_clkdiv_fixed;
use fixed::FixedU32;

/// 计算统一 PIO 状态机时钟分频比 (统一 1 bit = 20 PIO cycles，带 16.8 定点数四舍五入优化)
pub(crate) fn calculate_pio_clkdiv(
    sys_clk: u32,
    speed: ArincSpeed,
) -> FixedU32<fixed::types::extra::U8> {
    let (int_part, frac_part) = calculate_clkdiv_fixed(sys_clk, speed);
    let raw_bits = ((int_part as u32) << 8) | (frac_part as u32);
    FixedU32::from_bits(raw_bits)
}

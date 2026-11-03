//! ARINC 429 奇偶校验模块
//!
//! ARINC 429 规范规定：第 32 位为奇偶校验位（Parity bit），
//! 默认为奇校验（Odd Parity），即整个 32 位字中逻辑 '1' 的总数必须为奇数。

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum ParityMode {
    /// 标准 ARINC 429 奇校验：整个 32 位包含奇数个 1
    Odd,
    /// 偶校验：整个 32 位包含偶数个 1 (少数专用/定制系统使用)
    Even,
    /// 无校验/保留原样 (用于故障注入测试或透传)
    None,
}

/// 计算并返回完整的 32 位字（将指定奇偶校验位填入 bit 31 / ARINC 429 bit 32）
#[inline]
pub fn apply_parity(word_without_parity: u32, mode: ParityMode) -> u32 {
    let base = word_without_parity & 0x7FFF_FFFF;
    let ones = base.count_ones();
    match mode {
        ParityMode::Odd => {
            // 如果前 31 位中有偶数个 1，则第 32 位置 1，使其变为奇数个 1
            if (ones & 1) == 0 {
                base | 0x8000_0000
            } else {
                base
            }
        }
        ParityMode::Even => {
            // 如果前 31 位中有奇数个 1，则第 32 位置 1，使其变为偶数个 1
            if (ones & 1) == 1 {
                base | 0x8000_0000
            } else {
                base
            }
        }
        ParityMode::None => word_without_parity,
    }
}

/// 校验一个 32 位 ARINC 字的奇偶校验位是否符合给定模式
#[inline]
pub fn check_parity(word: u32, mode: ParityMode) -> bool {
    let ones = word.count_ones();
    match mode {
        ParityMode::Odd => (ones & 1) == 1,
        ParityMode::Even => (ones & 1) == 0,
        ParityMode::None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_odd_parity_calculation() {
        // 全 0 (0 个 1，偶数) -> 加奇校验后应该在最高位置 1 (1 个 1，奇数)
        let w = apply_parity(0x0000_0000, ParityMode::Odd);
        assert_eq!(w, 0x8000_0000);
        assert!(check_parity(w, ParityMode::Odd));

        // 只有 1 个 1 (奇数) -> 加奇校验后最高位应为 0
        let w2 = apply_parity(0x0000_0001, ParityMode::Odd);
        assert_eq!(w2, 0x0000_0001);
        assert!(check_parity(w2, ParityMode::Odd));

        // 31 位全 1 (31 个 1，奇数) -> 加奇校验后最高位应为 0
        let w3 = apply_parity(0x7FFF_FFFF, ParityMode::Odd);
        assert_eq!(w3, 0x7FFF_FFFF);
        assert!(check_parity(w3, ParityMode::Odd));

        // 2 个 1 -> 加奇校验后最高位置 1
        let w4 = apply_parity(0x0000_0003, ParityMode::Odd);
        assert_eq!(w4, 0x8000_0003);
        assert!(check_parity(w4, ParityMode::Odd));
    }

    #[test]
    fn test_parity_error_detection() {
        // 正确的奇校验字
        let valid = apply_parity(0x1234_5678, ParityMode::Odd);
        assert!(check_parity(valid, ParityMode::Odd));

        // 翻转其中任意 1 bit 模拟单比特错误
        let corrupted = valid ^ 0x0000_0004;
        assert!(!check_parity(corrupted, ParityMode::Odd));
    }
}

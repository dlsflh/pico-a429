//! ARINC 429 奇偶校验模块 (no_std)
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

/// 根据指定的奇偶校验模式为低 31 位字设置第 32 位校验位 (bit 31)
#[inline]
pub fn apply_parity(word31: u32, mode: ParityMode) -> u32 {
    let base = word31 & 0x7FFF_FFFF;
    let ones = base.count_ones();
    match mode {
        ParityMode::Odd => {
            if (ones & 1) == 0 {
                base | 0x8000_0000
            } else {
                base
            }
        }
        ParityMode::Even => {
            if (ones & 1) == 1 {
                base | 0x8000_0000
            } else {
                base
            }
        }
        ParityMode::None => word31,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parity_error_detection() {
        // 0xE048D013 包含 15 个 1 (有效奇校验字)
        let valid = 0xE048D013u32;
        assert!(check_parity(valid, ParityMode::Odd));

        // 翻转其中任意 1 bit 模拟单比特错误，必须被立刻检出
        let corrupted = valid ^ 0x0000_0004;
        assert!(!check_parity(corrupted, ParityMode::Odd));

        // 翻转后应为偶校验
        assert!(check_parity(corrupted, ParityMode::Even));
    }
}

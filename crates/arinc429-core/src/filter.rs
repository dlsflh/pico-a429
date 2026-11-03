//! ARINC 429 接收过滤模块 (no_std)
//!
//! 提供基于 256-bit 位图的高性能 O(1) Label 过滤引擎。
//! 内存开销仅 32 字节，单周期无分支位运算完成命中判断。

use crate::label::Label;

/// 过滤引擎错误枚举
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum FilterError {
    /// 模式冲突错误：尝试在不兼容的模式下添加规则（例如在黑名单模式下调用允许白名单规则）
    ModeMismatch {
        current_mode: FilterMode,
        attempted_rule: &'static str,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum FilterMode {
    /// 全放行模式：接收所有到达的有效帧 (位图被忽略)
    AcceptAll,
    /// 白名单模式：仅接收位图中允许的 Label
    Whitelist,
    /// 黑名单模式：丢弃位图中指定的 Label，接收其余全部
    Blacklist,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct LabelFilter {
    /// 256 位掩码，每个 bit 对应一个八进制 Label (0~255)
    bitmap: [u32; 8],
    /// 当前过滤模式
    mode: FilterMode,
}

impl Default for LabelFilter {
    fn default() -> Self {
        Self::new_accept_all()
    }
}

impl LabelFilter {
    /// 创建全放行过滤器
    pub const fn new_accept_all() -> Self {
        Self {
            bitmap: [0; 8],
            mode: FilterMode::AcceptAll,
        }
    }

    /// 创建白名单过滤器（默认全阻断，需通过 `allow_label` 添加）
    pub const fn new_whitelist() -> Self {
        Self {
            bitmap: [0; 8],
            mode: FilterMode::Whitelist,
        }
    }

    /// 创建黑名单过滤器（默认全放行，需通过 `block_label` 添加黑名单）
    pub const fn new_blacklist() -> Self {
        Self {
            bitmap: [0; 8],
            mode: FilterMode::Blacklist,
        }
    }

    /// 获取当前模式
    pub fn mode(&self) -> FilterMode {
        self.mode
    }

    /// 允许指定八进制 Label
    pub fn allow_label(&mut self, label: Label) -> Result<(), FilterError> {
        match self.mode {
            FilterMode::AcceptAll => {
                self.bitmap = [0; 8];
                self.mode = FilterMode::Whitelist;
                self.add_label_rule(label);
                Ok(())
            }
            FilterMode::Whitelist => {
                self.add_label_rule(label);
                Ok(())
            }
            FilterMode::Blacklist => Err(FilterError::ModeMismatch {
                current_mode: FilterMode::Blacklist,
                attempted_rule: "allow_label",
            }),
        }
    }

    /// 封锁指定八进制 Label
    pub fn block_label(&mut self, label: Label) -> Result<(), FilterError> {
        match self.mode {
            FilterMode::AcceptAll => {
                self.bitmap = [0; 8];
                self.mode = FilterMode::Blacklist;
                self.add_label_rule(label);
                Ok(())
            }
            FilterMode::Blacklist => {
                self.add_label_rule(label);
                Ok(())
            }
            FilterMode::Whitelist => Err(FilterError::ModeMismatch {
                current_mode: FilterMode::Whitelist,
                attempted_rule: "block_label",
            }),
        }
    }

    /// 直接向位图中置位 Label 规则 (不改变当前 mode)
    pub fn add_label_rule(&mut self, label: Label) {
        let octal = label.to_octal();
        let word_idx = (octal / 32) as usize;
        let bit_idx = octal % 32;
        self.bitmap[word_idx] |= 1 << bit_idx;
    }

    /// 查询特定 Label 是否在当前规则位图中
    pub fn contains_label_rule(&self, label: Label) -> bool {
        let octal = label.to_octal();
        let word_idx = (octal / 32) as usize;
        let bit_idx = octal % 32;
        (self.bitmap[word_idx] & (1 << bit_idx)) != 0
    }

    /// 完全重置为默认出厂全放行模式 (AcceptAll, 规则清零)
    pub fn reset(&mut self) {
        self.bitmap = [0; 8];
        self.mode = FilterMode::AcceptAll;
    }

    /// 清空配置并重置为 AcceptAll
    pub fn clear(&mut self) {
        self.reset();
    }

    /// 高性能判断一个到达的 (label, parity_valid) 帧是否应被接收
    ///
    /// 注意：Label 过滤器专注于八进制标号路由，物理奇偶校验错误字在全放行与白名单命中时均允许透传至上位机分析仪以供监控告警
    #[inline(always)]
    pub fn matches(&self, label: Label, _parity_valid: bool) -> bool {
        match self.mode {
            FilterMode::AcceptAll => true,
            FilterMode::Whitelist => {
                let octal = label.to_octal();
                let word_idx = (octal / 32) as usize;
                let bit_idx = octal % 32;
                (self.bitmap[word_idx] & (1 << bit_idx)) != 0
            }
            FilterMode::Blacklist => {
                let octal = label.to_octal();
                let word_idx = (octal / 32) as usize;
                let bit_idx = octal % 32;
                (self.bitmap[word_idx] & (1 << bit_idx)) == 0
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_accept_all_filter() {
        let filter = LabelFilter::new_accept_all();
        assert!(filter.matches(Label::from_octal(0o310), true));
        assert!(filter.matches(Label::from_octal(0o203), true));
        // 全放行模式下允许透传校验错误帧，供上位机监控捕获并警报
        assert!(filter.matches(Label::from_octal(0o310), false));
    }

    #[test]
    fn test_whitelist_filter() {
        let mut filter = LabelFilter::new_whitelist();
        filter.allow_label(Label::from_octal(0o310)).unwrap();
        filter.allow_label(Label::from_octal(0o203)).unwrap();

        assert!(filter.matches(Label::from_octal(0o310), true));
        assert!(filter.matches(Label::from_octal(0o203), true));
        assert!(!filter.matches(Label::from_octal(0o100), true)); // 未在白名单中
    }

    #[test]
    fn test_blacklist_filter() {
        let mut filter = LabelFilter::new_blacklist();
        filter.block_label(Label::from_octal(0o377)).unwrap(); // 屏蔽高频广播

        assert!(filter.matches(Label::from_octal(0o310), true));
        assert!(!filter.matches(Label::from_octal(0o377), true)); // 被黑名单过滤
    }

    #[test]
    fn test_mode_transitions_and_fail_fast() {
        let mut filter = LabelFilter::new_accept_all();
        assert_eq!(filter.mode(), FilterMode::AcceptAll);

        filter.allow_label(Label::from_octal(0o310)).unwrap();
        assert_eq!(filter.mode(), FilterMode::Whitelist);
        assert!(filter.matches(Label::from_octal(0o310), true));
        assert!(!filter.matches(Label::from_octal(0o203), true));

        // 在 Whitelist 模式下调用 block_label 拒绝静默清空规则，必须显式报错
        assert!(filter.block_label(Label::from_octal(0o203)).is_err());
        assert!(filter.matches(Label::from_octal(0o310), true));

        filter.reset();
        assert_eq!(filter.mode(), FilterMode::AcceptAll);
        filter.block_label(Label::from_octal(0o203)).unwrap();
        assert_eq!(filter.mode(), FilterMode::Blacklist);
        assert!(filter.matches(Label::from_octal(0o310), true));
        assert!(!filter.matches(Label::from_octal(0o203), true));

        // 在 Blacklist 模式下调用 allow_label 同样必须显式报错
        assert!(filter.allow_label(Label::from_octal(0o310)).is_err());
        assert!(!filter.matches(Label::from_octal(0o203), true));
    }
}

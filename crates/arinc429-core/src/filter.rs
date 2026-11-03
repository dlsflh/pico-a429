//! ARINC 429 接收过滤模块
//!
//! 提供基于 256-bit 位图的高性能 O(1) Label 过滤引擎。
//! 内存开销仅 32 字节，单周期无分支位运算完成命中判断。

use crate::label::Label;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum FilterMode {
    /// 全放行模式：接收所有到达的有效帧
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
    /// 可选 SDI 过滤掩码 (低 4 位分别代表 SDI 0, 1, 2, 3 是否允许)
    /// None 表示不检查 SDI
    sdi_mask: Option<u8>,
    /// 是否过滤奇偶校验错误的帧 (true: 自动丢弃坏帧; false: 保留并上报错误标志)
    drop_parity_errors: bool,
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
            sdi_mask: None,
            drop_parity_errors: true,
        }
    }

    /// 创建白名单过滤器（默认全阻断，需通过 `allow_label` 添加）
    pub const fn new_whitelist() -> Self {
        Self {
            bitmap: [0; 8],
            mode: FilterMode::Whitelist,
            sdi_mask: None,
            drop_parity_errors: true,
        }
    }

    /// 创建黑名单过滤器（默认全放行，需通过 `block_label` 添加黑名单）
    pub const fn new_blacklist() -> Self {
        Self {
            bitmap: [0; 8],
            mode: FilterMode::Blacklist,
            sdi_mask: None,
            drop_parity_errors: true,
        }
    }

    /// 设置过滤模式
    pub fn set_mode(&mut self, mode: FilterMode) {
        self.mode = mode;
    }

    /// 获取当前模式
    pub fn mode(&self) -> FilterMode {
        self.mode
    }

    /// 设置是否丢弃奇偶校验错误的帧
    pub fn set_drop_parity_errors(&mut self, drop: bool) {
        self.drop_parity_errors = drop;
    }

    /// 是否丢弃奇偶校验错误帧
    pub fn drop_parity_errors(&self) -> bool {
        self.drop_parity_errors
    }

    /// 允许指定八进制 Label (若当前不为白名单模式，则自动切换为 Whitelist 并清空原有规则)
    pub fn allow_label(&mut self, label: Label) {
        if self.mode != FilterMode::Whitelist {
            self.bitmap = [0; 8];
            self.mode = FilterMode::Whitelist;
        }
        let octal = label.to_octal();
        let word_idx = (octal / 32) as usize;
        let bit_idx = octal % 32;
        self.bitmap[word_idx] |= 1 << bit_idx;
    }

    /// 封锁指定八进制 Label (若当前不为黑名单模式，则自动切换为 Blacklist 并清空原有规则)
    pub fn block_label(&mut self, label: Label) {
        if self.mode != FilterMode::Blacklist {
            self.bitmap = [0; 8];
            self.mode = FilterMode::Blacklist;
        }
        let octal = label.to_octal();
        let word_idx = (octal / 32) as usize;
        let bit_idx = octal % 32;
        self.bitmap[word_idx] |= 1 << bit_idx;
    }

    /// 清空所有位图配置并重置为全放行模式 (AcceptAll)
    pub fn clear(&mut self) {
        self.bitmap = [0; 8];
        self.mode = FilterMode::AcceptAll;
    }

    /// 配置 SDI 过滤掩码 (例如 0b0001 仅允许 SDI 0，0b0011 允许 SDI 0 和 1)
    pub fn set_sdi_mask(&mut self, mask: Option<u8>) {
        self.sdi_mask = mask.map(|m| m & 0x0F);
    }

    /// 高性能判断一个到达的 (label, sdi, parity_valid) 帧是否应被接收
    #[inline(always)]
    pub fn matches(&self, label: Label, sdi: u8, parity_valid: bool) -> bool {
        // 1. 奇偶校验检查
        if self.drop_parity_errors && !parity_valid {
            return false;
        }

        // 2. SDI 检查
        if let Some(mask) = self.sdi_mask {
            let sdi_bit = 1 << (sdi & 0x03);
            if (mask & sdi_bit) == 0 {
                return false;
            }
        }

        // 3. Label 位图快速匹配
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
                // 如果位图对应位为 1，说明在黑名单中，应拒绝
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
        assert!(filter.matches(Label::from_octal(0o310), 0, true));
        assert!(filter.matches(Label::from_octal(0o203), 1, true));
        // 校验错误被拦截
        assert!(!filter.matches(Label::from_octal(0o310), 0, false));
    }

    #[test]
    fn test_whitelist_filter() {
        let mut filter = LabelFilter::new_whitelist();
        filter.allow_label(Label::from_octal(0o310));
        filter.allow_label(Label::from_octal(0o203));

        assert!(filter.matches(Label::from_octal(0o310), 0, true));
        assert!(filter.matches(Label::from_octal(0o203), 0, true));
        assert!(!filter.matches(Label::from_octal(0o100), 0, true)); // 未在白名单中
    }

    #[test]
    fn test_blacklist_filter() {
        let mut filter = LabelFilter::new_blacklist();
        filter.block_label(Label::from_octal(0o377)); // 屏蔽高频广播

        assert!(filter.matches(Label::from_octal(0o310), 0, true));
        assert!(!filter.matches(Label::from_octal(0o377), 0, true)); // 被黑名单过滤
    }

    #[test]
    fn test_sdi_filtering() {
        let mut filter = LabelFilter::new_accept_all();
        // 仅允许 SDI 1 (系统 1)
        filter.set_sdi_mask(Some(0b0010)); // bit 1 is 1

        assert!(!filter.matches(Label::from_octal(0o310), 0, true));
        assert!(filter.matches(Label::from_octal(0o310), 1, true));
        assert!(!filter.matches(Label::from_octal(0o310), 2, true));
    }

    #[test]
    fn test_mode_transitions() {
        let mut filter = LabelFilter::new_accept_all();
        assert_eq!(filter.mode(), FilterMode::AcceptAll);

        // allow_label transitions mode to Whitelist
        filter.allow_label(Label::from_octal(0o310));
        assert_eq!(filter.mode(), FilterMode::Whitelist);
        assert!(filter.matches(Label::from_octal(0o310), 0, true));
        assert!(!filter.matches(Label::from_octal(0o203), 0, true));

        // clear resets to AcceptAll
        filter.clear();
        assert_eq!(filter.mode(), FilterMode::AcceptAll);
        assert!(filter.matches(Label::from_octal(0o310), 0, true));
        assert!(filter.matches(Label::from_octal(0o203), 0, true));

        // block_label transitions mode to Blacklist
        filter.block_label(Label::from_octal(0o203));
        assert_eq!(filter.mode(), FilterMode::Blacklist);
        assert!(filter.matches(Label::from_octal(0o310), 0, true));
        assert!(!filter.matches(Label::from_octal(0o203), 0, true));

        // clear resets to AcceptAll again
        filter.clear();
        assert_eq!(filter.mode(), FilterMode::AcceptAll);
        assert!(filter.matches(Label::from_octal(0o203), 0, true));
    }
}

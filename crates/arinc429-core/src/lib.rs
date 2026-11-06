//! ARINC 429 协议栈核心库 (no_std)
//!
//! 包含 ARINC 429 的字定义、奇偶校验、八进制 Label 变换、256 位高性能位图过滤、
//! BNR/BCD 编解码与上位机二进制/文本遥测协议。

#![cfg_attr(not(test), no_std)]

pub mod baud;
pub mod filter;
pub mod label;
pub mod parity;
pub mod protocol;
pub mod word;

// 常用类型直接顶层重导出
pub use baud::{calculate_clkdiv_fixed, ArincSpeed};
pub use filter::{FilterError, FilterMode, LabelFilter};
pub use label::Label;
pub use parity::{apply_parity, check_parity, ParityMode};
pub use protocol::{
    ack_status, find_frame_in_stream, flags, frame_types, opcodes, pack_12_channels,
    profile_sub_ops, unpack_12_channels, BinaryFrame, DecodeError, FilterAction, BINARY_FRAME_LEN,
    BINARY_MAGIC,
};
pub use word::{ArincWord, WordBuildError};

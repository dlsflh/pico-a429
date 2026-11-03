//! ARINC 429 协议栈核心库 (no_std)
//!
//! 包含 ARINC 429 的字定义、奇偶校验、八进制 Label 变换、256 位高性能位图过滤、
//! BNR/BCD 编解码与上位机二进制/文本遥测协议。

#![cfg_attr(not(test), no_std)]

pub mod baud;
pub mod bit;
pub mod bnr_bcd;
pub mod filter;
pub mod label;
pub mod parity;
pub mod protocol;
pub mod word;

// 常用类型直接顶层重导出
pub use baud::{calculate_clkdiv_fixed, evaluate_baud_accuracy, ArincSpeed};
pub use bit::{
    evaluate_bit_results, generate_bit_vectors, BitErrorCode, BitReport, BitStatus, BitVector,
};
pub use bnr_bcd::{decode_bcd_digits, decode_bnr_to_int, encode_bnr_from_int};
pub use filter::{FilterMode, LabelFilter};
pub use label::Label;
pub use parity::{apply_parity, check_parity, ParityMode};
pub use protocol::{BinaryFrame, HostCommand, BINARY_FRAME_LEN, BINARY_MAGIC};
pub use word::{ArincWord, ArincWordBuilder, SsmBcd, SsmBnr};

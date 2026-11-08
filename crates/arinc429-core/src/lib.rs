//! ARINC 429 航空电子数据总线核心协议库 (no_std)

#![no_std]

pub mod baud;
pub mod filter;
pub mod label;
pub mod parity;
pub mod protocol;
pub mod word;

pub use baud::{calculate_clkdiv_fixed, ArincSpeed};
pub use filter::{FilterError, FilterMode, LabelFilter};
pub use label::Label;
pub use parity::{check_parity, ParityMode};
pub use protocol::{
    ack_status, flags, frame_types, opcodes, profile_sub_ops, BinaryFrame, DecodeError,
    FilterAction, ProfileRecord, BINARY_FRAME_LEN, BINARY_MAGIC, PROFILE_MAGIC, PROFILE_RECORD_LEN,
};
pub use word::ArincWord;

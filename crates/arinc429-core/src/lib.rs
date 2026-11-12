//! ARINC 429 航空电子数据总线核心协议库 (no_std)

#![no_std]

pub mod baud;
pub mod filter;
pub mod label;
pub mod parity;
pub mod protocol;
pub mod word;

pub use baud::{ArincSpeed, calculate_clkdiv_fixed};
pub use filter::{FilterError, FilterMode, LabelFilter};
pub use label::Label;
pub use parity::{ParityMode, check_parity};
pub use protocol::{
    BINARY_FRAME_LEN, BINARY_MAGIC, BinaryFrame, DecodeError, FilterAction, PROFILE_MAGIC,
    PROFILE_RECORD_LEN, ProfileRecord, ack_status, flags, frame_types, opcodes, profile_sub_ops,
};
pub use word::ArincWord;

//! 通道抽象与多通道扩展路由器 (Channel Router)
//!
//! 支持从单路 (1 TX, 1 RX) 平滑扩展至多路 (N TX, N RX)。
//! 所有通道在统一的消息总线上打上 channel_id 汇聚与分发。

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;

use arinc429_core::protocol::{flags, BinaryFrame};
use arinc429_core::word::ArincWord;

pub const MAX_CHANNELS: usize = 4;

/// 接收到的单帧消息包
#[derive(Debug, Clone, Copy)]
pub struct InboundArincFrame {
    pub channel_id: u8,
    pub timestamp_us: u32,
    pub word: ArincWord,
    pub parity_ok: bool,
    pub high_speed: bool,
}

impl InboundArincFrame {
    /// 转换为上位机通信所需的标准 14 字节二进制遥测帧
    pub fn to_binary_frame(&self) -> BinaryFrame {
        let mut f = 0u8;
        if !self.parity_ok {
            f |= flags::PARITY_ERROR;
        }
        if self.high_speed {
            f |= flags::SPEED_100K;
        }

        BinaryFrame::new(self.channel_id, f, self.timestamp_us, self.word.to_raw())
    }
}

/// 下发待发送的消息包
#[derive(Debug, Clone, Copy)]
pub struct OutboundArincFrame {
    pub channel_id: u8,
    pub raw_word: u32,
}

/// 集中式多路接收通道缓冲区 (容量 64 帧，防止突发流量丢失)
pub static GLOBAL_RX_CHANNEL: Channel<CriticalSectionRawMutex, InboundArincFrame, 64> =
    Channel::new();

/// 集中式多路发送通道缓冲区
pub static GLOBAL_TX_CHANNEL: Channel<CriticalSectionRawMutex, OutboundArincFrame, 32> =
    Channel::new();

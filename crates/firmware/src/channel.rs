//! 通道抽象与多通道扩展路由器 (Channel Router)
//!
//! 支持从单路 (1 TX, 1 RX) 平滑扩展至多路 (N TX, N RX)。
//! 所有通道在统一的消息总线上打上 channel_id 汇聚与分发。
//! 采用非阻塞广播发布-订阅 (Pub-Sub) 架构，彻底消除多消费者竞争偷帧与反压挂死。

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::pubsub::PubSubChannel;

use arinc429_core::label::Label;
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

/// 通道动态配置与控制命令
#[derive(Debug, Clone, Copy)]
pub enum ChannelControlCmd {
    SetSpeed { high_speed: bool },
    FilterAllow(Label),
    FilterBlock(Label),
    FilterClear,
}

/// 集中式多路接收遥测广播总线
///
/// 具备 128 帧深度的环形缓冲，支持 3 个独立订阅端 (USB, Wi-Fi, 诊断)，2 个发布源。
/// 满载自动丢弃最旧帧，绝不反向阻塞物理层总线接收。
pub static GLOBAL_RX_PUBSUB: PubSubChannel<CriticalSectionRawMutex, InboundArincFrame, 128, 3, 2> =
    PubSubChannel::new();

/// 通道 0 待发数据队列 (容量 32 帧)
pub static CHANNEL0_TX_QUEUE: Channel<CriticalSectionRawMutex, OutboundArincFrame, 32> =
    Channel::new();

/// 通道 0 控制与配置命令队列 (容量 8)
pub static CHANNEL0_CTRL_QUEUE: Channel<CriticalSectionRawMutex, ChannelControlCmd, 8> =
    Channel::new();

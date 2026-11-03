//! 12 通道通用路由器与消息总线架构 (12-Channel Router)
//!
//! 支持从单通道平滑扩展至 12 路任意全双工/半双工通道（任何一路均可配置为 TX 或 RX）。
//! 所有通道在统一的消息总线上打上 channel_id (0~11) 进行汇聚与分发。
//! 采用非阻塞广播发布-订阅 (Pub-Sub) 架构，彻底消除多消费者竞争偷帧与反压挂死。

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::pubsub::PubSubChannel;
use embassy_sync::signal::Signal;

use arinc429_core::bit::BitReport;
use arinc429_core::label::Label;
use arinc429_core::protocol::{flags, BinaryFrame};
use arinc429_core::word::ArincWord;

/// RP2350 具备 3 个 PIO 块 * 4 个状态机 = 最大 12 路物理通道
pub const MAX_CHANNELS: usize = 12;

/// 通道工作模式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelDirection {
    Disabled,
    Tx,
    Rx,
}

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

/// BIT 自测试触发请求
#[derive(Debug, Clone, Copy)]
pub struct BitRequest {
    pub tx_channel: u8,
    pub rx_channel: u8,
}

// 静态队列构造原型
const TX_QUEUE_INIT: Channel<CriticalSectionRawMutex, OutboundArincFrame, 16> = Channel::new();
const CTRL_QUEUE_INIT: Channel<CriticalSectionRawMutex, ChannelControlCmd, 4> = Channel::new();

/// 12 个通道的下行发送数据队列数组 (每个通道 16 帧缓冲)
pub static CHANNEL_TX_QUEUES: [Channel<CriticalSectionRawMutex, OutboundArincFrame, 16>; MAX_CHANNELS] =
    [TX_QUEUE_INIT; MAX_CHANNELS];

/// 12 个通道的控制与配置命令队列数组
pub static CHANNEL_CTRL_QUEUES: [Channel<CriticalSectionRawMutex, ChannelControlCmd, 4>; MAX_CHANNELS] =
    [CTRL_QUEUE_INIT; MAX_CHANNELS];

/// 集中式多路接收遥测广播总线
///
/// 具备 128 帧深度的环形缓冲，支持 4 个独立订阅端 (USB, BIT 调度器, 诊断监听器)，最多 12 个通道发布。
/// 满载自动丢弃最旧帧，绝不反向阻塞物理层总线接收。
pub static GLOBAL_RX_PUBSUB: PubSubChannel<CriticalSectionRawMutex, InboundArincFrame, 128, 4, 12> =
    PubSubChannel::new();

/// BIT 请求投递队列
pub static BIT_REQ_CHANNEL: Channel<CriticalSectionRawMutex, BitRequest, 1> = Channel::new();

/// BIT 结果报告信号
pub static BIT_RESP_SIGNAL: Signal<CriticalSectionRawMutex, BitReport> = Signal::new();

/// 向指定通道投递发送数据帧
pub async fn send_to_channel_tx(channel_id: u8, frame: OutboundArincFrame) -> Result<(), &'static str> {
    if (channel_id as usize) < MAX_CHANNELS {
        CHANNEL_TX_QUEUES[channel_id as usize].send(frame).await;
        Ok(())
    } else {
        Err("Channel ID out of range (0..11)")
    }
}

/// 向指定通道投递控制/过滤命令
pub async fn send_to_channel_ctrl(channel_id: u8, cmd: ChannelControlCmd) -> Result<(), &'static str> {
    if (channel_id as usize) < MAX_CHANNELS {
        CHANNEL_CTRL_QUEUES[channel_id as usize].send(cmd).await;
        Ok(())
    } else {
        Err("Channel ID out of range (0..11)")
    }
}

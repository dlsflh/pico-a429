//! 12 通道通用路由器与消息总线架构 (12-Channel Router)
//!
//! 支持从单通道平滑扩展至 12 路任意物理通道。
//! 每个通道独立拥有独立的下行发送队列与专用控制队列，杜绝跨任务竞争偷帧。
//! 生产数据采用广播式 PubSub (GLOBAL_RX_PUBSUB)，自测试数据采用通道隔离专用点对点通道 (BIT_DIAGNOSTIC_CHANNEL)。

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::pubsub::PubSubChannel;
use embassy_sync::signal::Signal;

use arinc429_core::bit::BitReport;
use arinc429_core::protocol::{flags, BinaryFrame, FilterAction};
use arinc429_core::word::ArincWord;

/// 通道工作模式
#[derive(Debug, Clone, Copy, PartialEq, Eq, defmt::Format)]
pub enum ChannelDirection {
    Disabled,
    Tx,
    Rx,
}

/// 板载物理通道硬件描述符
#[derive(Debug, Clone, Copy)]
pub struct ChannelHwDescriptor {
    pub channel_id: u8,
    pub direction: ChannelDirection,
    pub pin_a: u8,
    pub pin_b: u8,
}

/// 当前板载物理装配激活的通道数 (Channel 0: TX, Channel 1: RX)
pub const ACTIVE_CHANNEL_COUNT: usize = 2;

/// 板载通道描述表：编译期声明通道物理方向与管脚映射
pub const BOARD_CHANNELS: [ChannelHwDescriptor; ACTIVE_CHANNEL_COUNT] = [
    ChannelHwDescriptor {
        channel_id: 0,
        direction: ChannelDirection::Tx,
        pin_a: 0,
        pin_b: 1,
    },
    ChannelHwDescriptor {
        channel_id: 1,
        direction: ChannelDirection::Rx,
        pin_a: 2,
        pin_b: 3,
    },
];

/// 查询板载通道的物理固定方向
#[inline]
pub fn channel_direction(channel_id: u8) -> Option<ChannelDirection> {
    if (channel_id as usize) < ACTIVE_CHANNEL_COUNT {
        Some(BOARD_CHANNELS[channel_id as usize].direction)
    } else {
        None
    }
}

/// 通道操作错误枚举
#[derive(Debug, Clone, Copy, PartialEq, Eq, defmt::Format)]
pub enum ChannelError {
    ChannelOutOfRange(u8),
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
    /// 转换为上位机通信所需的标准 16 字节二进制遥测帧 (完美整除 64 字节 USB 端点)
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

/// 通道动态配置与控制命令
#[derive(Debug, Clone, Copy)]
pub enum ChannelControlCmd {
    /// 动态配置通道使能、方向与波特率
    Configure {
        direction: ChannelDirection,
        high_speed: bool,
    },
    /// 过滤规则动作
    Filter(FilterAction),
    /// 进入/退出 BIT 诊断回环测试模式 (隔离外部广播总线)
    SetBitDiagnosticMode(bool),
}

/// BIT 自测试触发请求
#[derive(Debug, Clone, Copy)]
pub struct BitRequest {
    pub tx_channel: u8,
    pub rx_channel: u8,
}

// 静态队列构造原型
#[allow(clippy::declare_interior_mutable_const)]
const TX_QUEUE_INIT: Channel<CriticalSectionRawMutex, u32, 16> = Channel::new();
#[allow(clippy::declare_interior_mutable_const)]
const CTRL_QUEUE_INIT: Channel<CriticalSectionRawMutex, ChannelControlCmd, 4> = Channel::new();

/// 激活通道的下行发送数据队列数组 (每个通道 16 帧缓冲)
pub static CHANNEL_TX_QUEUES: [Channel<CriticalSectionRawMutex, u32, 16>; ACTIVE_CHANNEL_COUNT] =
    [TX_QUEUE_INIT; ACTIVE_CHANNEL_COUNT];

/// 激活通道的控制与配置命令队列数组 (每个通道独立独享 1 个，零竞态偷帧)
pub static CHANNEL_CTRL_QUEUES: [Channel<CriticalSectionRawMutex, ChannelControlCmd, 4>; ACTIVE_CHANNEL_COUNT] =
    [CTRL_QUEUE_INIT; ACTIVE_CHANNEL_COUNT];

/// 集中式多路接收遥测广播总线
///
/// 具备 128 帧深度的环形缓冲，支持 4 个独立订阅端 (USB, 诊断监听器等)，对应当前激活通道发布者。
/// 满载自动丢弃最旧帧，绝不反向阻塞物理层总线接收。
pub static GLOBAL_RX_PUBSUB: PubSubChannel<CriticalSectionRawMutex, InboundArincFrame, 128, 4, ACTIVE_CHANNEL_COUNT> =
    PubSubChannel::new();

/// BIT 自测试专用点对点无污染接收通道 (与生产全局广播 PubSub 物理隔离)
pub static BIT_DIAGNOSTIC_CHANNEL: Channel<CriticalSectionRawMutex, InboundArincFrame, 16> =
    Channel::new();

/// BIT 请求投递队列
pub static BIT_REQ_CHANNEL: Channel<CriticalSectionRawMutex, BitRequest, 1> = Channel::new();

/// BIT 结果报告信号
pub static BIT_RESP_SIGNAL: Signal<CriticalSectionRawMutex, BitReport> = Signal::new();

/// 向指定通道投递发送数据帧 (Fail-Fast 参数校验)
pub async fn send_to_channel_tx(channel_id: u8, raw_word: u32) -> Result<(), ChannelError> {
    if (channel_id as usize) < ACTIVE_CHANNEL_COUNT {
        CHANNEL_TX_QUEUES[channel_id as usize].send(raw_word).await;
        Ok(())
    } else {
        Err(ChannelError::ChannelOutOfRange(channel_id))
    }
}

/// 向指定通道投递控制/过滤命令 (Fail-Fast 参数校验)
pub async fn send_to_channel_ctrl(channel_id: u8, cmd: ChannelControlCmd) -> Result<(), ChannelError> {
    if (channel_id as usize) < ACTIVE_CHANNEL_COUNT {
        CHANNEL_CTRL_QUEUES[channel_id as usize].send(cmd).await;
        Ok(())
    } else {
        Err(ChannelError::ChannelOutOfRange(channel_id))
    }
}

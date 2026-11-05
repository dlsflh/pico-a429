//! 12 通道通用路由器与消息总线架构 (12-Channel Router)
//!
//! 支持从单通道平滑扩展至 12 路物理通道。
//! 每个通道独立拥有专用下行发送队列与控制队列，杜绝跨任务竞争偷帧。
//! 生产数据采用广播式 PubSub (GLOBAL_RX_PUBSUB)。

use core::sync::atomic::{AtomicBool, AtomicU16, AtomicU8, Ordering};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::pubsub::PubSubChannel;

use arinc429_core::protocol::{flags, frame_types, BinaryFrame, FilterAction};
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

/// 当前板载物理装配激活的通道数 (Channel 0..11 共 12 路)
pub const ACTIVE_CHANNEL_COUNT: usize = 12;

/// 板载通道描述表：12 通道物理引脚映射 (GP0..GP23)
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
    ChannelHwDescriptor {
        channel_id: 2,
        direction: ChannelDirection::Rx,
        pin_a: 4,
        pin_b: 5,
    },
    ChannelHwDescriptor {
        channel_id: 3,
        direction: ChannelDirection::Rx,
        pin_a: 6,
        pin_b: 7,
    },
    ChannelHwDescriptor {
        channel_id: 4,
        direction: ChannelDirection::Rx,
        pin_a: 8,
        pin_b: 9,
    },
    ChannelHwDescriptor {
        channel_id: 5,
        direction: ChannelDirection::Rx,
        pin_a: 10,
        pin_b: 11,
    },
    ChannelHwDescriptor {
        channel_id: 6,
        direction: ChannelDirection::Rx,
        pin_a: 12,
        pin_b: 13,
    },
    ChannelHwDescriptor {
        channel_id: 7,
        direction: ChannelDirection::Rx,
        pin_a: 14,
        pin_b: 15,
    },
    ChannelHwDescriptor {
        channel_id: 8,
        direction: ChannelDirection::Rx,
        pin_a: 16,
        pin_b: 17,
    },
    ChannelHwDescriptor {
        channel_id: 9,
        direction: ChannelDirection::Rx,
        pin_a: 18,
        pin_b: 19,
    },
    ChannelHwDescriptor {
        channel_id: 10,
        direction: ChannelDirection::Rx,
        pin_a: 20,
        pin_b: 21,
    },
    ChannelHwDescriptor {
        channel_id: 11,
        direction: ChannelDirection::Rx,
        pin_a: 26,
        pin_b: 27,
    },
];

// 运行时通道方向状态：0: Disabled, 1: Tx, 2: Rx
// 出厂默认状态：CH0 为 Tx, CH1 为 Rx, CH2..11 为被动监听 Rx (电气绝对安全模式)
static RUNTIME_CHANNEL_DIRECTIONS: [AtomicU8; ACTIVE_CHANNEL_COUNT] = [
    AtomicU8::new(1), // CH0 默认为 Tx
    AtomicU8::new(2), // CH1 默认为 Rx
    AtomicU8::new(2), // CH2 默认为 Rx
    AtomicU8::new(2), // CH3 默认为 Rx
    AtomicU8::new(2), // CH4 默认为 Rx
    AtomicU8::new(2), // CH5 默认为 Rx
    AtomicU8::new(2), // CH6 默认为 Rx
    AtomicU8::new(2), // CH7 默认为 Rx
    AtomicU8::new(2), // CH8 默认为 Rx
    AtomicU8::new(2), // CH9 默认为 Rx
    AtomicU8::new(2), // CH10 默认为 Rx
    AtomicU8::new(2), // CH11 默认为 Rx
];

// 运行时通道波特率：true 为 HIGH (100 kbps), false 为 LOW (12.5 kbps)
// 出厂默认状态：全部通道默认为 HIGH (100 kbps)
static RUNTIME_CHANNEL_SPEEDS: [AtomicBool; ACTIVE_CHANNEL_COUNT] = [
    AtomicBool::new(true), // CH0
    AtomicBool::new(true), // CH1
    AtomicBool::new(true), // CH2
    AtomicBool::new(true), // CH3
    AtomicBool::new(true), // CH4
    AtomicBool::new(true), // CH5
    AtomicBool::new(true), // CH6
    AtomicBool::new(true), // CH7
    AtomicBool::new(true), // CH8
    AtomicBool::new(true), // CH9
    AtomicBool::new(true), // CH10
    AtomicBool::new(true), // CH11
];

// 全局遥测数据包单调递增包序号 (溢出自动翻转)
static TELEMETRY_SEQ: AtomicU16 = AtomicU16::new(0);

/// 动态更新通道方向
#[inline]
pub fn set_channel_direction(channel_id: u8, dir: ChannelDirection) {
    if (channel_id as usize) < ACTIVE_CHANNEL_COUNT {
        let code = match dir {
            ChannelDirection::Disabled => 0,
            ChannelDirection::Tx => 1,
            ChannelDirection::Rx => 2,
        };
        RUNTIME_CHANNEL_DIRECTIONS[channel_id as usize].store(code, Ordering::Relaxed);
    }
}

/// 查询板载通道的当前配置方向
#[inline]
pub fn channel_direction(channel_id: u8) -> Option<ChannelDirection> {
    if (channel_id as usize) < ACTIVE_CHANNEL_COUNT {
        match RUNTIME_CHANNEL_DIRECTIONS[channel_id as usize].load(Ordering::Relaxed) {
            0 => Some(ChannelDirection::Disabled),
            1 => Some(ChannelDirection::Tx),
            2 => Some(ChannelDirection::Rx),
            _ => None,
        }
    } else {
        None
    }
}

/// 动态更新通道波特率
#[inline]
pub fn set_channel_speed(channel_id: u8, high_speed: bool) {
    if (channel_id as usize) < ACTIVE_CHANNEL_COUNT {
        RUNTIME_CHANNEL_SPEEDS[channel_id as usize].store(high_speed, Ordering::Relaxed);
    }
}

/// 查询板载通道当前波特率是否为高速
#[inline]
pub fn channel_is_high_speed(channel_id: u8) -> bool {
    if (channel_id as usize) < ACTIVE_CHANNEL_COUNT {
        RUNTIME_CHANNEL_SPEEDS[channel_id as usize].load(Ordering::Relaxed)
    } else {
        true
    }
}

/// 通道操作错误枚举
#[derive(Debug, Clone, Copy, PartialEq, Eq, defmt::Format)]
pub enum ChannelError {
    ChannelOutOfRange(u8),
    NotTxChannel(u8),
    QueueFull(u8),
    ChannelDisabled(u8),
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
    /// 转换为上位机通信所需的标准 16 字节二进制遥测帧 (分配单调自增序号)
    pub fn to_binary_frame(&self) -> BinaryFrame {
        let mut f = 0u8;
        if !self.parity_ok {
            f |= flags::PARITY_ERROR;
        }
        if self.high_speed {
            f |= flags::SPEED_100K;
        }
        let seq = TELEMETRY_SEQ.fetch_add(1, Ordering::Relaxed);

        BinaryFrame::new_full(
            frame_types::ARINC_DATA,
            self.channel_id,
            f,
            seq,
            self.timestamp_us,
            self.word.to_raw(),
        )
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
    /// 暂停物理收发工作 (停止状态机运行)
    Pause,
    /// 恢复物理收发工作 (按当前配置重启状态机)
    Resume,
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
pub static CHANNEL_CTRL_QUEUES: [Channel<CriticalSectionRawMutex, ChannelControlCmd, 4>;
    ACTIVE_CHANNEL_COUNT] = [CTRL_QUEUE_INIT; ACTIVE_CHANNEL_COUNT];

/// 集中式多路接收遥测广播总线
///
/// 具备 128 帧深度的环形缓冲，支持 4 个独立订阅端 (USB 等)，对应当前激活通道发布者。
/// 满载自动丢弃最旧帧，绝不反向阻塞物理层总线接收。
pub static GLOBAL_RX_PUBSUB: PubSubChannel<
    CriticalSectionRawMutex,
    InboundArincFrame,
    128,
    4,
    ACTIVE_CHANNEL_COUNT,
> = PubSubChannel::new();

/// 向指定通道投递发送数据帧 (非阻塞 Fail-Fast，彻底消除 USB 接收端死锁)
pub fn send_to_channel_tx(channel_id: u8, raw_word: u32) -> Result<(), ChannelError> {
    if (channel_id as usize) >= ACTIVE_CHANNEL_COUNT {
        return Err(ChannelError::ChannelOutOfRange(channel_id));
    }
    match channel_direction(channel_id) {
        Some(ChannelDirection::Tx) => {}
        Some(ChannelDirection::Disabled) => return Err(ChannelError::ChannelDisabled(channel_id)),
        _ => return Err(ChannelError::NotTxChannel(channel_id)),
    }
    CHANNEL_TX_QUEUES[channel_id as usize]
        .try_send(raw_word)
        .map_err(|_| ChannelError::QueueFull(channel_id))
}

/// 向指定通道投递控制/过滤命令 (Fail-Fast 参数校验)
pub async fn send_to_channel_ctrl(
    channel_id: u8,
    cmd: ChannelControlCmd,
) -> Result<(), ChannelError> {
    if (channel_id as usize) < ACTIVE_CHANNEL_COUNT {
        CHANNEL_CTRL_QUEUES[channel_id as usize].send(cmd).await;
        Ok(())
    } else {
        Err(ChannelError::ChannelOutOfRange(channel_id))
    }
}

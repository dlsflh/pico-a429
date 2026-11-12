//! 12 通道通用路由器与消息总线架构 (12-Channel Router)
//!
//! 支持从单通道平滑扩展至 12 路物理通道。
//! 每个通道独立拥有专用下行发送队列与控制队列，杜绝跨任务竞争偷帧。
//! 生产数据采用广播式 PubSub (GLOBAL_RX_PUBSUB)。

use core::cell::RefCell;
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU16, AtomicU32, Ordering};
use embassy_sync::blocking_mutex::Mutex as BlockingMutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::pubsub::PubSubChannel;

use arinc429_core::filter::LabelFilter;
use arinc429_core::protocol::{BinaryFrame, FilterAction, ProfileRecord, flags, frame_types};
use arinc429_core::word::ArincWord;

/// 通道工作模式
#[derive(Debug, Clone, Copy, PartialEq, Eq, defmt::Format)]
pub enum ChannelDirection {
    Disabled,
    Tx,
    Rx,
}

/// 当前板载物理装配激活的通道数 (Channel 0..11 共 12 路)
pub const ACTIVE_CHANNEL_COUNT: usize = 12;

// 运行时通道紧凑原子配置状态 (单一真理源，直接复用 4-bit nibble 编码: Enable, IsRx, HighSpeed)
// 出厂默认状态：全部 12 通道默认为 0 (电气绝对安全静默基态，全停用)
static RUNTIME_CHANNEL_CONFIG: [AtomicU8; ACTIVE_CHANNEL_COUNT] = [
    AtomicU8::new(0), // CH0
    AtomicU8::new(0), // CH1
    AtomicU8::new(0), // CH2
    AtomicU8::new(0), // CH3
    AtomicU8::new(0), // CH4
    AtomicU8::new(0), // CH5
    AtomicU8::new(0), // CH6
    AtomicU8::new(0), // CH7
    AtomicU8::new(0), // CH8
    AtomicU8::new(0), // CH9
    AtomicU8::new(0), // CH10
    AtomicU8::new(0), // CH11
];

/// 设备工作模式：Config (配置模式) 与 Operational (正常运行模式)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperatingMode {
    Config = 0,
    Operational = 1,
}

// 运行时工作模式：0: Config, 1: Operational (开机默认处于 Config 维护模式)
static GLOBAL_OPERATING_MODE: AtomicU8 = AtomicU8::new(0);

#[inline]
pub fn operating_mode() -> OperatingMode {
    if GLOBAL_OPERATING_MODE.load(Ordering::Acquire) == 1 {
        OperatingMode::Operational
    } else {
        OperatingMode::Config
    }
}

#[inline]
pub fn set_operating_mode(mode: OperatingMode) {
    GLOBAL_OPERATING_MODE.store(mode as u8, Ordering::Release);
}

#[inline]
pub fn is_config_mode() -> bool {
    operating_mode() == OperatingMode::Config
}

// 运行时硬件/协议故障标志 (置位时 LED 常亮报警)
static HARDWARE_FAULT: AtomicBool = AtomicBool::new(false);

#[inline]
pub fn set_hardware_fault(fault: bool) {
    HARDWARE_FAULT.store(fault, Ordering::Release);
}

#[inline]
pub fn is_hardware_fault() -> bool {
    HARDWARE_FAULT.load(Ordering::Acquire)
}

// 全局遥测数据包单调递增包序号 (溢出自动翻转)
static TELEMETRY_SEQ: AtomicU16 = AtomicU16::new(0);

/// 动态更新通道原子配置
#[inline]
pub fn set_channel_config(channel_id: u8, dir: ChannelDirection, high_speed: bool) {
    if (channel_id as usize) < ACTIVE_CHANNEL_COUNT {
        let (en, is_rx) = match dir {
            ChannelDirection::Disabled => (false, false),
            ChannelDirection::Tx => (true, false),
            ChannelDirection::Rx => (true, true),
        };
        let nibble = arinc429_core::protocol::pack_channel_nibble(en, is_rx, high_speed);
        RUNTIME_CHANNEL_CONFIG[channel_id as usize].store(nibble, Ordering::Relaxed);
    }
}

/// 查询板载通道的当前配置方向
#[inline]
pub fn channel_direction(channel_id: u8) -> Option<ChannelDirection> {
    if (channel_id as usize) < ACTIVE_CHANNEL_COUNT {
        let nibble = RUNTIME_CHANNEL_CONFIG[channel_id as usize].load(Ordering::Relaxed);
        let (en, is_rx, _) = arinc429_core::protocol::unpack_channel_nibble(nibble);
        if !en {
            Some(ChannelDirection::Disabled)
        } else if is_rx {
            Some(ChannelDirection::Rx)
        } else {
            Some(ChannelDirection::Tx)
        }
    } else {
        None
    }
}

/// 查询板载通道当前波特率是否为高速
#[inline]
pub fn channel_is_high_speed(channel_id: u8) -> bool {
    if (channel_id as usize) < ACTIVE_CHANNEL_COUNT {
        let nibble = RUNTIME_CHANNEL_CONFIG[channel_id as usize].load(Ordering::Relaxed);
        let (_, _, high_speed) = arinc429_core::protocol::unpack_channel_nibble(nibble);
        high_speed
    } else {
        true
    }
}

/// 获取当前所有 12 通道方向快照 (0: Disabled, 1: Tx, 2: Rx)
pub fn current_directions() -> [u8; ACTIVE_CHANNEL_COUNT] {
    let mut dirs = [0u8; ACTIVE_CHANNEL_COUNT];
    for i in 0..ACTIVE_CHANNEL_COUNT {
        let nibble = RUNTIME_CHANNEL_CONFIG[i].load(Ordering::Relaxed);
        let (en, is_rx, _) = arinc429_core::protocol::unpack_channel_nibble(nibble);
        dirs[i] = if !en {
            0
        } else if is_rx {
            2
        } else {
            1
        };
    }
    dirs
}

/// 获取当前所有 12 通道波特率快照 (1: High 100k, 0: Low 12.5k)
pub fn current_speeds() -> [u8; ACTIVE_CHANNEL_COUNT] {
    let mut speeds = [0u8; ACTIVE_CHANNEL_COUNT];
    for i in 0..ACTIVE_CHANNEL_COUNT {
        let nibble = RUNTIME_CHANNEL_CONFIG[i].load(Ordering::Relaxed);
        let (_, _, high_speed) = arinc429_core::protocol::unpack_channel_nibble(nibble);
        speeds[i] = if high_speed { 1 } else { 0 };
    }
    speeds
}

/// 零开销将 12 通道运行时配置编码为上位机所需的两个 32 位载荷字 (Payload 0: CH0..7, Payload 1: CH8..11)
pub fn pack_runtime_channels() -> (u32, u32) {
    let mut p0 = 0u32;
    let mut p1 = 0u32;
    for (ch, item) in RUNTIME_CHANNEL_CONFIG.iter().enumerate().take(8) {
        let nibble = (item.load(Ordering::Relaxed) & 0x0F) as u32;
        p0 |= nibble << (ch * 4);
    }
    for (ch, item) in RUNTIME_CHANNEL_CONFIG.iter().enumerate().skip(8) {
        let nibble = (item.load(Ordering::Relaxed) & 0x0F) as u32;
        p1 |= nibble << ((ch - 8) * 4);
    }
    (p0, p1)
}

static RUNTIME_CHANNEL_FILTERS: BlockingMutex<
    CriticalSectionRawMutex,
    RefCell<[LabelFilter; ACTIVE_CHANNEL_COUNT]>,
> = BlockingMutex::new(RefCell::new(
    [LabelFilter::new_accept_all(); ACTIVE_CHANNEL_COUNT],
));

/// 更新单个通道的 Filter 并返回更新后的副本
pub fn update_channel_filter(ch: u8, action: FilterAction) -> LabelFilter {
    let mut updated = LabelFilter::new_accept_all();
    if (ch as usize) < ACTIVE_CHANNEL_COUNT {
        RUNTIME_CHANNEL_FILTERS.lock(|cell| {
            let mut filters = cell.borrow_mut();
            let f = &mut filters[ch as usize];
            match action {
                FilterAction::Allow(label) => {
                    let _ = f.allow_label(label);
                }
                FilterAction::Block(label) => {
                    let _ = f.block_label(label);
                }
                FilterAction::Clear => {
                    *f = LabelFilter::new_accept_all();
                }
            }
            updated = *f;
        });
    }
    updated
}

/// 直接设置通道过滤器快照 (用于加载 Profile)
pub fn set_channel_filter_direct(ch: u8, filter: LabelFilter) {
    if (ch as usize) < ACTIVE_CHANNEL_COUNT {
        RUNTIME_CHANNEL_FILTERS.lock(|cell| {
            cell.borrow_mut()[ch as usize] = filter;
        });
    }
}

/// 获取当前所有 12 通道过滤器快照 (模式数组与 256 位掩码数组)
pub fn get_channel_filters_snapshot()
-> ([u8; ACTIVE_CHANNEL_COUNT], [[u32; 8]; ACTIVE_CHANNEL_COUNT]) {
    let mut modes = [0u8; ACTIVE_CHANNEL_COUNT];
    let mut bitmaps = [[0u32; 8]; ACTIVE_CHANNEL_COUNT];
    RUNTIME_CHANNEL_FILTERS.lock(|cell| {
        let filters = cell.borrow();
        for ch in 0..ACTIVE_CHANNEL_COUNT {
            modes[ch] = filters[ch].mode() as u8;
            bitmaps[ch] = filters[ch].bitmap();
        }
    });
    (modes, bitmaps)
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
    /// 动态配置通道使能、方向与波特率 (及可选 Label 过滤器原子装载)
    Configure {
        direction: ChannelDirection,
        high_speed: bool,
        filter: Option<LabelFilter>,
    },
    /// 过滤规则动作
    Filter(FilterAction),
    /// 全量重置并配置通道 Label 过滤器 (用于独立设置)
    SetFullFilter(LabelFilter),
    /// 暂停物理收发工作 (停止状态机运行)
    Pause,
    /// 恢复物理收发工作 (按当前配置重启状态机)
    Resume,
    /// 立即清空通道发送队列与硬件 FIFO (用于中止发送)
    PurgeTx,
}

// 静态队列构造原型 (扩容为 128 深度，确保 34.56 ms 防断流跑道)
#[allow(clippy::declare_interior_mutable_const)]
const TX_QUEUE_INIT: Channel<CriticalSectionRawMutex, u32, 128> = Channel::new();
#[allow(clippy::declare_interior_mutable_const)]
const CTRL_QUEUE_INIT: Channel<CriticalSectionRawMutex, ChannelControlCmd, 4> = Channel::new();

/// 激活通道的下行发送数据队列数组 (每个通道 128 帧缓冲)
pub static CHANNEL_TX_QUEUES: [Channel<CriticalSectionRawMutex, u32, 128>; ACTIVE_CHANNEL_COUNT] =
    [TX_QUEUE_INIT; ACTIVE_CHANNEL_COUNT];

/// 激活通道的控制与配置命令队列数组 (每个通道独立独享 1 个，零竞态偷帧)
pub static CHANNEL_CTRL_QUEUES: [Channel<CriticalSectionRawMutex, ChannelControlCmd, 4>;
    ACTIVE_CHANNEL_COUNT] = [CTRL_QUEUE_INIT; ACTIVE_CHANNEL_COUNT];

/// 集中式多路接收遥测总线 (Single Producer Multi-Publisher, Single USB Subscriber)
///
/// 具备 4096 帧深度的环形缓冲，支持 1 个专用 USB 订阅端，对应 12 个物理通道发布端。
/// 吸收 245.7 ms 全负荷遥测，消除上位机中断抖动丢帧。
pub static GLOBAL_RX_PUBSUB: PubSubChannel<
    CriticalSectionRawMutex,
    InboundArincFrame,
    4096,
    1,
    ACTIVE_CHANNEL_COUNT,
> = PubSubChannel::new();

/// 物理硬件已消费字数统计 (由 PIO 发送工作任务递增)
pub static CUMULATIVE_CONSUMED_COUNTER: [AtomicU32; ACTIVE_CHANNEL_COUNT] = [
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
];

/// 固件丢弃未进入硬件的字数统计 (队列满、通道未就绪或中止清空)
pub static DROPPED_TX_WORDS: [AtomicU32; ACTIVE_CHANNEL_COUNT] = [
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
];

/// 记录上次发送信用更新时的累积处理计数
pub static LAST_REPORTED_HANDLED: [AtomicU32; ACTIVE_CHANNEL_COUNT] = [
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
];

/// 流量控制信用更新脏位掩码 (bit 0..11 代表 channel 0..11 有信用变动待发送)
pub static CREDIT_DIRTY_MASK: AtomicU16 = AtomicU16::new(0);

/// 触发通道流量控制检查并按需标记脏位
pub fn check_and_mark_credit_dirty(channel_id: u8) {
    if (channel_id as usize) >= ACTIVE_CHANNEL_COUNT {
        return;
    }
    let ch = channel_id as usize;
    let consumed = CUMULATIVE_CONSUMED_COUNTER[ch].load(Ordering::Relaxed);
    let dropped = DROPPED_TX_WORDS[ch].load(Ordering::Relaxed);
    let total_handled = consumed.wrapping_add(dropped);
    let last = LAST_REPORTED_HANDLED[ch].load(Ordering::Relaxed);
    let delta = total_handled.wrapping_sub(last);

    let is_high_speed = channel_is_high_speed(channel_id);
    let threshold = if is_high_speed { 32 } else { 8 };

    if delta >= threshold || CHANNEL_TX_QUEUES[ch].is_empty() {
        CREDIT_DIRTY_MASK.fetch_or(1 << channel_id, Ordering::Release);
    }
}

/// 向指定通道投递发送数据帧 (非阻塞 Fail-Fast，零信用泄漏)
pub fn send_to_channel_tx(channel_id: u8, raw_word: u32) -> Result<(), ChannelError> {
    if (channel_id as usize) >= ACTIVE_CHANNEL_COUNT {
        return Err(ChannelError::ChannelOutOfRange(channel_id));
    }
    match channel_direction(channel_id) {
        Some(ChannelDirection::Tx) => {}
        Some(ChannelDirection::Disabled) => {
            DROPPED_TX_WORDS[channel_id as usize].fetch_add(1, Ordering::Relaxed);
            check_and_mark_credit_dirty(channel_id);
            return Err(ChannelError::ChannelDisabled(channel_id));
        }
        _ => {
            DROPPED_TX_WORDS[channel_id as usize].fetch_add(1, Ordering::Relaxed);
            check_and_mark_credit_dirty(channel_id);
            return Err(ChannelError::NotTxChannel(channel_id));
        }
    }
    if CHANNEL_TX_QUEUES[channel_id as usize]
        .try_send(raw_word)
        .is_err()
    {
        DROPPED_TX_WORDS[channel_id as usize].fetch_add(1, Ordering::Relaxed);
        check_and_mark_credit_dirty(channel_id);
        Err(ChannelError::QueueFull(channel_id))
    } else {
        Ok(())
    }
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

/// 原子暂停全部 12 通道物理收发
pub async fn pause_all_channels() {
    for ch in 0..ACTIVE_CHANNEL_COUNT as u8 {
        let _ = send_to_channel_ctrl(ch, ChannelControlCmd::Pause).await;
    }
}

/// 恢复全部 12 通道物理收发
pub async fn resume_all_channels() {
    for ch in 0..ACTIVE_CHANNEL_COUNT as u8 {
        let _ = send_to_channel_ctrl(ch, ChannelControlCmd::Resume).await;
    }
}

/// 配置单通道方向与速率
pub async fn apply_channel_config(channel: u8, dir: ChannelDirection, high_speed: bool) {
    set_channel_config(channel, dir, high_speed);
    let _ = send_to_channel_ctrl(
        channel,
        ChannelControlCmd::Configure {
            direction: dir,
            high_speed,
            filter: None,
        },
    )
    .await;
}

/// 批量原子应用 12 通道配置 (单通道 1 次入队，零队列饱和风险)
pub async fn apply_all_channels_batch(channels: &[(bool, bool, bool); ACTIVE_CHANNEL_COUNT]) {
    for (ch, &(enable, is_rx, high_speed)) in channels.iter().enumerate().take(ACTIVE_CHANNEL_COUNT)
    {
        let dir = if !enable {
            ChannelDirection::Disabled
        } else if is_rx {
            ChannelDirection::Rx
        } else {
            ChannelDirection::Tx
        };
        set_channel_config(ch as u8, dir, high_speed);
        let _ = send_to_channel_ctrl(
            ch as u8,
            ChannelControlCmd::Configure {
                direction: dir,
                high_speed,
                filter: None,
            },
        )
        .await;
    }
}

/// 全量原子还原配置方案 (包含方向、速率与全量 12 通道 LabelFilter，单通道单指令原子装载)
pub async fn apply_profile(record: &ProfileRecord) {
    for ch in 0..ACTIVE_CHANNEL_COUNT {
        let dir = match record.directions[ch] {
            1 => ChannelDirection::Tx,
            2 => ChannelDirection::Rx,
            _ => ChannelDirection::Disabled,
        };
        let hs = record.speeds[ch] != 0;
        let filter = record.get_channel_filter(ch);
        set_channel_config(ch as u8, dir, hs);
        set_channel_filter_direct(ch as u8, filter);
        let _ = send_to_channel_ctrl(
            ch as u8,
            ChannelControlCmd::Configure {
                direction: dir,
                high_speed: hs,
                filter: Some(filter),
            },
        )
        .await;
    }
}

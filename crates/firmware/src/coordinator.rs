//! 12 通道系统运行协调器 (System Coordinator)
//!
//! 统一协调管理 12 通道物理生命周期、运行模式切换 (OperatingMode)、
//! 模拟前端 (AFE) 以及配置 Profile 的全量原子还原。

use arinc429_core::protocol::ProfileRecord;

use crate::channel::{
    is_config_mode, send_to_channel_ctrl, set_channel_direction, set_channel_filter_direct,
    set_channel_speed, ChannelControlCmd, ChannelDirection, ACTIVE_CHANNEL_COUNT,
};

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
    set_channel_direction(channel, dir);
    set_channel_speed(channel, high_speed);
    let _ = send_to_channel_ctrl(
        channel,
        ChannelControlCmd::Configure {
            direction: dir,
            high_speed,
        },
    )
    .await;
}

/// 批量原子应用 12 通道配置 (暂停 -> 配置 -> 按模式恢复)
pub async fn apply_all_channels_batch(channels: &[(bool, bool, bool); ACTIVE_CHANNEL_COUNT]) {
    pause_all_channels().await;
    for ch in 0..ACTIVE_CHANNEL_COUNT {
        let (enable, is_rx, high_speed) = channels[ch];
        let dir = if !enable {
            ChannelDirection::Disabled
        } else if is_rx {
            ChannelDirection::Rx
        } else {
            ChannelDirection::Tx
        };
        apply_channel_config(ch as u8, dir, high_speed).await;
    }
    if !is_config_mode() {
        resume_all_channels().await;
    }
}

/// 批量原子应用 12 通道方向与速率 (暂停 -> 配置 -> 按模式恢复)
pub async fn apply_all_channels(dirs: &[u8; 12], speeds: &[u8; 12]) {
    pause_all_channels().await;
    for ch in 0..ACTIVE_CHANNEL_COUNT {
        let dir = match dirs[ch] {
            1 => ChannelDirection::Tx,
            2 => ChannelDirection::Rx,
            _ => ChannelDirection::Disabled,
        };
        apply_channel_config(ch as u8, dir, speeds[ch] != 0).await;
    }
    if !is_config_mode() {
        resume_all_channels().await;
    }
}

/// 全量原子还原配置方案 (包含方向、速率与全量 12 通道 LabelFilter)
pub async fn apply_profile(record: &ProfileRecord) {
    pause_all_channels().await;
    for ch in 0..ACTIVE_CHANNEL_COUNT {
        let dir = match record.directions[ch] {
            1 => ChannelDirection::Tx,
            2 => ChannelDirection::Rx,
            _ => ChannelDirection::Disabled,
        };
        let hs = record.speeds[ch] != 0;
        apply_channel_config(ch as u8, dir, hs).await;

        let filter = record.get_channel_filter(ch);
        set_channel_filter_direct(ch as u8, filter);
        let _ = send_to_channel_ctrl(ch as u8, ChannelControlCmd::SetFullFilter(filter)).await;
    }
    if !is_config_mode() {
        resume_all_channels().await;
    }
}

use std::sync::Mutex;
use std::sync::atomic::{AtomicU16, Ordering};
use tokio::sync::Notify;

pub const CHANNEL_COUNT: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelSpeed {
    High100k,
    Low12k5,
}

impl ChannelSpeed {
    pub fn default_credit_ceiling(&self) -> u32 {
        match self {
            Self::High100k => 128,
            Self::Low12k5 => 32,
        }
    }
}

struct ChannelFlowState {
    speed: ChannelSpeed,
    ceiling: u32,
    baseline_consumed: Option<u32>,
    sent_count: u32,
    reported_consumed: u32,
    reported_available_slots: u32,
    aborted: bool,
}

impl Default for ChannelFlowState {
    fn default() -> Self {
        Self {
            speed: ChannelSpeed::High100k,
            ceiling: 128,
            baseline_consumed: None,
            sent_count: 0,
            reported_consumed: 0,
            reported_available_slots: 128,
            aborted: false,
        }
    }
}

impl ChannelFlowState {
    fn usable_credit(&self) -> u32 {
        if self.aborted {
            return 0;
        }
        let baseline = match self.baseline_consumed {
            Some(b) => b,
            None => return self.ceiling.min(self.reported_available_slots),
        };
        let relative_consumed = self.reported_consumed.wrapping_sub(baseline);
        let in_flight = self.sent_count.saturating_sub(relative_consumed);
        let headroom = self.ceiling.saturating_sub(in_flight);
        headroom.min(self.reported_available_slots)
    }

    fn reset_on_abort(&mut self) {
        self.sent_count = 0;
        self.baseline_consumed = None;
        self.aborted = false;
        self.reported_available_slots = self.ceiling;
    }
}

pub struct ChannelScheduler {
    state: Mutex<ChannelFlowState>,
    credit_notify: Notify,
}

impl Default for ChannelScheduler {
    fn default() -> Self {
        Self {
            state: Mutex::new(ChannelFlowState::default()),
            credit_notify: Notify::new(),
        }
    }
}

impl ChannelScheduler {
    pub fn set_speed(&self, speed: ChannelSpeed) {
        let mut lock = self.state.lock().unwrap();
        lock.speed = speed;
        lock.ceiling = speed.default_credit_ceiling();
    }

    pub fn on_credit_update(&self, available_slots: u32, cumulative_handled: u32) {
        let mut lock = self.state.lock().unwrap();
        if lock.baseline_consumed.is_none() {
            lock.baseline_consumed = Some(cumulative_handled);
        }
        lock.reported_consumed = cumulative_handled;
        lock.reported_available_slots = available_slots;
        let usable = lock.usable_credit();
        drop(lock);

        if usable > 0 {
            self.credit_notify.notify_waiters();
        }
    }

    pub async fn acquire_credit(&self) -> Result<(), &'static str> {
        loop {
            {
                let mut lock = self.state.lock().unwrap();
                if lock.aborted {
                    return Err("Channel streaming was aborted");
                }
                if lock.usable_credit() > 0 {
                    lock.sent_count = lock.sent_count.wrapping_add(1);
                    return Ok(());
                }
            }
            self.credit_notify.notified().await;
        }
    }

    pub fn abort(&self) {
        let mut lock = self.state.lock().unwrap();
        lock.reset_on_abort();
        drop(lock);
        self.credit_notify.notify_waiters();
    }

    pub fn snapshot(&self) -> (u32, u32, u32) {
        let lock = self.state.lock().unwrap();
        (
            lock.usable_credit(),
            lock.sent_count,
            lock.reported_consumed,
        )
    }
}

pub struct StreamScheduler {
    channels: [ChannelScheduler; CHANNEL_COUNT],
    seq_counter: AtomicU16,
}

impl Default for StreamScheduler {
    fn default() -> Self {
        Self {
            channels: std::array::from_fn(|_| ChannelScheduler::default()),
            seq_counter: AtomicU16::new(0),
        }
    }
}

impl StreamScheduler {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn next_seq(&self) -> u16 {
        self.seq_counter.fetch_add(1, Ordering::Relaxed)
    }

    pub fn channel(&self, ch: u8) -> &ChannelScheduler {
        let idx = (ch as usize) % CHANNEL_COUNT;
        &self.channels[idx]
    }

    pub fn on_credit_frame(&self, ch: u8, available_slots: u32, cumulative_handled: u32) {
        let idx = (ch as usize) % CHANNEL_COUNT;
        self.channels[idx].on_credit_update(available_slots, cumulative_handled);
    }

    pub fn abort_channel(&self, ch: u8) {
        let idx = (ch as usize) % CHANNEL_COUNT;
        self.channels[idx].abort();
    }
}

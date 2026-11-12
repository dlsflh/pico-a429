use arinc429_core::protocol::{BinaryFrame, flags};
use std::collections::VecDeque;
use std::sync::Mutex;

pub const DEFAULT_RING_BUFFER_CAPACITY: usize = 32_768;
pub const CHANNEL_COUNT: usize = 12;

#[derive(Debug, Default, Clone)]
pub struct ChannelStats {
    pub total_received: u64,
    pub dropped_overwrites: u64,
    pub parity_errors: u64,
    pub gap_errors: u64,
}

struct InnerChannelBuffer {
    buffer: VecDeque<BinaryFrame>,
    capacity: usize,
    stats: ChannelStats,
}

impl InnerChannelBuffer {
    fn new(capacity: usize) -> Self {
        Self {
            buffer: VecDeque::with_capacity(capacity.min(4096)),
            capacity,
            stats: ChannelStats::default(),
        }
    }

    fn push(&mut self, frame: BinaryFrame) {
        self.stats.total_received += 1;
        if (frame.flags & flags::PARITY_ERROR) != 0 {
            self.stats.parity_errors += 1;
        }
        if (frame.flags & flags::GAP_ERROR) != 0 {
            self.stats.gap_errors += 1;
        }

        if self.buffer.len() >= self.capacity {
            self.buffer.pop_front();
            self.stats.dropped_overwrites += 1;
        }
        self.buffer.push_back(frame);
    }

    fn fetch(&self, limit: usize, label_filter: Option<u8>) -> Vec<BinaryFrame> {
        let mut result = Vec::new();
        for frame in self.buffer.iter().rev() {
            if let Some(target_label) = label_filter {
                let bus_label = (frame.raw_word() & 0xFF) as u8;
                let octal = arinc429_core::label::Label::from_raw_bus_byte(bus_label).to_octal();
                if octal != target_label {
                    continue;
                }
            }
            result.push(*frame);
            if result.len() >= limit {
                break;
            }
        }
        result
    }
}

pub struct TelemetryRingBuffers {
    channels: [Mutex<InnerChannelBuffer>; CHANNEL_COUNT],
}

impl Default for TelemetryRingBuffers {
    fn default() -> Self {
        Self::new(DEFAULT_RING_BUFFER_CAPACITY)
    }
}

impl TelemetryRingBuffers {
    pub fn new(capacity: usize) -> Self {
        Self {
            channels: std::array::from_fn(|_| Mutex::new(InnerChannelBuffer::new(capacity))),
        }
    }

    pub fn push(&self, frame: BinaryFrame) {
        let ch = (frame.channel as usize) % CHANNEL_COUNT;
        if let Ok(mut lock) = self.channels[ch].lock() {
            lock.push(frame);
        }
    }

    pub fn fetch(&self, channel: u8, limit: usize, label_filter: Option<u8>) -> Vec<BinaryFrame> {
        let ch = (channel as usize) % CHANNEL_COUNT;
        if let Ok(lock) = self.channels[ch].lock() {
            lock.fetch(limit, label_filter)
        } else {
            Vec::new()
        }
    }

    pub fn stats(&self, channel: u8) -> ChannelStats {
        let ch = (channel as usize) % CHANNEL_COUNT;
        if let Ok(lock) = self.channels[ch].lock() {
            lock.stats.clone()
        } else {
            ChannelStats::default()
        }
    }
}

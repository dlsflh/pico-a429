use arinc429_core::label::Label;
use arinc429_core::protocol::BinaryFrame;
use schemars::JsonSchema;
use serde::Serialize;
use std::sync::atomic::{AtomicU32, Ordering};

pub const CHANNEL_COUNT: usize = 12;

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct CvtLabelValue {
    pub channel: u8,
    pub label_octal: u8,
    pub label_str: String,
    pub raw_word: u32,
    pub data: u32,
    pub sdi: u8,
    pub ssm: u8,
    pub parity_ok: bool,
    pub timestamp_us: u32,
}

struct ChannelCvt {
    words: [AtomicU32; 256],
    timestamps: [AtomicU32; 256],
    valid_mask: [AtomicU32; 8],
}

impl Default for ChannelCvt {
    fn default() -> Self {
        Self {
            words: std::array::from_fn(|_| AtomicU32::new(0)),
            timestamps: std::array::from_fn(|_| AtomicU32::new(0)),
            valid_mask: std::array::from_fn(|_| AtomicU32::new(0)),
        }
    }
}

impl ChannelCvt {
    fn update(&self, label_octal: u8, raw_word: u32, timestamp_us: u32) {
        let idx = label_octal as usize;
        self.words[idx].store(raw_word, Ordering::Release);
        self.timestamps[idx].store(timestamp_us, Ordering::Release);
        let mask_idx = idx / 32;
        let bit = 1u32 << (idx % 32);
        self.valid_mask[mask_idx].fetch_or(bit, Ordering::Relaxed);
    }

    fn get(&self, label_octal: u8) -> Option<(u32, u32)> {
        let idx = label_octal as usize;
        let mask_idx = idx / 32;
        let bit = 1u32 << (idx % 32);
        if (self.valid_mask[mask_idx].load(Ordering::Relaxed) & bit) == 0 {
            return None;
        }
        let raw = self.words[idx].load(Ordering::Acquire);
        let ts = self.timestamps[idx].load(Ordering::Acquire);
        Some((raw, ts))
    }

    fn get_active_labels(&self) -> Vec<u8> {
        let mut labels = Vec::new();
        for mask_idx in 0..8 {
            let mask = self.valid_mask[mask_idx].load(Ordering::Relaxed);
            if mask != 0 {
                for bit in 0..32 {
                    if (mask & (1 << bit)) != 0 {
                        labels.push((mask_idx * 32 + bit) as u8);
                    }
                }
            }
        }
        labels
    }
}

pub struct CurrentValueTable {
    channels: [ChannelCvt; CHANNEL_COUNT],
}

impl Default for CurrentValueTable {
    fn default() -> Self {
        Self {
            channels: std::array::from_fn(|_| ChannelCvt::default()),
        }
    }
}

impl CurrentValueTable {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn update_from_frame(&self, frame: &BinaryFrame) {
        let ch = (frame.channel as usize) % CHANNEL_COUNT;
        let raw = frame.raw_word();
        let bus_label = (raw & 0xFF) as u8;
        let octal = Label::from_raw_bus_byte(bus_label).to_octal();
        self.channels[ch].update(octal, raw, frame.timestamp_us());
    }

    pub fn get_label(&self, channel: u8, label_octal: u8) -> Option<CvtLabelValue> {
        let ch = (channel as usize) % CHANNEL_COUNT;
        let (raw, ts) = self.channels[ch].get(label_octal)?;
        let word = arinc429_core::word::ArincWord::from_raw(raw);
        Some(CvtLabelValue {
            channel,
            label_octal,
            label_str: format!("{:03o}", label_octal),
            raw_word: raw,
            data: word.data(),
            sdi: word.sdi(),
            ssm: word.ssm(),
            parity_ok: word.is_parity_valid(),
            timestamp_us: ts,
        })
    }

    pub fn get_labels(&self, channel: u8, labels: Option<&[u8]>) -> Vec<CvtLabelValue> {
        let ch = (channel as usize) % CHANNEL_COUNT;
        let target_labels: Vec<u8> = match labels {
            Some(list) => list.to_vec(),
            None => self.channels[ch].get_active_labels(),
        };

        target_labels
            .into_iter()
            .filter_map(|lbl| self.get_label(channel, lbl))
            .collect()
    }
}

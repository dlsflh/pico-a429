use anyhow::{Context, Result, anyhow};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, Deserialize)]
pub struct TomlProfileRoot {
    pub profile: ProfileMeta,
    #[serde(default)]
    pub channel: HashMap<String, ChannelConfigEntry>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProfileMeta {
    pub name: String,
    #[serde(default = "default_version")]
    pub version: String,
    pub boot_slot: Option<u8>,
}

fn default_version() -> String {
    "1.0".to_string()
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChannelConfigEntry {
    #[serde(default = "default_direction")]
    pub direction: String,
    #[serde(default = "default_speed")]
    pub speed: String,
    #[serde(default = "default_filter_mode")]
    pub filter_mode: String,
    #[serde(default)]
    pub labels: Vec<String>,
}

fn default_direction() -> String {
    "DISABLED".to_string()
}

fn default_speed() -> String {
    "high".to_string()
}

fn default_filter_mode() -> String {
    "none".to_string()
}

#[derive(Debug, Clone)]
pub struct ParsedProfile {
    pub name: String,
    pub boot_slot: Option<u8>,
    pub channels: [(bool, bool, bool); 12], // (enable, is_rx, is_high_speed)
    pub filters: HashMap<u8, (u8, Vec<u8>)>, // channel -> (action: 0=clear, 1=allow, 2=block, labels)
}

impl ParsedProfile {
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let text = std::fs::read_to_string(path.as_ref())
            .with_context(|| format!("Failed to read profile file {:?}", path.as_ref()))?;
        text.parse()
    }
}

impl std::str::FromStr for ParsedProfile {
    type Err = anyhow::Error;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let root: TomlProfileRoot =
            toml::from_str(text).context("Failed to parse TOML profile syntax")?;

        let mut channels = [(false, false, false); 12];
        let mut filters = HashMap::new();

        for (key, entry) in root.channel {
            let ch_idx: usize = key
                .parse()
                .with_context(|| format!("Invalid channel index '{}'", key))?;
            if ch_idx >= 12 {
                return Err(anyhow!("Channel index {} out of range (0..11)", ch_idx));
            }

            let dir = entry.direction.to_ascii_uppercase();
            let (enable, is_rx) = match dir.as_str() {
                "TX" => (true, false),
                "RX" => (true, true),
                "DISABLED" | "NONE" | "--" => (false, false),
                other => {
                    return Err(anyhow!(
                        "Invalid direction '{}' on channel {}",
                        other,
                        ch_idx
                    ));
                }
            };

            let is_high_speed = match entry.speed.to_ascii_lowercase().as_str() {
                "high" | "100k" | "100" => true,
                "low" | "12.5k" | "12k5" | "12" => false,
                other => return Err(anyhow!("Invalid speed '{}' on channel {}", other, ch_idx)),
            };

            channels[ch_idx] = (enable, is_rx, is_high_speed);

            if is_rx && enable {
                let action = match entry.filter_mode.to_ascii_lowercase().as_str() {
                    "whitelist" | "allow" => 1u8,
                    "blacklist" | "block" => 2u8,
                    "none" | "clear" | "all" => 0u8,
                    other => {
                        return Err(anyhow!(
                            "Invalid filter_mode '{}' on channel {}",
                            other,
                            ch_idx
                        ));
                    }
                };

                let mut labels = Vec::new();
                for s in entry.labels {
                    let octal =
                        u8::from_str_radix(s.trim_start_matches("0o"), 8).with_context(|| {
                            format!("Invalid octal label '{}' on channel {}", s, ch_idx)
                        })?;
                    labels.push(octal);
                }
                filters.insert(ch_idx as u8, (action, labels));
            }
        }

        Ok(ParsedProfile {
            name: root.profile.name,
            boot_slot: root.profile.boot_slot,
            channels,
            filters,
        })
    }
}

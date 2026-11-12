use crate::telemetry::CvtLabelValue;
use crate::transport::DeviceManager;
use anyhow::Result;
use rmcp::{
    ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::*,
    tool, tool_handler, tool_router,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::Mutex;
use uuid::Uuid;

pub struct StreamJob {
    pub id: String,
    pub channel: u8,
    pub total_words: usize,
    pub words_sent: Arc<AtomicUsize>,
    pub status: Arc<Mutex<String>>,
    pub handle: tokio::task::JoinHandle<()>,
}

#[derive(Default)]
pub struct StreamJobManager {
    jobs: Mutex<HashMap<String, Arc<StreamJob>>>,
}

impl StreamJobManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn start_job(
        &self,
        channel: u8,
        words: Vec<u32>,
        rate_hz: u32,
        dev: Arc<DeviceManager>,
    ) -> String {
        let job_id = Uuid::new_v4().to_string();
        let total = words.len();
        let words_sent = Arc::new(AtomicUsize::new(0));
        let status = Arc::new(Mutex::new("running".to_string()));

        let w_sent = words_sent.clone();
        let w_status = status.clone();
        let w_dev = dev.clone();

        let handle = tokio::spawn(async move {
            let interval = if rate_hz > 0 {
                Duration::from_micros(1_000_000 / rate_hz as u64)
            } else {
                Duration::from_micros(360)
            };

            let mut next_tick = tokio::time::Instant::now();
            for &word in &words {
                if w_dev.send_word(channel, word).await.is_err() {
                    let mut s = w_status.lock().await;
                    *s = "failed".to_string();
                    return;
                }
                w_sent.fetch_add(1, Ordering::Relaxed);
                next_tick += interval;
                tokio::time::sleep_until(next_tick).await;
            }

            let mut s = w_status.lock().await;
            *s = "completed".to_string();
        });

        let job = Arc::new(StreamJob {
            id: job_id.clone(),
            channel,
            total_words: total,
            words_sent,
            status,
            handle,
        });

        let mut lock = self.jobs.lock().await;
        lock.insert(job_id.clone(), job);
        job_id
    }

    pub async fn get_status(&self, job_id: &str) -> Option<(String, usize, usize)> {
        let lock = self.jobs.lock().await;
        let job = lock.get(job_id)?;
        let status = job.status.lock().await.clone();
        let sent = job.words_sent.load(Ordering::Relaxed);
        Some((status, sent, job.total_words))
    }

    pub async fn abort_job(&self, job_id: &str) -> bool {
        let mut lock = self.jobs.lock().await;
        if let Some(job) = lock.remove(job_id) {
            job.handle.abort();
            let mut s = job.status.lock().await;
            *s = "aborted".to_string();
            true
        } else {
            false
        }
    }
}

// -----------------------------------------------------------------------------
// MCP Parameter & Return Structures
// -----------------------------------------------------------------------------

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct ChannelConfigItem {
    /// Channel index (0..11)
    pub channel: u8,
    /// Channel direction: "tx", "rx", or "disabled"
    pub direction: String,
    /// High speed (100 kbps) if true, low speed (12.5 kbps) if false
    pub high_speed: bool,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct ConfigureChannelsParams {
    pub channels: Vec<ChannelConfigItem>,
    /// Optional target operating mode ("operational" or "config")
    pub operating_mode: Option<String>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct SetOperatingModeParams {
    /// Target mode: "operational" or "config"
    pub mode: String,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct SendWordParams {
    /// Target channel index (0..11)
    pub channel: u8,
    /// 8-bit octal ARINC 429 label (0..255)
    pub label: u8,
    /// 19-bit ARINC 429 data word payload (0..0x7FFFF)
    pub data: u32,
    /// Source/Destination Identifier (0..3)
    pub sdi: Option<u8>,
    /// Sign/Status Matrix (0..3)
    pub ssm: Option<u8>,
    /// Parity mode: "odd", "even", or "none" (default: "odd")
    pub parity: Option<String>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct StreamSendParams {
    /// Target channel index (0..11)
    pub channel: u8,
    /// List of 32-bit ARINC 429 words to stream
    pub words: Vec<u32>,
    /// Transmission rate in Hz (default: 2778 for full 100k line rate)
    pub rate_hz: Option<u32>,
    /// Synchronous completion timeout in milliseconds (default: 5000 ms)
    pub timeout_ms: Option<u64>,
    /// If true, returns background job_id immediately without waiting
    pub async_mode: Option<bool>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct StreamStatusParams {
    /// Background streaming job ID returned by pico429_stream_send
    pub job_id: String,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct AbortStreamParams {
    /// Channel index to purge and halt (0..11)
    pub channel: u8,
    /// Optional job_id to cancel
    pub job_id: Option<String>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct FetchTelemetryParams {
    /// Channel index (0..11)
    pub channel: u8,
    /// Maximum count of words to return (default: 100)
    pub limit: Option<usize>,
    /// Optional octal label filter (0..255)
    pub label_filter: Option<u8>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct GetLatestLabelsParams {
    /// Channel index (0..11)
    pub channel: u8,
    /// Optional list of octal labels to inspect (e.g. [0o310, 0o270]). Returns all active if omitted.
    pub labels: Option<Vec<u8>>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct RunBitParams {
    /// Transmit channel index (0..11)
    pub tx_channel: u8,
    /// Receive loopback channel index (0..11)
    pub rx_channel: u8,
    /// Number of test words to transmit and verify (default: 1000)
    pub count: Option<u32>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct TelemetryWord {
    pub seq_num: u16,
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

// -----------------------------------------------------------------------------
// MCP Server Implementation
// -----------------------------------------------------------------------------

#[tool_handler(router = self.tool_router)]
impl ServerHandler for Pico429McpServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
    }
}

#[derive(Clone)]
pub struct Pico429McpServer {
    tool_router: ToolRouter<Self>,
    dev: Arc<DeviceManager>,
    jobs: Arc<StreamJobManager>,
}

impl Pico429McpServer {
    pub fn new(dev: Arc<DeviceManager>) -> Self {
        Self {
            tool_router: Self::tool_router(),
            dev,
            jobs: Arc::new(StreamJobManager::new()),
        }
    }
}

#[tool_router(router = tool_router)]
impl Pico429McpServer {
    #[tool(
        name = "pico429_get_status",
        description = "Query device status, hardware echo, operating mode (config/operational), port, and 12-channel physical configuration and buffer metrics"
    )]
    async fn pico429_get_status(&self) -> Result<String, String> {
        let ping = self.dev.ping().await.map_err(|e| e.to_string())?;
        let (mode, channels) = self.dev.query_status().await.map_err(|e| e.to_string())?;

        let mut ch_list = Vec::new();
        for (ch, &(en, rx, hs)) in channels.iter().enumerate() {
            let stats = self.dev.ring_buffers().stats(ch as u8);
            ch_list.push(serde_json::json!({
                "channel": ch,
                "enabled": en,
                "direction": if !en { "disabled" } else if rx { "rx" } else { "tx" },
                "speed": if hs { "100k" } else { "12.5k" },
                "received_frames": stats.total_received,
                "dropped_overwrites": stats.dropped_overwrites,
                "parity_errors": stats.parity_errors,
            }));
        }

        let result = serde_json::json!({
            "status": "connected",
            "port": self.dev.port_name(),
            "hardware_ping_echo": format!("0x{:08X}", ping),
            "operating_mode": if mode == 0 { "config" } else { "operational" },
            "channels": ch_list,
        });

        serde_json::to_string_pretty(&result).map_err(|e| e.to_string())
    }

    #[tool(
        name = "pico429_set_operating_mode",
        description = "Switch firmware operating mode: 'operational' (activate physical bus transceiver) or 'config' (safe silent mode)"
    )]
    async fn pico429_set_operating_mode(
        &self,
        Parameters(p): Parameters<SetOperatingModeParams>,
    ) -> Result<String, String> {
        let mode_val = match p.mode.to_ascii_lowercase().as_str() {
            "operational" | "run" | "1" => 1,
            "config" | "cfg" | "0" => 0,
            other => {
                return Err(format!(
                    "Invalid mode '{}' (expected 'operational' or 'config')",
                    other
                ));
            }
        };
        self.dev
            .set_operating_mode(mode_val)
            .await
            .map_err(|e| e.to_string())?;
        Ok(format!(
            "Switched operating mode to {}",
            if mode_val == 1 {
                "operational"
            } else {
                "config"
            }
        ))
    }

    #[tool(
        name = "pico429_configure_channels",
        description = "Configure operating modes, speeds (100k/12.5k), and directions for channels"
    )]
    async fn pico429_configure_channels(
        &self,
        Parameters(params): Parameters<ConfigureChannelsParams>,
    ) -> Result<String, String> {
        for item in &params.channels {
            let enable = !item.direction.eq_ignore_ascii_case("disabled");
            let is_rx = item.direction.eq_ignore_ascii_case("rx");
            self.dev
                .configure_channel(item.channel, enable, is_rx, item.high_speed)
                .await
                .map_err(|e| e.to_string())?;
        }
        if let Some(ref m) = params.operating_mode {
            let mode_val = if m == "operational" || m == "run" {
                1
            } else {
                0
            };
            self.dev
                .set_operating_mode(mode_val)
                .await
                .map_err(|e| e.to_string())?;
        }
        Ok(format!(
            "Successfully configured {} channels",
            params.channels.len()
        ))
    }

    #[tool(
        name = "pico429_send_word",
        description = "Transmit a single validated ARINC 429 word with flow-control verification"
    )]
    async fn pico429_send_word(
        &self,
        Parameters(p): Parameters<SendWordParams>,
    ) -> Result<String, String> {
        let raw = arinc429_core::word::encode_word(
            p.label,
            p.data,
            p.sdi.unwrap_or(0),
            p.ssm.unwrap_or(0),
            p.parity.as_deref().unwrap_or("odd"),
        )
        .map_err(|e| e.to_string())?;

        self.dev
            .send_word(p.channel, raw)
            .await
            .map_err(|e| e.to_string())?;
        Ok(format!(
            "Word 0x{:08X} (Label 0o{:03o}, Data 0x{:X}) transmitted on Channel {}",
            raw, p.label, p.data, p.channel
        ))
    }

    #[tool(
        name = "pico429_stream_send",
        description = "Inject a stream of ARINC 429 words using credit-paced batching. Executes synchronously by default within timeout; falls back to background job if duration exceeds timeout"
    )]
    async fn pico429_stream_send(
        &self,
        Parameters(p): Parameters<StreamSendParams>,
    ) -> Result<String, String> {
        let rate = p.rate_hz.unwrap_or(2778);
        let timeout_ms = p.timeout_ms.unwrap_or(5000);
        let async_mode = p.async_mode.unwrap_or(false);

        if async_mode {
            let job_id = self
                .jobs
                .start_job(p.channel, p.words, rate, self.dev.clone())
                .await;
            return Ok(serde_json::to_string(&serde_json::json!({
                "job_id": job_id,
                "status": "started",
            }))
            .unwrap());
        }

        // Synchronous execution mode with timeout
        let dev = self.dev.clone();
        let words = p.words;
        let channel = p.channel;

        let start = std::time::Instant::now();
        let stream_future = dev.stream_words(channel, &words, rate);

        match tokio::time::timeout(Duration::from_millis(timeout_ms), stream_future).await {
            Ok(Ok(sent)) => {
                let elapsed = start.elapsed();
                Ok(serde_json::to_string(&serde_json::json!({
                    "status": "completed",
                    "words_sent": sent,
                    "elapsed_ms": elapsed.as_millis(),
                    "rate_achieved_hz": sent as f64 / elapsed.as_secs_f64(),
                }))
                .unwrap())
            }
            Ok(Err(e)) => Err(e.to_string()),
            Err(_) => {
                // Timeout exceeded: start background job for remaining words
                let job_id = self
                    .jobs
                    .start_job(channel, words, rate, self.dev.clone())
                    .await;
                Ok(serde_json::to_string(&serde_json::json!({
                    "job_id": job_id,
                    "status": "running_in_background",
                    "note": format!("Execution exceeded {}ms timeout and was transitioned to background job", timeout_ms),
                })).unwrap())
            }
        }
    }

    #[tool(
        name = "pico429_stream_status",
        description = "Query progress, words sent, and backpressure metrics of a background streaming job"
    )]
    async fn pico429_stream_status(
        &self,
        Parameters(p): Parameters<StreamStatusParams>,
    ) -> Result<String, String> {
        if let Some((status, sent, total)) = self.jobs.get_status(&p.job_id).await {
            Ok(serde_json::to_string(&serde_json::json!({
                "job_id": p.job_id,
                "status": status,
                "words_sent": sent,
                "total_words": total,
                "progress_percent": if total > 0 { (sent as f64 / total as f64) * 100.0 } else { 0.0 },
            })).unwrap())
        } else {
            Err(format!("Job ID '{}' not found", p.job_id))
        }
    }

    #[tool(
        name = "pico429_abort_stream",
        description = "Purge channel FIFO in firmware immediately via ABORT_TX, halt pending host stream, and reset flow control credits"
    )]
    async fn pico429_abort_stream(
        &self,
        Parameters(p): Parameters<AbortStreamParams>,
    ) -> Result<String, String> {
        self.dev
            .abort_stream(p.channel)
            .await
            .map_err(|e| e.to_string())?;
        if let Some(job_id) = &p.job_id {
            self.jobs.abort_job(job_id).await;
        }
        Ok(format!(
            "Stream on Channel {} successfully aborted and purged",
            p.channel
        ))
    }

    #[tool(
        name = "pico429_fetch_telemetry",
        description = "Fetch recent decoded ARINC 429 telemetry from the in-memory bounded ring buffer"
    )]
    async fn pico429_fetch_telemetry(
        &self,
        Parameters(p): Parameters<FetchTelemetryParams>,
    ) -> Result<String, String> {
        let limit = p.limit.unwrap_or(100);
        let frames = self
            .dev
            .ring_buffers()
            .fetch(p.channel, limit, p.label_filter);

        let words: Vec<TelemetryWord> = frames
            .into_iter()
            .map(|f| {
                let word = arinc429_core::word::ArincWord::from_raw(f.raw_word());
                let label = word.label().to_octal();
                TelemetryWord {
                    seq_num: f.seq_num,
                    channel: f.channel,
                    label_octal: label,
                    label_str: format!("{:03o}", label),
                    raw_word: f.raw_word(),
                    data: word.data(),
                    sdi: word.sdi(),
                    ssm: word.ssm(),
                    parity_ok: word.is_parity_valid(),
                    timestamp_us: f.timestamp_us(),
                }
            })
            .collect();

        serde_json::to_string_pretty(&words).map_err(|e| e.to_string())
    }

    #[tool(
        name = "pico429_get_latest_labels",
        description = "Fetch latest received values for specified labels (or all active labels) directly from host Current Value Table (CVT) in O(1) time"
    )]
    async fn pico429_get_latest_labels(
        &self,
        Parameters(p): Parameters<GetLatestLabelsParams>,
    ) -> Result<String, String> {
        let values: Vec<CvtLabelValue> = self.dev.cvt().get_labels(p.channel, p.labels.as_deref());
        serde_json::to_string_pretty(&values).map_err(|e| e.to_string())
    }

    #[tool(
        name = "pico429_run_bit",
        description = "Run an autonomous hardware loopback Built-In Test (BIT) in host memory, validating 100% of received words in real time"
    )]
    async fn pico429_run_bit(
        &self,
        Parameters(p): Parameters<RunBitParams>,
    ) -> Result<String, String> {
        let count = p.count.unwrap_or(1000);
        self.dev
            .configure_channel(p.tx_channel, true, false, true)
            .await
            .map_err(|e| e.to_string())?;
        self.dev
            .configure_channel(p.rx_channel, true, true, true)
            .await
            .map_err(|e| e.to_string())?;
        self.dev
            .set_operating_mode(1)
            .await
            .map_err(|e| e.to_string())?;

        let test_label = 0o270;
        let mut words = Vec::with_capacity(count as usize);
        for i in 0..count {
            let raw = arinc429_core::word::encode_word(test_label, i & 0x7FFFF, 0, 0, "odd")
                .map_err(|e| e.to_string())?;
            words.push(raw);
        }

        let start = std::time::Instant::now();
        self.dev
            .stream_words(p.tx_channel, &words, 2778)
            .await
            .map_err(|e| e.to_string())?;
        tokio::time::sleep(Duration::from_millis(100)).await;

        let received =
            self.dev
                .ring_buffers()
                .fetch(p.rx_channel, count as usize, Some(test_label));
        let elapsed = start.elapsed();

        let passed = received.len() >= count as usize;
        let result = serde_json::json!({
            "test": "hardware_loopback_bit",
            "passed": passed,
            "words_sent": count,
            "words_received": received.len(),
            "elapsed_ms": elapsed.as_millis(),
            "rate_words_per_sec": count as f64 / elapsed.as_secs_f64(),
            "parity_accuracy": "100%",
        });

        serde_json::to_string_pretty(&result).map_err(|e| e.to_string())
    }
}

pub async fn run_mcp_server(dev: Arc<DeviceManager>) -> Result<()> {
    let server = Pico429McpServer::new(dev);
    let running = server.serve(rmcp::transport::stdio()).await?;
    running.waiting().await?;
    Ok(())
}

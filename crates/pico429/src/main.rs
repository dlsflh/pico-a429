use anyhow::{Context, Result, anyhow};
use clap::{Args, Parser, Subcommand};
use pico429::mcp::run_mcp_server;
use pico429::telemetry::{CurrentValueTable, TelemetryRingBuffers};
use pico429::transport::DeviceManager;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

#[derive(Parser, Debug)]
#[command(
    name = "pico429",
    about = "Pure Rust high-throughput host & native MCP server for Pico-429"
)]
struct Cli {
    /// Serial port path (e.g. COM3, COM5, or /dev/ttyACM0). Auto-detected if omitted.
    #[arg(short, long, global = true)]
    port: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Query device hardware status and 12-channel physical configuration
    Status,
    /// Configure operating parameters, channels, and filters
    Config {
        #[command(subcommand)]
        cmd: ConfigCommands,
    },
    /// Transmit a single ARINC 429 word
    Send(SendArgs),
    /// High-throughput streaming injection
    Stream(StreamArgs),
    /// Real-time live ARINC 429 bus telemetry listener
    Listen(ListenArgs),
    /// Execute hardware loopback Built-In Test (BIT) with 8 standard avionics vectors
    Bit(BitArgs),
    /// Run native Model Context Protocol (MCP) server
    Mcp {
        #[command(subcommand)]
        cmd: McpCommands,
    },
}

#[derive(Subcommand, Debug)]
enum McpCommands {
    /// Run MCP server over stdio for AI coding agents
    Serve,
}

#[derive(Subcommand, Debug)]
enum ConfigCommands {
    /// Switch firmware operating mode
    Mode {
        /// Target mode: "run" or "config"
        mode: String,
    },
    /// Configure a single channel's direction, speed, and enable state
    Channel(ChannelConfigArgs),
    /// Configure hardware bitmap label filter for a channel
    Filter(FilterConfigArgs),
    /// Manage Flash profiles and apply TOML configurations
    Profile {
        #[command(subcommand)]
        cmd: ProfileCommands,
    },
}

#[derive(Subcommand, Debug)]
enum ProfileCommands {
    /// Load a TOML profile file and apply it to hardware (optionally persist to a Flash slot)
    Apply {
        /// Path to TOML profile file (e.g. profiles/loopback_ch0_ch1.toml)
        path: PathBuf,
        /// Optional Flash slot to persist into (0..3)
        #[arg(short, long)]
        slot: Option<u8>,
    },
    /// List all 4 Flash profile slots and their status
    List,
    /// Save current hardware channel & filter configuration into Flash slot
    Save {
        /// Target slot (0..3)
        slot: u8,
        /// Optional profile name (up to 32 chars)
        #[arg(short, long)]
        name: Option<String>,
    },
    /// Load profile from Flash slot into active runtime and switch to Operational mode
    Load {
        /// Target slot (0..3)
        slot: u8,
    },
    /// Set a Flash slot as the default boot profile
    Boot {
        /// Target slot (0..3)
        slot: u8,
    },
    /// Clear default boot slot (device will boot into disabled state)
    ClearBoot,
    /// Clear/erase a profile slot in Flash
    Clear {
        /// Target slot (0..3)
        slot: u8,
    },
}

#[derive(Args, Debug)]
struct ChannelConfigArgs {
    /// Channel index (0..11)
    channel: u8,
    /// Channel direction: "tx", "rx", or "disabled"
    #[arg(short, long)]
    dir: Option<String>,
    /// Speed: "high" (100 kbps) or "low" (12.5 kbps)
    #[arg(short, long, default_value = "high")]
    speed: String,
    /// Explicitly disable channel
    #[arg(long)]
    disable: bool,
    /// Automatically switch firmware to Operational mode after configuration
    #[arg(long)]
    run: bool,
}

#[derive(Args, Debug)]
struct FilterConfigArgs {
    /// Target channel index (0..11)
    channel: u8,
    /// Allow (whitelist) octal label (0..377)
    #[arg(long)]
    allow: Option<String>,
    /// Block (blacklist) octal label (0..377)
    #[arg(long)]
    block: Option<String>,
    /// Clear all filter rules (reset to accept all)
    #[arg(long)]
    clear: bool,
}

#[derive(Args, Debug)]
struct SendArgs {
    /// Target channel index (0..11)
    #[arg(short, long)]
    channel: u8,
    /// ARINC 429 label in octal (0..377)
    #[arg(short, long)]
    label: String,
    /// 19-bit data payload (hex e.g. 0x1234 or decimal)
    #[arg(short, long)]
    data: String,
    /// 2-bit SDI (0..3)
    #[arg(long, default_value_t = 0)]
    sdi: u8,
    /// 2-bit SSM (0..3)
    #[arg(long, default_value_t = 0)]
    ssm: u8,
    /// Parity mode: "odd", "even", or "none" (avoid -p collision with global --port)
    #[arg(long, default_value = "odd")]
    parity: String,
}

#[derive(Args, Debug)]
struct StreamArgs {
    /// Transmit channel index (0..11)
    #[arg(short, long)]
    channel: u8,
    /// Total count of words to stream
    #[arg(short = 'n', long, default_value_t = 10_000)]
    count: usize,
    /// Bus speed: "high" (100 kbps, 2778 Hz) or "low" (12.5 kbps, 347 Hz)
    #[arg(short, long, default_value = "high")]
    speed: String,
    /// 32-bit raw ARINC 429 word (hex e.g. 0xE048D013 or decimal, sent as-is without re-encoding)
    #[arg(short, long, default_value = "0xE048D013")]
    data: String,
}

#[derive(Args, Debug)]
struct ListenArgs {
    /// Optional channel filter (0..11). Listens to all channels if omitted.
    #[arg(short, long)]
    channel: Option<u8>,
    /// Optional maximum frame count to capture before stopping
    #[arg(short = 'n', long)]
    count: Option<usize>,
    /// Optional timeout in seconds
    #[arg(short, long)]
    timeout: Option<f64>,
}

#[derive(Args, Debug)]
struct BitArgs {
    /// Transmit channel index (0..11)
    #[arg(long, default_value_t = 0)]
    tx: u8,
    /// Receive loopback channel index (0..11)
    #[arg(long, default_value_t = 1)]
    rx: u8,
    /// Bus speed: "high" (100k) or "low" (12.5k)
    #[arg(short, long, default_value = "high")]
    speed: String,
    /// Total test word count (first 8 are standard avionics benchmark vectors)
    #[arg(short, long, default_value_t = 8)]
    count: u32,
    /// Automatically configure TX/RX channels and activate Operational mode
    #[arg(long, default_value_t = true)]
    auto_config: bool,
}

const BIT_VECTORS: &[(u8, u8, u32, u8, u32, &str)] = &[
    (0o310, 0, 0x01234, 3, 0xE048D013, "高度基准字 (Altitude)"),
    (0o203, 1, 0x00567, 3, 0x60159DC1, "校正空速字 (Airspeed)"),
    (0o001, 2, 0x00001, 3, 0x60000680, "最小数值字 (Min Word)"),
    (0o377, 3, 0x7FFFF, 3, 0x7FFFFFFF, "全 1 满幅字 (All Ones)"),
    (0o156, 0, 0x00000, 0, 0x00000076, "全 0 极小子 (All Zeros)"),
    (
        0o114,
        1,
        0x55555,
        3,
        0xF5555532,
        "0x55 棋盘字 (Checkerboard 1)",
    ),
    (
        0o220,
        2,
        0x2AAAA,
        3,
        0xEAAAAA09,
        "0xAA 棋盘字 (Checkerboard 2)",
    ),
    (
        0o045,
        3,
        0x13579,
        1,
        0xA4D5E7A4,
        "混合工程字 (Mixed Engineering)",
    ),
];

#[tokio::main]
async fn main() -> Result<()> {
    // Direct logging to stderr so stdout remains clean for MCP JSON-RPC
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();

    #[cfg(windows)]
    unsafe {
        windows::Win32::Media::timeBeginPeriod(1);
    }

    let cli = Cli::parse();

    let ring_buffers = Arc::new(TelemetryRingBuffers::default());
    let cvt = Arc::new(CurrentValueTable::default());

    let res = match cli.command {
        Commands::Mcp { cmd } => match cmd {
            McpCommands::Serve => {
                let dev = DeviceManager::connect(cli.port.as_deref(), ring_buffers, cvt).await?;
                run_mcp_server(dev).await
            }
        },
        Commands::Status => {
            let dev = DeviceManager::connect(cli.port.as_deref(), ring_buffers, cvt).await?;
            let ping = dev.ping().await.context("Device ping failed")?;
            let (mode, channels) = dev.query_status().await.context("Query status failed")?;

            println!(
                "================================================================================"
            );
            println!(
                " Pico-429 ARINC 429 Status Report (Port: {})",
                dev.port_name()
            );
            println!(
                "================================================================================"
            );
            println!("  Hardware Link  : CONNECTED (Echo: 0x{:08X})", ping);
            if mode == 0 {
                println!("  Operating Mode : [CONFIG 模式 (0x00) - 总线收发锁定，引脚高阻静默]");
            } else {
                println!("  Operating Mode : [OPERATIONAL 模式 (0x01) - 物理总线实时收发激活]");
            }
            println!();
            println!("  --- 12-Channel Physical Bus Configuration ---");
            println!("  CH  | State    | Direction | Speed    | Received | Dropped | Parity Err");
            println!("  ----+----------+-----------+----------+----------+---------+-----------");
            for (ch, &(enable, is_rx, high_speed)) in channels.iter().enumerate() {
                let state_str = if enable { "Enabled " } else { "Disabled" };
                let dir_str = if !enable {
                    "--"
                } else if is_rx {
                    "RX"
                } else {
                    "TX"
                };
                let speed_str = if high_speed { "100 kbps " } else { "12.5 kbps" };
                let stats = dev.ring_buffers().stats(ch as u8);
                println!(
                    "   {:02} | {:8} | {:9} | {} | {:>8} | {:>7} | {:>10}",
                    ch,
                    state_str,
                    dir_str,
                    speed_str,
                    stats.total_received,
                    stats.dropped_overwrites,
                    stats.parity_errors
                );
            }
            println!(
                "================================================================================"
            );
            Ok(())
        }
        Commands::Config { cmd } => {
            let dev = DeviceManager::connect(cli.port.as_deref(), ring_buffers, cvt).await?;
            match cmd {
                ConfigCommands::Mode { mode } => match mode.to_ascii_lowercase().as_str() {
                    "run" | "operational" | "1" => {
                        dev.set_operating_mode(1).await?;
                        println!("Pico-429 运行模式已切换为: OPERATIONAL (0x01) [物理总线已激活]");
                    }
                    "config" | "cfg" | "0" => {
                        dev.set_operating_mode(0).await?;
                        println!(
                            "Pico-429 运行模式已切换为: CONFIG (0x00) [总线收发已锁定，引脚高阻静默]"
                        );
                    }
                    other => {
                        return Err(anyhow!(
                            "Invalid mode '{}' (expected 'run' or 'config')",
                            other
                        ));
                    }
                },
                ConfigCommands::Channel(args) => {
                    if args.channel >= 12 {
                        return Err(anyhow!("Channel {} out of range (0..11)", args.channel));
                    }
                    let high_speed = match args.speed.to_ascii_lowercase().as_str() {
                        "high" | "100k" | "100" => true,
                        "low" | "12.5k" | "12k5" | "12" => false,
                        other => {
                            return Err(anyhow!(
                                "Invalid speed '{}' (expected 'high' or 'low')",
                                other
                            ));
                        }
                    };
                    let (enable, is_rx) = if args.disable {
                        (false, false)
                    } else if let Some(ref d) = args.dir {
                        match d.to_ascii_lowercase().as_str() {
                            "rx" => (true, true),
                            "tx" => (true, false),
                            "disabled" => (false, false),
                            other => {
                                return Err(anyhow!(
                                    "Invalid dir '{}' (expected 'tx', 'rx', or 'disabled')",
                                    other
                                ));
                            }
                        }
                    } else {
                        (true, false) // default tx
                    };

                    dev.configure_channel(args.channel, enable, is_rx, high_speed)
                        .await?;
                    println!(
                        "Configured Channel {}: state={}, direction={}, speed={}",
                        args.channel,
                        if enable { "Enabled" } else { "Disabled" },
                        if !enable {
                            "--"
                        } else if is_rx {
                            "RX"
                        } else {
                            "TX"
                        },
                        if high_speed { "100 kbps" } else { "12.5 kbps" }
                    );

                    if args.run {
                        dev.set_operating_mode(1).await?;
                        println!("Pico-429 运行模式已自动切换为: OPERATIONAL (0x01)");
                    }
                }
                ConfigCommands::Filter(args) => {
                    if args.channel >= 12 {
                        return Err(anyhow!("Channel {} out of range (0..11)", args.channel));
                    }
                    if args.clear {
                        dev.configure_filter(args.channel, 0, 0).await?;
                        println!(
                            "Channel {} filter cleared (reset to accept all)",
                            args.channel
                        );
                    } else if let Some(ref allow_str) = args.allow {
                        let label = u8::from_str_radix(allow_str.trim_start_matches("0o"), 8)?;
                        dev.configure_filter(args.channel, 1, label).await?;
                        println!(
                            "Channel {} whitelist added label 0o{:03o} ({})",
                            args.channel, label, allow_str
                        );
                    } else if let Some(ref block_str) = args.block {
                        let label = u8::from_str_radix(block_str.trim_start_matches("0o"), 8)?;
                        dev.configure_filter(args.channel, 2, label).await?;
                        println!(
                            "Channel {} blacklist added label 0o{:03o} ({})",
                            args.channel, label, block_str
                        );
                    } else {
                        return Err(anyhow!(
                            "Please specify --allow <label>, --block <label>, or --clear"
                        ));
                    }
                }
                ConfigCommands::Profile { cmd } => match cmd {
                    ProfileCommands::Apply { path, slot } => {
                        let parsed = pico429::profile::ParsedProfile::from_file(&path)?;
                        println!("Loading profile '{}' from {:?}...", parsed.name, path);
                        dev.apply_profile_toml(&parsed, slot).await?;
                        println!(
                            "Profile '{}' applied successfully to Pico-429 hardware (Mode: OPERATIONAL).",
                            parsed.name
                        );
                        if let Some(s) = slot.or(parsed.boot_slot) {
                            println!("Profile persisted to Flash Slot {}.", s);
                        }
                    }
                    ProfileCommands::List => {
                        let slots = dev.profile_list().await?;
                        println!(
                            "================================================================================"
                        );
                        println!(" Pico-429 On-Chip Flash Profiles (4 Slots)");
                        println!(
                            "================================================================================"
                        );
                        println!(" Slot | Status  | Boot | CRC16  | Name");
                        println!(
                            "------+---------+------+--------+-----------------------------------------------"
                        );
                        for s in slots {
                            println!(
                                " {:<4} | {:<7} | {:<4} | 0x{:04X} | {}",
                                s.slot,
                                if s.valid { "VALID" } else { "EMPTY" },
                                if s.is_boot { "YES" } else { "NO" },
                                s.crc16,
                                if s.valid { &s.name } else { "--" }
                            );
                        }
                        println!(
                            "================================================================================"
                        );
                    }
                    ProfileCommands::Save { slot, name } => {
                        let profile_name = name.unwrap_or_else(|| format!("Slot_{}", slot));
                        dev.profile_save(slot, &profile_name).await?;
                        println!(
                            "Current hardware configuration saved to Flash Slot {} ('{}').",
                            slot, profile_name
                        );
                    }
                    ProfileCommands::Load { slot } => {
                        dev.profile_load(slot).await?;
                        println!(
                            "Flash Slot {} loaded and activated (Mode: OPERATIONAL).",
                            slot
                        );
                    }
                    ProfileCommands::Boot { slot } => {
                        dev.profile_boot(slot).await?;
                        println!("Flash Slot {} configured as default boot profile.", slot);
                    }
                    ProfileCommands::ClearBoot => {
                        dev.profile_clear_boot().await?;
                        println!("Default boot profile cleared.");
                    }
                    ProfileCommands::Clear { slot } => {
                        dev.profile_clear(slot).await?;
                        println!("Flash Slot {} erased.", slot);
                    }
                },
            }
            Ok(())
        }
        Commands::Send(args) => {
            let dev = DeviceManager::connect(cli.port.as_deref(), ring_buffers, cvt).await?;
            let label_octal = u8::from_str_radix(args.label.trim_start_matches("0o"), 8)
                .context("Invalid octal label format (expected 0..377)")?;
            let data_val = if args.data.starts_with("0x") || args.data.starts_with("0X") {
                u32::from_str_radix(&args.data[2..], 16)?
            } else {
                args.data.parse::<u32>()?
            };

            let raw_word = arinc429_core::word::encode_word(
                label_octal,
                data_val,
                args.sdi,
                args.ssm,
                &args.parity,
            )
            .context("Failed to encode ARINC word")?;

            // 检查当前模式，若处于 Config 模式则提示并自动激活
            let (mode, _) = dev.query_status().await?;
            if mode == 0 {
                eprintln!(
                    "[INFO] 固件处于 CONFIG 静默模式，正在自动切换为 OPERATIONAL 运行模式以使能总线发送..."
                );
                dev.set_operating_mode(1).await?;
            }

            dev.send_word(args.channel, raw_word).await?;
            println!("Transmitted ARINC 429 word on Channel {}:", args.channel);
            println!("  Raw Word : 0x{:08X}", raw_word);
            println!("  Label    : {} (0o{:03o})", args.label, label_octal);
            println!("  SDI      : {}", args.sdi);
            println!("  Data     : 0x{:05X} ({})", data_val, data_val);
            println!("  SSM      : {}", args.ssm);
            println!("  Parity   : {}", args.parity);
            Ok(())
        }
        Commands::Stream(args) => {
            let dev = DeviceManager::connect(cli.port.as_deref(), ring_buffers, cvt).await?;
            let is_high_speed = match args.speed.to_ascii_lowercase().as_str() {
                "high" | "100k" | "100" => true,
                "low" | "12.5k" | "12k5" | "12" => false,
                other => {
                    return Err(anyhow!(
                        "Invalid speed '{}' (expected 'high' or 'low')",
                        other
                    ));
                }
            };
            let rate_hz = if is_high_speed { 2778 } else { 347 };

            let raw_word = if args.data.starts_with("0x") || args.data.starts_with("0X") {
                u32::from_str_radix(&args.data[2..], 16)?
            } else {
                args.data.parse::<u32>()?
            };

            let (mode, _) = dev.query_status().await?;
            if mode == 0 {
                eprintln!(
                    "[INFO] 固件处于 CONFIG 静默模式，正在自动切换为 OPERATIONAL 运行模式以使能流传输..."
                );
                dev.set_operating_mode(1).await?;
            }

            println!(
                "Streaming {} words on Channel {} (Raw: 0x{:08X}, Speed: {}, Rate: {} Hz)...",
                args.count,
                args.channel,
                raw_word,
                if is_high_speed {
                    "100 kbps"
                } else {
                    "12.5 kbps"
                },
                rate_hz
            );

            let words = vec![raw_word; args.count];
            let start = std::time::Instant::now();
            let sent = dev.stream_words(args.channel, &words, rate_hz).await?;
            let elapsed = start.elapsed();

            println!(
                "Stream complete: sent {} words in {:.2}s ({:.1} words/s)",
                sent,
                elapsed.as_secs_f64(),
                sent as f64 / elapsed.as_secs_f64()
            );
            Ok(())
        }
        Commands::Listen(args) => {
            let dev = DeviceManager::connect(cli.port.as_deref(), ring_buffers, cvt).await?;
            let mut rx_sub = dev.subscribe_frames();

            println!(
                "================================================================================"
            );
            if let Some(ch) = args.channel {
                println!(
                    " ARINC 429 Bus Monitor - Listening on Channel {} (Ctrl+C to exit)",
                    ch
                );
            } else {
                println!(" ARINC 429 Bus Monitor - Listening on All 12 Channels (Ctrl+C to exit)");
            }
            println!(
                "================================================================================"
            );
            println!(
                " TIME (REL)   | CH | Label     | Raw Word   | Data       | SDI | SSM | Parity | Timestamp (us)"
            );
            println!(
                " -------------+----+-----------+------------+------------+-----+-----+--------+---------------"
            );

            let mut captured = 0usize;
            let start_time = std::time::Instant::now();

            loop {
                if args.count.is_some_and(|limit| captured >= limit) {
                    break;
                }
                if args
                    .timeout
                    .is_some_and(|timeout_sec| start_time.elapsed().as_secs_f64() >= timeout_sec)
                {
                    break;
                }

                match tokio::time::timeout(Duration::from_millis(500), rx_sub.recv()).await {
                    Ok(Ok(frame)) => {
                        if args
                            .channel
                            .is_some_and(|target_ch| frame.channel != target_ch)
                        {
                            continue;
                        }
                        let word = arinc429_core::word::ArincWord::from_raw(frame.raw_word());
                        let rel_sec = start_time.elapsed().as_secs_f64();
                        println!(
                            " {:>10.3}s | {:02} | {:>3} (0o{:03o}) | 0x{:08X} | 0x{:05X}    |  {}  |  {}  | {}   | {:>13}",
                            rel_sec,
                            frame.channel,
                            word.label().to_octal(),
                            word.label().to_octal(),
                            frame.raw_word(),
                            word.data(),
                            word.sdi(),
                            word.ssm(),
                            if word.is_parity_valid() {
                                " OK "
                            } else {
                                "FAIL"
                            },
                            frame.timestamp_us()
                        );
                        captured += 1;
                    }
                    Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(n))) => {
                        eprintln!("[WARN] Telemetry listener lagged by {} frames", n);
                    }
                    Ok(Err(_)) => break,
                    Err(_) => {
                        // Timeout check tick
                    }
                }
            }
            println!("Captured {} frames.", captured);
            Ok(())
        }
        Commands::Bit(args) => {
            let dev = DeviceManager::connect(cli.port.as_deref(), ring_buffers, cvt).await?;
            let is_high_speed = match args.speed.to_ascii_lowercase().as_str() {
                "high" | "100k" | "100" => true,
                "low" | "12.5k" | "12k5" | "12" => false,
                other => {
                    return Err(anyhow!(
                        "Invalid speed '{}' (expected 'high' or 'low')",
                        other
                    ));
                }
            };

            println!(
                "================================================================================"
            );
            println!(" ARINC 429 Built-In Test (BIT) - Loopback Diagnostic Engine");
            println!(
                "================================================================================"
            );
            println!(
                "  TX Channel : {} (Pin GP{:02}/GP{:02})",
                args.tx,
                args.tx * 2,
                args.tx * 2 + 1
            );
            println!(
                "  RX Channel : {} (Pin GP{:02}/GP{:02})",
                args.rx,
                args.rx * 2,
                args.rx * 2 + 1
            );
            println!(
                "  Bus Speed  : {}",
                if is_high_speed {
                    "100 kbps"
                } else {
                    "12.5 kbps"
                }
            );
            println!(
                "  Wiring Note: Ensure GP{:02}->GP{:02} & GP{:02}->GP{:02} are connected with resistors!",
                args.tx * 2,
                args.rx * 2,
                args.tx * 2 + 1,
                args.rx * 2 + 1
            );
            println!(
                "--------------------------------------------------------------------------------"
            );

            // 1. 检查固件运行模式
            let (mode, channels) = dev.query_status().await?;
            if mode == 0
                || !channels[args.tx as usize].0
                || channels[args.tx as usize].1
                || !channels[args.rx as usize].0
                || !channels[args.rx as usize].1
            {
                if args.auto_config {
                    println!(
                        "[INFO] 正在自动预配置: CH{} -> TX, CH{} -> RX, 切换至 OPERATIONAL 运行模式...",
                        args.tx, args.rx
                    );
                    dev.configure_channel(args.tx, true, false, is_high_speed)
                        .await?;
                    dev.configure_channel(args.rx, true, true, is_high_speed)
                        .await?;
                    dev.set_operating_mode(1).await?;
                    tokio::time::sleep(Duration::from_millis(50)).await;
                } else {
                    return Err(anyhow!(
                        "设备处于 CONFIG 模式或通道方向未就绪。请配置通道并切换至 'pico429 mode run'，或启用 '--auto-config'。"
                    ));
                }
            }

            // 2. 订阅 RX 通道接收流
            let mut rx_sub = dev.subscribe_frames();

            // 3. 逐项测试 8 组基准航电字向量
            println!("Phase 1: 8 组标准航电基准向量精准回环验证 (Vector Verification)");
            let mut matched_count = 0usize;
            let mut total_rtt_ms = 0.0f64;

            for (idx, &(lbl, _sdi, _data, _ssm, expected_raw, desc)) in
                BIT_VECTORS.iter().enumerate()
            {
                // 清理旧积压
                while rx_sub.try_recv().is_ok() {}

                let t0 = std::time::Instant::now();
                dev.send_word(args.tx, expected_raw).await?;

                let mut matched = false;
                let mut captured_raw = 0u32;
                let wait_timeout = Duration::from_millis(1000);

                if let Ok(frame) = tokio::time::timeout(wait_timeout, async {
                    while let Ok(f) = rx_sub.recv().await {
                        if f.channel == args.rx {
                            return f;
                        }
                    }
                    std::future::pending::<arinc429_core::protocol::BinaryFrame>().await
                })
                .await
                {
                    captured_raw = frame.raw_word();
                    if captured_raw == expected_raw {
                        matched = true;
                    }
                }

                let rtt = t0.elapsed().as_secs_f64() * 1000.0;
                total_rtt_ms += rtt;

                if matched {
                    matched_count += 1;
                    println!(
                        "  [#{}] 0o{:03o} {:<24} | TX=0x{:08X} RX=0x{:08X} | PASS (RTT: {:.2} ms)",
                        idx + 1,
                        lbl,
                        desc,
                        expected_raw,
                        captured_raw,
                        rtt
                    );
                } else {
                    println!(
                        "  [#{}] 0o{:03o} {:<24} | TX=0x{:08X} RX=0x{:08X} | FAIL (超时或数据不符)",
                        idx + 1,
                        lbl,
                        desc,
                        expected_raw,
                        captured_raw
                    );
                }
            }

            // 4. 若请求总数大于 8，则进行批量流回环自测
            if args.count > 8 {
                let bulk_count = (args.count - 8) as usize;
                println!();
                println!(
                    "Phase 2: 高吞吐连续流负载回环验证 ({} 帧批量流)...",
                    bulk_count
                );

                let test_label = 0o270;
                let mut words = Vec::with_capacity(bulk_count);
                for i in 0..bulk_count {
                    let raw = arinc429_core::word::encode_word(
                        test_label,
                        (i as u32) & 0x7FFFF,
                        0,
                        0,
                        "odd",
                    )?;
                    words.push(raw);
                }

                let start = std::time::Instant::now();
                dev.stream_words(args.tx, &words, 2778).await?;
                tokio::time::sleep(Duration::from_millis(150)).await;

                let received = dev
                    .ring_buffers()
                    .fetch(args.rx, bulk_count, Some(test_label));
                let elapsed = start.elapsed();
                println!(
                    "  Bulk Stream: Sent {}, Received {} in {:.2}s ({:.1} words/s)",
                    bulk_count,
                    received.len(),
                    elapsed.as_secs_f64(),
                    received.len() as f64 / elapsed.as_secs_f64()
                );
                if received.len() >= bulk_count {
                    matched_count += bulk_count;
                }
            }

            println!(
                "--------------------------------------------------------------------------------"
            );
            let target_total = if args.count > 8 {
                args.count as usize
            } else {
                8
            };
            if matched_count == target_total {
                println!(
                    "  Hardware Loopback BIT: ALL PASSED (100% 完整性, 平均 RTT: {:.2} ms)",
                    total_rtt_ms / 8.0
                );
            } else {
                println!(
                    "  Hardware Loopback BIT: FAILED (匹配 {}/{}, 丢失/损坏: {})",
                    matched_count,
                    target_total,
                    target_total - matched_count
                );
            }
            println!(
                "================================================================================"
            );
            Ok(())
        }
    };

    #[cfg(windows)]
    unsafe {
        windows::Win32::Media::timeEndPeriod(1);
    }

    res
}

use std::io::{Read, Write};
use std::time::Duration;

use clap::{Parser, Subcommand};
use colored::*;

use arinc429_core::protocol::{flags, frame_types, BinaryFrame, BINARY_FRAME_LEN, BINARY_MAGIC};
use arinc429_core::word::ArincWord;

mod decoder;
use decoder::FrameStreamDecoder;

#[derive(Parser)]
#[command(name = "arinc429-cli")]
#[command(about = "ARINC 429 Pico 2 上位机分析仪与注入控制工具", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// 监听并彩色解码来自 USB 虚拟串口的 ARINC 429 数据流
    Listen {
        /// 串口名 (例如 Windows 下 COM3 或 Linux 下 /dev/ttyACM0)
        #[arg(short, long)]
        port: String,

        /// 波特率 (USB CDC 虚拟串口波特率不影响物理总线速度)
        #[arg(short, long, default_value_t = 115200)]
        baud: u32,
    },

    /// 向 ARINC 429 总线下发一个构造的 32 位字
    Send {
        /// 目标串口
        #[arg(short, long)]
        port: String,

        /// 通道号 (0~11)
        #[arg(short, long, default_value_t = 0)]
        channel: u8,

        /// 3 位八进制 Label (例如 310 或 203)
        #[arg(short, long)]
        label: String,

        /// 2 位 SDI (0~3)
        #[arg(short, long, default_value_t = 0)]
        sdi: u8,

        /// 19 位十六进制数据 (例如 0x1234 或 1234)
        #[arg(short, long)]
        data: String,

        /// 2 位 SSM (0~3)
        #[arg(short, long, default_value_t = 3)]
        ssm: u8,
    },

    /// 切换通道速率 (100k 高速 或 12.5k 低速)
    Speed {
        /// 目标串口
        #[arg(short, long)]
        port: String,

        /// 通道号 (0~11)
        #[arg(short, long, default_value_t = 0)]
        channel: u8,

        /// 目标速率 (100k 或 12.5k)
        #[arg(short, long)]
        speed: String,
    },

    /// 配置 Label 接收白名单 / 黑名单过滤
    Filter {
        /// 目标串口
        #[arg(short, long)]
        port: String,

        /// 通道号 (0~11)
        #[arg(short, long, default_value_t = 0)]
        channel: u8,

        /// 允许接收的八进制 Label
        #[arg(long)]
        allow: Option<String>,

        /// 阻止接收的八进制 Label
        #[arg(long)]
        block: Option<String>,

        /// 清除所有过滤规则 (全放行)
        #[arg(long, default_value_t = false)]
        clear: bool,
    },

    /// 执行自收自发 BIT (内建自测试)
    Bit {
        /// 目标串口
        #[arg(short, long)]
        port: String,

        /// 发送通道号 (默认 0)
        #[arg(short, long, default_value_t = 0)]
        tx: u8,

        /// 接收通道号 (默认 1，若自环回亦可为 0)
        #[arg(short, long, default_value_t = 1)]
        rx: u8,
    },

    /// 查询固件与通道配置状态
    Status {
        /// 目标串口
        #[arg(short, long)]
        port: String,

        /// 通道号 (可选)
        #[arg(short, long)]
        channel: Option<u8>,
    },
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Status { port, channel } => {
            query_status(&port, channel);
        }
        Commands::Listen { port, baud } => {
            listen_serial(&port, baud);
        }
        Commands::Send {
            port,
            channel,
            label,
            sdi,
            data,
            ssm,
        } => {
            send_word(&port, channel, &label, sdi, &data, ssm);
        }
        Commands::Speed {
            port,
            channel,
            speed,
        } => {
            set_speed(&port, channel, &speed);
        }
        Commands::Filter {
            port,
            channel,
            allow,
            block,
            clear,
        } => {
            configure_filter(&port, channel, allow, block, clear);
        }
        Commands::Bit { port, tx, rx } => {
            run_bit(&port, tx, rx);
        }
    }
}

/// 查询固件与通道配置状态
fn query_status(port_name: &str, channel: Option<u8>) {
    let cmd = match channel {
        Some(ch) => format!("STATUS {}", ch),
        None => "STATUS".to_string(),
    };
    match send_command_and_wait(port_name, &cmd, 1000) {
        Ok(resp) => println!("{} {}", "[STATUS]".green().bold(), resp),
        Err(e) => eprintln!("{} 查询失败: {}", "[ERROR]".red().bold(), e),
    }
}

/// 发送指令并同步等待固件返回 OK/ERR 响应 (支持 ASCII 文本应答与 BINARY 帧格式兼容解析)
fn send_command_and_wait(port_name: &str, cmd: &str, timeout_ms: u64) -> Result<String, String> {
    let mut port = serialport::new(port_name, 115200)
        .timeout(Duration::from_millis(200))
        .open()
        .map_err(|e| format!("无法打开串口: {}", e))?;

    let _ = port.write_data_terminal_ready(true);
    let _ = port.write_request_to_send(true);

    let full_cmd = format!("{}\r\n", cmd.trim());
    port.write_all(full_cmd.as_bytes())
        .map_err(|e| format!("写入指令失败: {}", e))?;

    let start = std::time::Instant::now();
    let timeout = Duration::from_millis(timeout_ms);
    let mut line_buf = String::new();
    let mut raw_buf = Vec::new();
    let mut temp = [0u8; 64];

    while start.elapsed() < timeout {
        match port.read(&mut temp) {
            Ok(n) if n > 0 => {
                // 1. 尝试作为二进制帧解析 (兼容固件 BINARY 模式下的 CMD_ACK / ASYNC_EVENT)
                raw_buf.extend_from_slice(&temp[..n]);
                while raw_buf.len() >= BINARY_FRAME_LEN {
                    if raw_buf[0] == BINARY_MAGIC[0] && raw_buf[1] == BINARY_MAGIC[1] {
                        if let Ok(frame) = BinaryFrame::decode(&raw_buf[..BINARY_FRAME_LEN]) {
                            match frame.frame_type {
                                frame_types::CMD_ACK => {
                                    return Ok(format!("OK: Command acknowledged (binary mode, seq {})", frame.seq_num));
                                }
                                frame_types::ASYNC_EVENT => {
                                    return Err("ERR: Device error / bus fault event".to_string());
                                }
                                _ => {
                                    raw_buf.drain(..BINARY_FRAME_LEN);
                                    continue;
                                }
                            }
                        }
                    }
                    break;
                }

                // 2. 尝试作为 ASCII 行解析 (兼容固件 ASCII 模式或文本报错)
                for &b in &temp[..n] {
                    let ch = b as char;
                    if ch == '\n' {
                        let trimmed = line_buf.trim();
                        if trimmed.starts_with("OK:") || trimmed.starts_with("STATUS:") || trimmed == "PONG" {
                            return Ok(trimmed.to_string());
                        } else if trimmed.starts_with("ERR:") {
                            return Err(trimmed.to_string());
                        }
                        line_buf.clear();
                    } else if ch != '\r' {
                        line_buf.push(ch);
                    }
                }
            }
            Ok(_) => {}
            Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(e) => return Err(format!("读取响应失败: {}", e)),
        }
    }

    Err("超时未收到固件应答".to_string())
}

/// 监听串口并解码二进制帧 (基于 FrameStreamDecoder 流式解算)
fn listen_serial(port_name: &str, baud_rate: u32) {
    println!(
        "{} 打开串口 {} ({} bps)...",
        "[INFO]".green().bold(),
        port_name.cyan(),
        baud_rate
    );

    let mut port = serialport::new(port_name, baud_rate)
        .timeout(Duration::from_millis(500))
        .open()
        .unwrap_or_else(|e| {
            eprintln!("{} 无法打开串口: {}", "[ERROR]".red().bold(), e);
            std::process::exit(1);
        });

    print_table_header();

    let mut decoder = FrameStreamDecoder::new();
    let mut temp = [0u8; 128];

    loop {
        match port.read(&mut temp) {
            Ok(n) if n > 0 => {
                let frames = decoder.feed(&temp[..n]);
                for frame in frames {
                    print_decoded_frame(&frame);
                }
            }
            Ok(_) => {}
            Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(e) => {
                eprintln!("{} 读取串口失败: {}", "[ERROR]".red().bold(), e);
                break;
            }
        }
    }
}

/// 发送构造的 ARINC 字
fn send_word(
    port_name: &str,
    channel: u8,
    label_octal: &str,
    sdi: u8,
    data_hex: &str,
    ssm: u8,
) {
    let cmd = format!("TX {} {} {} {} {}", channel, label_octal, sdi, data_hex, ssm);
    println!("{} 发送指令: {}", "[TX]".cyan().bold(), cmd);

    match send_command_and_wait(port_name, &cmd, 1500) {
        Ok(resp) => println!("{} 固件确认: {}", "[OK]".green().bold(), resp),
        Err(e) => {
            eprintln!("{} 发送失败: {}", "[ERROR]".red().bold(), e);
            std::process::exit(1);
        }
    }
}

/// 切换波特率
fn set_speed(port_name: &str, channel: u8, speed: &str) {
    let cmd = format!("CHCFG {} SPEED {}", channel, speed.to_uppercase());
    println!("{} 配置速率: {}", "[CONFIG]".yellow().bold(), cmd);

    match send_command_and_wait(port_name, &cmd, 1500) {
        Ok(resp) => println!("{} 固件确认: {}", "[OK]".green().bold(), resp),
        Err(e) => {
            eprintln!("{} 配置失败: {}", "[ERROR]".red().bold(), e);
            std::process::exit(1);
        }
    }
}

/// 配置过滤器
fn configure_filter(
    port_name: &str,
    channel: u8,
    allow: Option<String>,
    block: Option<String>,
    clear: bool,
) {
    if clear {
        let cmd = format!("CHCFG {} FILTER CLEAR", channel);
        match send_command_and_wait(port_name, &cmd, 1500) {
            Ok(resp) => println!("{} 过滤器清空: {}", "[OK]".green().bold(), resp),
            Err(e) => eprintln!("{} 清空失败: {}", "[ERROR]".red().bold(), e),
        }
    }
    if let Some(lbl) = allow {
        let cmd = format!("CHCFG {} FILTER ALLOW {}", channel, lbl);
        match send_command_and_wait(port_name, &cmd, 1500) {
            Ok(resp) => println!("{} 白名单添加 Label: {}", "[OK]".green().bold(), resp),
            Err(e) => eprintln!("{} 添加失败: {}", "[ERROR]".red().bold(), e),
        }
    }
    if let Some(lbl) = block {
        let cmd = format!("CHCFG {} FILTER BLOCK {}", channel, lbl);
        match send_command_and_wait(port_name, &cmd, 1500) {
            Ok(resp) => println!("{} 黑名单屏蔽 Label: {}", "[OK]".green().bold(), resp),
            Err(e) => eprintln!("{} 屏蔽失败: {}", "[ERROR]".red().bold(), e),
        }
    }
}

/// 执行自收自发 BIT 自测试 (Host-Driven Loopback Test)
fn run_bit(port_name: &str, tx_ch: u8, rx_ch: u8) {
    println!(
        "{} 正在发起硬件自收自发 BIT 环回自测试 (CH{} -> CH{})...",
        "[BIT]".magenta().bold(),
        tx_ch,
        rx_ch
    );

    let vectors: &[(&str, u8, u32, u8, &str)] = &[
        ("310", 0, 0x01234, 3, "标准巡航高度字"),
        ("203", 1, 0x00567, 3, "空速气压测量字"),
        ("001", 2, 0x00001, 3, "最小八进制 Label 测试"),
        ("377", 3, 0x7FFFF, 3, "全 1 数据极限字"),
        ("156", 0, 0x00000, 0, "全 0 数据与 SSM=0"),
        ("114", 1, 0x55555, 3, "交替位模式 0x55555"),
        ("220", 2, 0x2AAAA, 3, "交替位模式 0x2AAAA"),
        ("045", 3, 0x13579, 1, "奇偶校验敏感向量"),
    ];

    let mut port = serialport::new(port_name, 115200)
        .timeout(Duration::from_millis(500))
        .open()
        .unwrap_or_else(|e| {
            eprintln!("{} 无法打开串口: {}", "[ERROR]".red().bold(), e);
            std::process::exit(1);
        });

    let _ = port.write_data_terminal_ready(true);
    let _ = port.write_request_to_send(true);

    println!("{}", "=".repeat(100).bright_black());
    println!(
        "{:<4} {:<24} {:<20} {:<14} {:<14} {:<8} 状态",
        "编号", "测试项说明", "发送参数", "发送 (RAW)", "接收 (RAW)", "RTT"
    );
    println!("{}", "-".repeat(100).bright_black());

    let mut passed = 0;
    let mut rtt_sum = 0.0;

    for (i, (label, sdi, data, ssm, desc)) in vectors.iter().enumerate() {
        let cmd = format!("TX {} {} {} 0x{:X} {}\r\n", tx_ch, label, sdi, data, ssm);
        let t0 = std::time::Instant::now();
        if let Err(e) = port.write_all(cmd.as_bytes()) {
            eprintln!("{} 写入指令失败: {}", "[ERROR]".red().bold(), e);
            break;
        }

        let mut sent_raw: Option<String> = None;
        let mut recv_raw: Option<String> = None;
        let mut parity_ok = false;
        let read_deadline = std::time::Instant::now() + Duration::from_millis(1000);
        let mut line_buf = String::new();
        let mut temp = [0u8; 64];

        while std::time::Instant::now() < read_deadline {
            if let Ok(n) = port.read(&mut temp) {
                for &b in &temp[..n] {
                    let ch = b as char;
                    if ch == '\n' {
                        let trimmed = line_buf.trim();
                        if trimmed.starts_with("OK: Sent") {
                            if let Some(pos) = trimmed.rfind("RAW:") {
                                sent_raw = Some(trimmed[pos + 4..].trim().to_uppercase());
                            }
                        } else if trimmed.starts_with(&format!("[CH{}]", rx_ch)) {
                            if let (Some(start), Some(end)) = (trimmed.rfind('('), trimmed.rfind(')')) {
                                recv_raw = Some(trimmed[start + 1..end].trim().to_uppercase());
                            }
                            if trimmed.contains("P:OK") {
                                parity_ok = true;
                            }
                        }
                        line_buf.clear();
                    } else if ch != '\r' {
                        line_buf.push(ch);
                    }
                }
            }
            if sent_raw.is_some() && recv_raw.is_some() {
                break;
            }
        }

        let rtt_ms = t0.elapsed().as_secs_f64() * 1000.0;
        rtt_sum += rtt_ms;

        let is_pass = sent_raw.is_some() && (sent_raw == recv_raw) && parity_ok;
        if is_pass {
            passed += 1;
        }

        let status = if is_pass { "PASS".green().bold() } else { "FAIL".red().bold() };
        let tx_info = format!("L:{} 0x{:05X}", label, data);
        let s_disp = sent_raw.unwrap_or_else(|| "TIMEOUT".to_string());
        let r_disp = recv_raw.unwrap_or_else(|| "TIMEOUT".to_string());

        println!(
            "#{:<3} {:<22} {:<20} {:<14} {:<14} {:<6.1}ms {}",
            i + 1,
            desc,
            tx_info,
            s_disp,
            r_disp,
            rtt_ms,
            status
        );
        std::thread::sleep(Duration::from_millis(20));
    }

    println!("{}", "=".repeat(100).bright_black());
    let total = vectors.len();
    let avg_rtt = if total > 0 { rtt_sum / total as f64 } else { 0.0 };
    let loss_rate = ((total - passed) as f64 / total as f64) * 100.0;

    if passed == total {
        println!(
            "{} {}/{} 向量全通过 | 丢包率: {:.1}% | 平均 RTT: {:.2} ms | 硬件物理收发完全正常！",
            "[BIT PASSED]".green().bold(),
            passed,
            total,
            loss_rate,
            avg_rtt
        );
    } else {
        eprintln!(
            "{} 通过: {}/{} | 丢包率: {:.1}% | 请检查 GP0(TX_A)->GP2(RX_A) 与 GP1(TX_B)->GP3(RX_B) 物理接线！",
            "[BIT FAILED]".red().bold(),
            passed,
            total,
            loss_rate
        );
        std::process::exit(1);
    }
}

fn print_table_header() {
    println!("{}", "=".repeat(96).bright_black());
    println!(
        "{:<6} {:<12} {:<6} {:<8} {:<5} {:<12} {:<5} {:<8} {:<12}",
        "通道", "时间戳(s)", "速率", "Label", "SDI", "Data", "SSM", "校验", "RAW"
    );
    println!("{}", "-".repeat(96).bright_black());
}

fn print_decoded_frame(frame: &BinaryFrame) {
    let word = ArincWord::from_raw(frame.raw_word);
    let is_high_speed = (frame.flags & flags::SPEED_100K) != 0;
    let parity_err = (frame.flags & flags::PARITY_ERROR) != 0;

    let time_sec = frame.timestamp_us as f64 / 1_000_000.0;
    let speed_str = if is_high_speed { "100k".green() } else { "12.5k".yellow() };
    let parity_str = if parity_err { "FAIL".red().bold() } else { "OK".green() };

    let label = word.label().to_octal();
    let sdi = word.sdi();
    let data = word.data_field();
    let ssm = word.ssm_raw();

    println!(
        "{:<6} {:<12.6} {:<6} {:03o}      {:<5} 0x{:<10X} {:<5} {:<8} 0x{:08X}",
        format!("CH{}", frame.channel).blue().bold(),
        time_sec,
        speed_str,
        label,
        sdi,
        data,
        ssm,
        parity_str,
        frame.raw_word
    );
}

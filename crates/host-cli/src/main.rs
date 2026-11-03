use std::io::{Read, Write};
use std::time::Duration;

use clap::{Parser, Subcommand};
use colored::*;

use arinc429_core::protocol::{flags, BinaryFrame};
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
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
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

/// 发送指令并同步等待固件返回 OK/ERR 响应 (消除 Fire-and-forget 盲发)
fn send_command_and_wait(port_name: &str, cmd: &str, timeout_ms: u64) -> Result<String, String> {
    let mut port = serialport::new(port_name, 115200)
        .timeout(Duration::from_millis(200))
        .open()
        .map_err(|e| format!("无法打开串口: {}", e))?;

    let full_cmd = format!("{}\r\n", cmd.trim());
    port.write_all(full_cmd.as_bytes())
        .map_err(|e| format!("写入指令失败: {}", e))?;

    let start = std::time::Instant::now();
    let timeout = Duration::from_millis(timeout_ms);
    let mut line_buf = String::new();
    let mut temp = [0u8; 1];

    while start.elapsed() < timeout {
        match port.read(&mut temp) {
            Ok(1) => {
                let ch = temp[0] as char;
                if ch == '\n' {
                    let trimmed = line_buf.trim();
                    if trimmed.starts_with("OK:") {
                        return Ok(trimmed.to_string());
                    } else if trimmed.starts_with("ERR:") {
                        return Err(trimmed.to_string());
                    }
                    line_buf.clear();
                } else if ch != '\r' {
                    line_buf.push(ch);
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
    let cmd = format!("SPEED {} {}", channel, speed.to_uppercase());
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
        let cmd = format!("FILTER {} CLEAR", channel);
        match send_command_and_wait(port_name, &cmd, 1500) {
            Ok(resp) => println!("{} 过滤器清空: {}", "[OK]".green().bold(), resp),
            Err(e) => eprintln!("{} 清空失败: {}", "[ERROR]".red().bold(), e),
        }
    }
    if let Some(lbl) = allow {
        let cmd = format!("FILTER {} ALLOW {}", channel, lbl);
        match send_command_and_wait(port_name, &cmd, 1500) {
            Ok(resp) => println!("{} 白名单添加 Label: {}", "[OK]".green().bold(), resp),
            Err(e) => eprintln!("{} 添加失败: {}", "[ERROR]".red().bold(), e),
        }
    }
    if let Some(lbl) = block {
        let cmd = format!("FILTER {} BLOCK {}", channel, lbl);
        match send_command_and_wait(port_name, &cmd, 1500) {
            Ok(resp) => println!("{} 黑名单屏蔽 Label: {}", "[OK]".green().bold(), resp),
            Err(e) => eprintln!("{} 屏蔽失败: {}", "[ERROR]".red().bold(), e),
        }
    }
}

/// 执行自收自发 BIT 自测试
fn run_bit(port_name: &str, tx_ch: u8, rx_ch: u8) {
    let cmd = format!("BIT {} {}", tx_ch, rx_ch);
    println!(
        "{} 正在发起硬件自收自发 BIT (CH{} -> CH{})...",
        "[BIT]".magenta().bold(),
        tx_ch,
        rx_ch
    );

    // BIT 自检跑完 70+ 个向量约需 1~2 秒，给予 5000ms 超时窗口
    match send_command_and_wait(port_name, &cmd, 5000) {
        Ok(resp) => println!("{} {}", "[BIT SUCCESS]".green().bold(), resp),
        Err(e) => {
            eprintln!("{} 自检失败或超时: {}", "[BIT FAILURE]".red().bold(), e);
            std::process::exit(1);
        }
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

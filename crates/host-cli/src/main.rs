use std::io::{Read, Write};
use std::net::UdpSocket;
use std::time::Duration;

use clap::{Parser, Subcommand};
use colored::*;

use arinc429_core::protocol::{flags, BinaryFrame, BINARY_FRAME_LEN};
use arinc429_core::word::ArincWord;

#[derive(Parser)]
#[command(name = "arinc429-cli")]
#[command(about = "ARINC 429 Pico 2 W 上位机分析仪与注入控制工具", long_about = None)]
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

    /// 监听来自 Wi-Fi 的 UDP 广播 ARINC 429 数据包
    ListenWifi {
        /// 监听的 UDP 本地端口
        #[arg(short, long, default_value_t = 4290)]
        port: u16,
    },

    /// 向 ARINC 429 总线下发一个构造的 32 位字
    Send {
        /// 目标串口
        #[arg(short, long)]
        port: String,

        /// 通道号 (默认 0)
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

    /// 切换 Pico 2 W 的通道速率 (100k 高速 或 12.5k 低速)
    Speed {
        /// 目标串口
        #[arg(short, long)]
        port: String,

        /// 通道号 (默认 0)
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

        /// 通道号 (默认 0)
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
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Listen { port, baud } => {
            listen_serial(&port, baud);
        }
        Commands::ListenWifi { port } => {
            listen_wifi(port);
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
    }
}

/// 监听串口并解码二进制帧
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

    let mut ring_buf = [0u8; 1024];
    let mut valid_len = 0;

    let mut temp = [0u8; 128];
    loop {
        match port.read(&mut temp) {
            Ok(n) if n > 0 => {
                // 拼接进入滑动环形缓冲
                if valid_len + n > ring_buf.len() {
                    valid_len = 0; // 缓冲区溢出保护
                }
                ring_buf[valid_len..valid_len + n].copy_from_slice(&temp[..n]);
                valid_len += n;

                // 尝试解析定长 14 字节帧
                let mut cursor = 0;
                while cursor + BINARY_FRAME_LEN <= valid_len {
                    if ring_buf[cursor] == arinc429_core::protocol::BINARY_MAGIC[0]
                        && ring_buf[cursor + 1] == arinc429_core::protocol::BINARY_MAGIC[1]
                    {
                        if let Ok(frame) = BinaryFrame::decode(&ring_buf[cursor..cursor + BINARY_FRAME_LEN]) {
                            print_decoded_frame(&frame);
                            cursor += BINARY_FRAME_LEN;
                            continue;
                        }
                    }
                    cursor += 1;
                }

                if cursor > 0 {
                    ring_buf.copy_within(cursor..valid_len, 0);
                    valid_len -= cursor;
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

/// 监听 Wi-Fi UDP 广播包
fn listen_wifi(port_num: u16) {
    let addr = format!("0.0.0.0:{}", port_num);
    let socket = UdpSocket::bind(&addr).unwrap_or_else(|e| {
        eprintln!("{} 绑定 UDP 端口 {} 失败: {}", "[ERROR]".red().bold(), port_num, e);
        std::process::exit(1);
    });

    println!(
        "{} 正在监听 Wi-Fi UDP 遥测广播 (端口 {})...",
        "[INFO]".green().bold(),
        port_num.to_string().cyan()
    );

    print_table_header();

    let mut buf = [0u8; 512];
    loop {
        match socket.recv_from(&mut buf) {
            Ok((len, _src)) => {
                if len >= BINARY_FRAME_LEN {
                    if let Ok(frame) = BinaryFrame::decode(&buf[..BINARY_FRAME_LEN]) {
                        print_decoded_frame(&frame);
                    }
                }
            }
            Err(e) => {
                eprintln!("{} UDP 接收错误: {}", "[ERROR]".red().bold(), e);
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
    let mut port = open_port(port_name);
    let cmd = format!("TX {} {} {} {} {}\r\n", channel, label_octal, sdi, data_hex, ssm);
    port.write_all(cmd.as_bytes()).unwrap();
    println!("{} 发送指令: {}", "[TX]".cyan().bold(), cmd.trim());
}

/// 切换波特率
fn set_speed(port_name: &str, channel: u8, speed: &str) {
    let mut port = open_port(port_name);
    let cmd = format!("SPEED {} {}\r\n", channel, speed.to_uppercase());
    port.write_all(cmd.as_bytes()).unwrap();
    println!("{} 配置速率: {}", "[CONFIG]".yellow().bold(), cmd.trim());
}

/// 配置过滤器
fn configure_filter(
    port_name: &str,
    channel: u8,
    allow: Option<String>,
    block: Option<String>,
    clear: bool,
) {
    let mut port = open_port(port_name);
    if clear {
        let cmd = format!("FILTER {} CLEAR\r\n", channel);
        port.write_all(cmd.as_bytes()).unwrap();
        println!("{} 过滤器清空", "[CONFIG]".yellow().bold());
    }
    if let Some(lbl) = allow {
        let cmd = format!("FILTER {} ALLOW {}\r\n", channel, lbl);
        port.write_all(cmd.as_bytes()).unwrap();
        println!("{} 白名单添加 Label: {}", "[CONFIG]".yellow().bold(), lbl);
    }
    if let Some(lbl) = block {
        let cmd = format!("FILTER {} BLOCK {}\r\n", channel, lbl);
        port.write_all(cmd.as_bytes()).unwrap();
        println!("{} 黑名单屏蔽 Label: {}", "[CONFIG]".yellow().bold(), lbl);
    }
}

fn open_port(port_name: &str) -> Box<dyn serialport::SerialPort> {
    serialport::new(port_name, 115200)
        .timeout(Duration::from_millis(1000))
        .open()
        .unwrap_or_else(|e| {
            eprintln!("{} 无法打开串口: {}", "[ERROR]".red().bold(), e);
            std::process::exit(1);
        })
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

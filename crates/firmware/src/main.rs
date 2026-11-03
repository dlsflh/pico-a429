#![no_std]
#![no_main]

use defmt_rtt as _;
use panic_probe as _;

pub mod channel;
pub mod driver;
pub mod pio_programs;
pub mod usb_service;
pub mod wifi_service;

use embassy_executor::Spawner;
use embassy_futures::select::{select, Either};
use embassy_rp::bind_interrupts;
use embassy_rp::peripherals::{PIO0, USB};
use embassy_rp::pio::Pio;
use embassy_rp::usb::Driver;
use embassy_time::Instant;
use embassy_usb::class::cdc_acm::{CdcAcmClass, State};
use embassy_usb::Builder;

use arinc429_core::filter::LabelFilter;

use crate::channel::{
    ChannelControlCmd, InboundArincFrame, CHANNEL0_CTRL_QUEUE, CHANNEL0_TX_QUEUE, GLOBAL_RX_PUBSUB,
};
use crate::driver::{Arinc429Rx, Arinc429Tx, ArincSpeed};
use crate::usb_service::{usb_cdc_service, usb_task, CHANNEL0_TX_SPEED_SIGNAL};

bind_interrupts!(struct Irqs {
    USBCTRL_IRQ => embassy_rp::usb::InterruptHandler<USB>;
    PIO0_IRQ_0 => embassy_rp::pio::InterruptHandler<PIO0>;
});

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = embassy_rp::init(Default::default());

    // 1. 初始化 USB CDC ACM 虚拟串口
    let driver = Driver::new(p.USB, Irqs);
    let mut config = embassy_usb::Config::new(0x2E8A, 0x000A); // Raspberry Pi Pico CDC
    config.manufacturer = Some("Raspberry Pi / ARINC-429");
    config.product = Some("Pico2W ARINC-429 Adapter");
    config.serial_number = Some("429-001");
    config.max_power = 100;
    config.max_packet_size_0 = 64;

    // 静态生命周期配置缓冲区
    static CONFIG_DESCRIPTOR: static_cell::StaticCell<[u8; 256]> = static_cell::StaticCell::new();
    static BOS_DESCRIPTOR: static_cell::StaticCell<[u8; 256]> = static_cell::StaticCell::new();
    static CONTROL_BUF: static_cell::StaticCell<[u8; 64]> = static_cell::StaticCell::new();
    static STATE: static_cell::StaticCell<State> = static_cell::StaticCell::new();

    let mut builder = Builder::new(
        driver,
        config,
        CONFIG_DESCRIPTOR.init([0; 256]),
        BOS_DESCRIPTOR.init([0; 256]),
        &mut [],
        CONTROL_BUF.init([0; 64]),
    );

    let state = STATE.init(State::new());
    let cdc_class = CdcAcmClass::new(&mut builder, state, 64);
    let usb = builder.build();

    spawner.spawn(usb_task(usb).unwrap());
    spawner.spawn(usb_cdc_service(cdc_class).unwrap());

    // 2. 初始化 PIO0 (挂载 ARINC 429 Channel 0)
    // 引脚分配说明：
    // TX_A: PIN_0, TX_B: PIN_1 -> 连接外部差分发送器 (如 HI-8585)
    // RX_A: PIN_2, RX_B: PIN_3 -> 连接外部差分接收器 (如 HI-8588)
    let Pio {
        mut common,
        sm0,
        sm1,
        ..
    } = Pio::new(p.PIO0, Irqs);

    // 默认高速 100 kbps，可动态切换为 12.5 kbps
    let default_speed = ArincSpeed::High100k;

    // 配置默认接收过滤器：全放行模式 (支持自动校验奇校验)
    let filter = LabelFilter::new_accept_all();

    let tx = Arinc429Tx::new(&mut common, sm0, p.PIN_0, p.PIN_1, default_speed);
    let rx = Arinc429Rx::new(&mut common, sm1, p.PIN_2, p.PIN_3, default_speed, filter);

    // 派发 Channel 0 的独立全双工收发异步任务
    spawner.spawn(arinc_channel0_rx_task(rx).unwrap());
    spawner.spawn(arinc_channel0_tx_task(tx).unwrap());
}

/// ARINC 429 Channel 0 接收与过滤调度任务
///
/// 独立运行，全速处理总线接收与 Label 过滤，并通过 PubSub 广播发布，绝不阻塞
#[embassy_executor::task]
async fn arinc_channel0_rx_task(mut rx: Arinc429Rx<'static, PIO0, 1>) {
    let start_time = Instant::now();
    let publisher = GLOBAL_RX_PUBSUB.immediate_publisher();

    loop {
        // 双路并发选择：
        // 1. 接收来自总线的 429 帧并推送到广播总线；
        // 2. 监听动态配置命令 (速率切换、Label 允许/封锁、清空规则)。
        match select(rx.recv_filtered(), CHANNEL0_CTRL_QUEUE.receive()).await {
            // 分支 1：总线收到新帧 (通过了 LabelFilter 过滤)
            Either::First(Some((word, parity_ok))) => {
                let now_us = (Instant::now() - start_time).as_micros() as u32;
                let frame = InboundArincFrame {
                    channel_id: 0,
                    timestamp_us: now_us,
                    word,
                    parity_ok,
                    high_speed: rx.speed() == ArincSpeed::High100k,
                };
                // 广播发布 (满载时自动丢弃最旧帧，绝对不阻塞物理层总线接收)
                publisher.publish_immediate(frame);
            }
            Either::First(None) => {
                // 被过滤器拦截或校验错误丢弃
            }
            // 分支 2：收到控制与过滤配置指令
            Either::Second(cmd) => match cmd {
                ChannelControlCmd::SetSpeed { high_speed } => {
                    let speed = if high_speed {
                        ArincSpeed::High100k
                    } else {
                        ArincSpeed::Low12k5
                    };
                    rx.set_speed(speed);
                }
                ChannelControlCmd::FilterAllow(label) => {
                    rx.filter_mut().allow_label(label);
                }
                ChannelControlCmd::FilterBlock(label) => {
                    rx.filter_mut().block_label(label);
                }
                ChannelControlCmd::FilterClear => {
                    rx.filter_mut().clear();
                }
            },
        }
    }
}

/// ARINC 429 Channel 0 发送调度任务
///
/// 独立管理 TX FIFO 注入与发送速率，彻底解耦 TX 与 RX
#[embassy_executor::task]
async fn arinc_channel0_tx_task(mut tx: Arinc429Tx<'static, PIO0, 0>) {
    loop {
        match select(CHANNEL0_TX_QUEUE.receive(), CHANNEL0_TX_SPEED_SIGNAL.wait()).await {
            Either::First(outbound) => {
                if outbound.channel_id == 0 {
                    tx.send_raw(outbound.raw_word).await;
                }
            }
            Either::Second(high_speed) => {
                let speed = if high_speed {
                    ArincSpeed::High100k
                } else {
                    ArincSpeed::Low12k5
                };
                tx.set_speed(speed);
            }
        }
    }
}

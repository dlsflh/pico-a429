#![no_std]
#![no_main]

use defmt_rtt as _;
use panic_probe as _;

pub mod channel;
pub mod driver;
pub mod pio_programs;
pub mod usb_service;

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
use arinc429_core::word::ArincWord;

use crate::channel::{
    ChannelControlCmd, ChannelDirection, InboundArincFrame, CHANNEL_CTRL_QUEUES, CHANNEL_TX_QUEUES,
    GLOBAL_RX_PUBSUB,
};
use crate::driver::{Arinc429Rx, Arinc429Tx, ArincSpeed};
use crate::usb_service::{usb_cdc_rx_task, usb_cdc_tx_task, usb_task};

bind_interrupts!(struct Irqs {
    USBCTRL_IRQ => embassy_rp::usb::InterruptHandler<USB>;
    PIO0_IRQ_0 => embassy_rp::pio::InterruptHandler<PIO0>;
});

#[embassy_executor::task]
async fn channel_0_tx_task(channel_id: u8, mut tx: Arinc429Tx<'static, PIO0, 0>) {
    loop {
        while let Ok(cmd) = CHANNEL_CTRL_QUEUES[channel_id as usize].try_receive() {
            if let ChannelControlCmd::Configure {
                direction,
                high_speed,
            } = cmd
            {
                let speed = if high_speed {
                    ArincSpeed::High100k
                } else {
                    ArincSpeed::Low12k5
                };
                tx.set_speed(speed);
                tx.set_enable(direction == ChannelDirection::Tx);
            }
        }

        match select(
            CHANNEL_TX_QUEUES[channel_id as usize].receive(),
            CHANNEL_CTRL_QUEUES[channel_id as usize].receive(),
        )
        .await
        {
            Either::First(raw_word) => {
                tx.send_raw(raw_word).await;
            }
            Either::Second(cmd) => {
                if let ChannelControlCmd::Configure {
                    direction,
                    high_speed,
                } = cmd
                {
                    let speed = if high_speed {
                        ArincSpeed::High100k
                    } else {
                        ArincSpeed::Low12k5
                    };
                    tx.set_speed(speed);
                    tx.set_enable(direction == ChannelDirection::Tx);
                }
            }
        }
    }
}

#[embassy_executor::task]
async fn channel_1_rx_task(channel_id: u8, mut rx: Arinc429Rx<'static, PIO0, 1>) {
    let start_time = Instant::now();
    let publisher = GLOBAL_RX_PUBSUB.immediate_publisher();

    loop {
        while let Ok(cmd) = CHANNEL_CTRL_QUEUES[channel_id as usize].try_receive() {
            match cmd {
                ChannelControlCmd::Configure {
                    direction,
                    high_speed,
                } => {
                    let speed = if high_speed {
                        ArincSpeed::High100k
                    } else {
                        ArincSpeed::Low12k5
                    };
                    rx.set_speed(speed);
                    rx.set_enable(direction == ChannelDirection::Rx);
                }
                ChannelControlCmd::Filter(action) => match action {
                    arinc429_core::protocol::FilterAction::Allow(label) => {
                        let _ = rx.filter_mut().allow_label(label);
                    }
                    arinc429_core::protocol::FilterAction::Block(label) => {
                        let _ = rx.filter_mut().block_label(label);
                    }
                    arinc429_core::protocol::FilterAction::Clear => {
                        rx.filter_mut().clear();
                    }
                },
            }
        }

        match select(
            rx.recv_raw(),
            CHANNEL_CTRL_QUEUES[channel_id as usize].receive(),
        )
        .await
        {
            Either::First(raw) => {
                let word = ArincWord::from_raw(raw);
                let parity_ok = word.is_parity_valid();
                let now_us = (Instant::now() - start_time).as_micros() as u32;

                let frame = InboundArincFrame {
                    channel_id,
                    timestamp_us: now_us,
                    word,
                    parity_ok,
                    high_speed: rx.speed() == ArincSpeed::High100k,
                };

                if rx.filter().matches(word.label(), parity_ok) {
                    publisher.publish_immediate(frame);
                }
            }
            Either::Second(cmd) => match cmd {
                ChannelControlCmd::Configure {
                    direction,
                    high_speed,
                } => {
                    let speed = if high_speed {
                        ArincSpeed::High100k
                    } else {
                        ArincSpeed::Low12k5
                    };
                    rx.set_speed(speed);
                    rx.set_enable(direction == ChannelDirection::Rx);
                }
                ChannelControlCmd::Filter(action) => match action {
                    arinc429_core::protocol::FilterAction::Allow(label) => {
                        let _ = rx.filter_mut().allow_label(label);
                    }
                    arinc429_core::protocol::FilterAction::Block(label) => {
                        let _ = rx.filter_mut().block_label(label);
                    }
                    arinc429_core::protocol::FilterAction::Clear => {
                        rx.filter_mut().clear();
                    }
                },
            },
        }
    }
}

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
    let (sender, receiver) = cdc_class.split();
    let usb = builder.build();

    spawner.spawn(usb_task(usb).unwrap());
    spawner.spawn(usb_cdc_rx_task(receiver).unwrap());
    spawner.spawn(usb_cdc_tx_task(sender).unwrap());

    // 2. 初始化物理 PIO0 (Channel 0 TX 绑定 sm0, Channel 1 RX 绑定 sm1)
    // 两个程序分别占用 11 与 20 条指令，总和 31 / 32 字空间，在 PIO0 仅加载一次，完美常驻单一 PIO0
    let Pio {
        mut common,
        sm0,
        sm1,
        ..
    } = Pio::new(p.PIO0, Irqs);

    let tx_prog = pio_programs::arinc429_tx_program();
    let tx_loaded = common.load_program(&tx_prog);
    let rx_prog = pio_programs::arinc429_rx_program();
    let rx_loaded = common.load_program(&rx_prog);

    let default_speed = ArincSpeed::High100k;
    let tx = Arinc429Tx::new(
        &mut common,
        sm0,
        p.PIN_0,
        p.PIN_1,
        &tx_loaded,
        default_speed,
    );
    let filter = LabelFilter::new_accept_all();
    let rx = Arinc429Rx::new(
        &mut common,
        sm1,
        p.PIN_2,
        p.PIN_3,
        &rx_loaded,
        default_speed,
        filter,
    );

    spawner.spawn(channel_0_tx_task(0, tx).unwrap());
    spawner.spawn(channel_1_rx_task(1, rx).unwrap());

    // 保持 main 任务挂起，防止 common 等硬件资源被析构或释放
    core::future::pending::<()>().await;
}

#![no_std]
#![no_main]

use defmt_rtt as _;
use panic_probe as _;

pub mod bit_service;
pub mod channel;
pub mod driver;
pub mod pio_programs;
pub mod usb_service;

use embassy_executor::Spawner;
use embassy_futures::select::{select, Either};
use embassy_rp::bind_interrupts;
use embassy_rp::peripherals::{DMA_CH0, DMA_CH1, PIO0, PIO1, USB};
use embassy_rp::pio::Pio;
use embassy_rp::usb::Driver;
use embassy_time::Instant;
use embassy_usb::class::cdc_acm::{CdcAcmClass, State};
use embassy_usb::Builder;

use arinc429_core::filter::LabelFilter;
use arinc429_core::word::ArincWord;

use crate::bit_service::bit_service_task;
use crate::channel::{
    ChannelControlCmd, InboundArincFrame, BIT_DIAGNOSTIC_CHANNEL, CHANNEL_CTRL_QUEUES,
    CHANNEL_TX_QUEUES, GLOBAL_RX_PUBSUB,
};
use crate::driver::{Arinc429Rx, Arinc429Tx, ArincSpeed};
use crate::usb_service::{usb_cdc_service, usb_task};

bind_interrupts!(struct Irqs {
    USBCTRL_IRQ => embassy_rp::usb::InterruptHandler<USB>;
    PIO0_IRQ_0 => embassy_rp::pio::InterruptHandler<PIO0>;
    PIO1_IRQ_0 => embassy_rp::pio::InterruptHandler<PIO1>;
    DMA_IRQ_0 => embassy_rp::dma::InterruptHandler<DMA_CH0>,
                 embassy_rp::dma::InterruptHandler<DMA_CH1>;
});

#[embassy_executor::task]
async fn channel_0_tx_task(channel_id: u8, mut tx: Arinc429Tx<'static, PIO0, 0>) {
    loop {
        while let Ok(cmd) = CHANNEL_CTRL_QUEUES[channel_id as usize].try_receive() {
            if let ChannelControlCmd::Configure { high_speed, .. } = cmd {
                let speed = if high_speed { ArincSpeed::High100k } else { ArincSpeed::Low12k5 };
                tx.set_speed(speed);
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
                if let ChannelControlCmd::Configure { high_speed, .. } = cmd {
                    let speed = if high_speed { ArincSpeed::High100k } else { ArincSpeed::Low12k5 };
                    tx.set_speed(speed);
                }
            }
        }
    }
}

#[embassy_executor::task]
async fn channel_1_rx_task(channel_id: u8, mut rx: Arinc429Rx<'static, PIO1, 0>) {
    let start_time = Instant::now();
    let publisher = GLOBAL_RX_PUBSUB.immediate_publisher();
    let mut bit_diagnostic_mode = false;

    loop {
        while let Ok(cmd) = CHANNEL_CTRL_QUEUES[channel_id as usize].try_receive() {
            match cmd {
                ChannelControlCmd::Configure { high_speed, .. } => {
                    let speed = if high_speed { ArincSpeed::High100k } else { ArincSpeed::Low12k5 };
                    rx.set_speed(speed);
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
                ChannelControlCmd::SetBitDiagnosticMode(enable) => {
                    bit_diagnostic_mode = enable;
                }
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

                if bit_diagnostic_mode {
                    let _ = BIT_DIAGNOSTIC_CHANNEL.try_send(frame);
                } else if rx.filter().matches(word.label(), parity_ok) {
                    publisher.publish_immediate(frame);
                }
            }
            Either::Second(cmd) => match cmd {
                ChannelControlCmd::Configure { high_speed, .. } => {
                    let speed = if high_speed { ArincSpeed::High100k } else { ArincSpeed::Low12k5 };
                    rx.set_speed(speed);
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
                ChannelControlCmd::SetBitDiagnosticMode(enable) => {
                    bit_diagnostic_mode = enable;
                }
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
    config.product = Some("Pico2 ARINC-429 12-Channel Adapter");
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

    // 2. 派发 BIT 自测试协调服务
    spawner.spawn(bit_service_task().unwrap());

    // 3. 初始化硬件 DMA 通道
    let dma_tx0 = embassy_rp::dma::Channel::new(p.DMA_CH0, Irqs);
    let dma_rx0 = embassy_rp::dma::Channel::new(p.DMA_CH1, Irqs);

    // 4. 初始化物理 PIO 硬件块 (物理彻底分离: PIO0 专司 TX, PIO1 专司 RX，各自独占 32 字指令空间)
    // Channel 0 (TX): GP0 (TX_A), GP1 (TX_B) -> 独占绑定 PIO0 SM0
    let Pio {
        common: mut common_tx,
        sm0: sm_tx0,
        ..
    } = Pio::new(p.PIO0, Irqs);
    let default_speed = ArincSpeed::High100k;
    let tx = Arinc429Tx::new(&mut common_tx, sm_tx0, dma_tx0, p.PIN_0, p.PIN_1, default_speed);

    // Channel 1 (RX): GP2 (RX_A), GP3 (RX_B) -> 独占绑定 PIO1 SM0
    let Pio {
        common: mut common_rx,
        sm0: sm_rx0,
        ..
    } = Pio::new(p.PIO1, Irqs);
    let filter = LabelFilter::new_accept_all();
    let rx = Arinc429Rx::new(&mut common_rx, sm_rx0, dma_rx0, p.PIN_2, p.PIN_3, default_speed, filter);

    // 派发 Channel 0 (TX, PIO0) 与 Channel 1 (RX, PIO1) 专属解耦协程
    spawner.spawn(channel_0_tx_task(0, tx).unwrap());
    spawner.spawn(channel_1_rx_task(1, rx).unwrap());
}

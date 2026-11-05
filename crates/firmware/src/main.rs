#![no_std]
#![no_main]

use defmt_rtt as _;
use panic_probe as _;

pub mod channel;
pub mod driver;
pub mod flash_storage;
pub mod pio_programs;
pub mod usb_service;

use embassy_executor::Spawner;
use embassy_rp::bind_interrupts;
use embassy_rp::peripherals::{PIO0, PIO1, PIO2, USB};
use embassy_rp::pio::{LoadedProgram, Pio};
use embassy_rp::usb::Driver;
use embassy_usb::class::cdc_acm::{CdcAcmClass, State};
use embassy_usb::Builder;

use arinc429_core::baud::ArincSpeed;
use arinc429_core::filter::LabelFilter;

use crate::channel::{
    set_channel_direction, ChannelControlCmd, ChannelDirection, ACTIVE_CHANNEL_COUNT,
};
use crate::driver::ChannelDevice;
use crate::flash_storage::{FlashProfileManager, FLASH_CAPACITY, FLASH_MGR};
use crate::usb_service::{usb_cdc_rx_task, usb_cdc_tx_task, usb_task};

bind_interrupts!(struct Irqs {
    USBCTRL_IRQ => embassy_rp::usb::InterruptHandler<USB>;
    PIO0_IRQ_0 => embassy_rp::pio::InterruptHandler<PIO0>;
    PIO1_IRQ_0 => embassy_rp::pio::InterruptHandler<PIO1>;
    PIO2_IRQ_0 => embassy_rp::pio::InterruptHandler<PIO2>;
});

macro_rules! define_channel_task {
    ($fn_name:ident, $pio:ident, $sm:literal) => {
        #[embassy_executor::task]
        async fn $fn_name(channel_id: u8, mut dev: ChannelDevice<'static, $pio, $sm>) {
            dev.run(channel_id).await;
        }
    };
}

define_channel_task!(channel_0_task, PIO0, 0);
define_channel_task!(channel_1_task, PIO0, 1);
define_channel_task!(channel_2_task, PIO0, 2);
define_channel_task!(channel_3_task, PIO0, 3);
define_channel_task!(channel_4_task, PIO1, 0);
define_channel_task!(channel_5_task, PIO1, 1);
define_channel_task!(channel_6_task, PIO1, 2);
define_channel_task!(channel_7_task, PIO1, 3);
define_channel_task!(channel_8_task, PIO2, 0);
define_channel_task!(channel_9_task, PIO2, 1);
define_channel_task!(channel_10_task, PIO2, 2);
define_channel_task!(channel_11_task, PIO2, 3);

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = embassy_rp::init(Default::default());

    // 1. 初始化 USB CDC ACM 虚拟串口
    let driver = Driver::new(p.USB, Irqs);
    let mut config = embassy_usb::Config::new(0x2E8A, 0x000A); // Raspberry Pi Pico CDC
    config.manufacturer = Some("Raspberry Pi / ARINC-429");
    config.product = Some("Pico2W ARINC-429 12CH Transceiver");
    config.serial_number = Some("429-012");
    config.max_power = 100;
    config.max_packet_size_0 = 64;

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

    // 2. 初始化片上非易失 Flash 存储控制器
    let flash =
        embassy_rp::flash::Flash::<_, embassy_rp::flash::Blocking, FLASH_CAPACITY>::new_blocking(p.FLASH);
    let mut mgr = FlashProfileManager::new(flash);
    let boot_slot = mgr.get_boot_slot();
    {
        let mut guard = FLASH_MGR.lock().await;
        *guard = Some(mgr);
    }

    // 3. 初始化 3 个硬件 PIO 模块 (PIO0, PIO1, PIO2 对应 12 个物理通道)
    // 每个 PIO 仅装载一次 TX (11字) 与 RX (20字) 微代码 (31/32字常驻)
    let Pio {
        common: mut c0,
        sm0: sm0_0,
        sm1: sm0_1,
        sm2: sm0_2,
        sm3: sm0_3,
        ..
    } = Pio::new(p.PIO0, Irqs);

    let Pio {
        common: mut c1,
        sm0: sm1_0,
        sm1: sm1_1,
        sm2: sm1_2,
        sm3: sm1_3,
        ..
    } = Pio::new(p.PIO1, Irqs);

    let Pio {
        common: mut c2,
        sm0: sm2_0,
        sm1: sm2_1,
        sm2: sm2_2,
        sm3: sm2_3,
        ..
    } = Pio::new(p.PIO2, Irqs);

    let tx_prog = pio_programs::arinc429_tx_program();
    let rx_prog = pio_programs::arinc429_rx_program();

    static TX_0: static_cell::StaticCell<LoadedProgram<'static, PIO0>> =
        static_cell::StaticCell::new();
    static RX_0: static_cell::StaticCell<LoadedProgram<'static, PIO0>> =
        static_cell::StaticCell::new();
    static TX_1: static_cell::StaticCell<LoadedProgram<'static, PIO1>> =
        static_cell::StaticCell::new();
    static RX_1: static_cell::StaticCell<LoadedProgram<'static, PIO1>> =
        static_cell::StaticCell::new();
    static TX_2: static_cell::StaticCell<LoadedProgram<'static, PIO2>> =
        static_cell::StaticCell::new();
    static RX_2: static_cell::StaticCell<LoadedProgram<'static, PIO2>> =
        static_cell::StaticCell::new();

    let tx_0 = TX_0.init(c0.load_program(&tx_prog));
    let rx_0 = RX_0.init(c0.load_program(&rx_prog));
    let tx_1 = TX_1.init(c1.load_program(&tx_prog));
    let rx_1 = RX_1.init(c1.load_program(&rx_prog));
    let tx_2 = TX_2.init(c2.load_program(&tx_prog));
    let rx_2 = RX_2.init(c2.load_program(&rx_prog));

    // 出厂初始状态：CH0=TX High, CH1=RX High, CH2..11=RX High (被动监听安全模式)
    let dev0 = ChannelDevice::new(
        &mut c0,
        sm0_0,
        p.PIN_0,
        p.PIN_1,
        tx_0,
        rx_0,
        ChannelDirection::Tx,
        ArincSpeed::High100k,
        LabelFilter::new_accept_all(),
    );
    let dev1 = ChannelDevice::new(
        &mut c0,
        sm0_1,
        p.PIN_2,
        p.PIN_3,
        tx_0,
        rx_0,
        ChannelDirection::Rx,
        ArincSpeed::High100k,
        LabelFilter::new_accept_all(),
    );
    let dev2 = ChannelDevice::new(
        &mut c0,
        sm0_2,
        p.PIN_4,
        p.PIN_5,
        tx_0,
        rx_0,
        ChannelDirection::Rx,
        ArincSpeed::High100k,
        LabelFilter::new_accept_all(),
    );
    let dev3 = ChannelDevice::new(
        &mut c0,
        sm0_3,
        p.PIN_6,
        p.PIN_7,
        tx_0,
        rx_0,
        ChannelDirection::Rx,
        ArincSpeed::High100k,
        LabelFilter::new_accept_all(),
    );

    let dev4 = ChannelDevice::new(
        &mut c1,
        sm1_0,
        p.PIN_8,
        p.PIN_9,
        tx_1,
        rx_1,
        ChannelDirection::Rx,
        ArincSpeed::High100k,
        LabelFilter::new_accept_all(),
    );
    let dev5 = ChannelDevice::new(
        &mut c1,
        sm1_1,
        p.PIN_10,
        p.PIN_11,
        tx_1,
        rx_1,
        ChannelDirection::Rx,
        ArincSpeed::High100k,
        LabelFilter::new_accept_all(),
    );
    let dev6 = ChannelDevice::new(
        &mut c1,
        sm1_2,
        p.PIN_12,
        p.PIN_13,
        tx_1,
        rx_1,
        ChannelDirection::Rx,
        ArincSpeed::High100k,
        LabelFilter::new_accept_all(),
    );
    let dev7 = ChannelDevice::new(
        &mut c1,
        sm1_3,
        p.PIN_14,
        p.PIN_15,
        tx_1,
        rx_1,
        ChannelDirection::Rx,
        ArincSpeed::High100k,
        LabelFilter::new_accept_all(),
    );

    let dev8 = ChannelDevice::new(
        &mut c2,
        sm2_0,
        p.PIN_16,
        p.PIN_17,
        tx_2,
        rx_2,
        ChannelDirection::Rx,
        ArincSpeed::High100k,
        LabelFilter::new_accept_all(),
    );
    let dev9 = ChannelDevice::new(
        &mut c2,
        sm2_1,
        p.PIN_18,
        p.PIN_19,
        tx_2,
        rx_2,
        ChannelDirection::Rx,
        ArincSpeed::High100k,
        LabelFilter::new_accept_all(),
    );
    let dev10 = ChannelDevice::new(
        &mut c2,
        sm2_2,
        p.PIN_20,
        p.PIN_21,
        tx_2,
        rx_2,
        ChannelDirection::Rx,
        ArincSpeed::High100k,
        LabelFilter::new_accept_all(),
    );
    let dev11 = ChannelDevice::new(
        &mut c2,
        sm2_3,
        p.PIN_22,
        p.PIN_23,
        tx_2,
        rx_2,
        ChannelDirection::Rx,
        ArincSpeed::High100k,
        LabelFilter::new_accept_all(),
    );

    spawner.spawn(channel_0_task(0, dev0).unwrap());
    spawner.spawn(channel_1_task(1, dev1).unwrap());
    spawner.spawn(channel_2_task(2, dev2).unwrap());
    spawner.spawn(channel_3_task(3, dev3).unwrap());
    spawner.spawn(channel_4_task(4, dev4).unwrap());
    spawner.spawn(channel_5_task(5, dev5).unwrap());
    spawner.spawn(channel_6_task(6, dev6).unwrap());
    spawner.spawn(channel_7_task(7, dev7).unwrap());
    spawner.spawn(channel_8_task(8, dev8).unwrap());
    spawner.spawn(channel_9_task(9, dev9).unwrap());
    spawner.spawn(channel_10_task(10, dev10).unwrap());
    spawner.spawn(channel_11_task(11, dev11).unwrap());

    // 4. 若 Flash 存在设定的开机自启动 Profile，自动加载并应用
    if let Some(slot) = boot_slot {
        let mut guard = FLASH_MGR.lock().await;
        if let Some(ref mut m) = *guard {
            if let Ok(record) = m.load_profile(slot) {
                for ch in 0..ACTIVE_CHANNEL_COUNT {
                    let rec = &record.channels[ch];
                    let dir = match rec.direction {
                        1 => ChannelDirection::Tx,
                        2 => ChannelDirection::Rx,
                        _ => ChannelDirection::Disabled,
                    };
                    let high_speed = rec.high_speed != 0;
                    set_channel_direction(ch as u8, dir);
                    channel::set_channel_speed(ch as u8, high_speed);
                    let _ = channel::send_to_channel_ctrl(
                        ch as u8,
                        ChannelControlCmd::Configure {
                            direction: dir,
                            high_speed,
                        },
                    )
                    .await;
                }
            }
        }
    }

    // 保持 main 任务挂起，防止 common 等硬件资源被析构或释放
    core::future::pending::<()>().await;
}

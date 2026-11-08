#![no_std]
#![no_main]

use defmt_rtt as _;
use panic_probe as _;

pub mod channel;
pub mod coordinator;
pub mod driver;
pub mod flash_storage;
pub mod pio_programs;
pub mod usb_service;

use embassy_executor::Spawner;
use embassy_rp::bind_interrupts;
use embassy_rp::gpio::{Level, Output};
use embassy_rp::peripherals::{PIO0, PIO1, PIO2, USB};
use embassy_rp::pio::{LoadedProgram, Pio};
use embassy_rp::usb::Driver;
use embassy_usb::class::cdc_acm::{CdcAcmClass, State};
use embassy_usb::Builder;

use arinc429_core::baud::ArincSpeed;
use arinc429_core::filter::LabelFilter;

use crate::channel::ChannelDirection;
use crate::driver::ChannelDevice;
use crate::flash_storage::{FlashProfileManager, FLASH_MGR};
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

/// 板载工作指示灯任务 (PIN_25)
/// - 故障模式：常亮
/// - 配置模式 (Config)：慢闪 (1 Hz)
/// - 工作模式 (Operational)：快闪 (5 Hz)
#[embassy_executor::task]
async fn led_task(mut led: Output<'static>) {
    loop {
        if crate::channel::is_hardware_fault() {
            led.set_high();
            embassy_time::Timer::after_millis(100).await;
        } else {
            match crate::channel::operating_mode() {
                crate::channel::OperatingMode::Config => {
                    led.set_high();
                    embassy_time::Timer::after_millis(500).await;
                    led.set_low();
                    embassy_time::Timer::after_millis(500).await;
                }
                crate::channel::OperatingMode::Operational => {
                    led.set_high();
                    embassy_time::Timer::after_millis(100).await;
                    led.set_low();
                    embassy_time::Timer::after_millis(100).await;
                }
            }
        }
    }
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = embassy_rp::init(Default::default());

    // 0. 初始化板载 LED 并启动指示灯任务
    let led = Output::new(p.PIN_25, Level::Low);
    spawner.spawn(led_task(led).unwrap());

    // 1. 初始化 USB CDC ACM 虚拟串口
    let driver = Driver::new(p.USB, Irqs);
    let mut config = embassy_usb::Config::new(0x2E8A, 0x000A); // Raspberry Pi Pico CDC
    config.manufacturer = Some("Raspberry Pi / ARINC-429");
    config.product = Some("Pico2W ARINC-429 Adapter");
    config.serial_number = Some("429-012");
    config.max_power = 100;
    config.max_packet_size_0 = 64;
    // config.bcd_usb = UsbVersion::Two; // 恢复为 embassy-usb 0.6.0 默认值 (UsbVersion::TwoOne, USB 2.1)

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

    // 2. 初始化 Flash 配置管理器与开机自启动 Profile 槽位检测 (零开销内存映射只读)
    let mgr = FlashProfileManager::new();
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

    macro_rules! spawn_channels {
        ($spawner:expr, $common:expr, $tx_prog:expr, $rx_prog:expr, [
            $(($task:ident, $ch:expr, $sm:expr, $pin_a:expr, $pin_b:expr, $dir:expr)),* $(,)?
        ]) => {
            $(
                let dev = ChannelDevice::new(
                    &mut $common,
                    $sm,
                    $pin_a,
                    $pin_b,
                    $tx_prog,
                    $rx_prog,
                    $dir,
                    ArincSpeed::High100k,
                    LabelFilter::new_accept_all(),
                );
                $spawner.spawn($task($ch, dev).unwrap());
            )*
        };
    }

    // 出厂默认基态：全部 12 通道处于 Disabled 静默状态，PIO 状态机停用，引脚高阻弱下拉
    spawn_channels!(spawner, c0, tx_0, rx_0, [
        (channel_0_task, 0, sm0_0, p.PIN_0, p.PIN_1, ChannelDirection::Disabled),
        (channel_1_task, 1, sm0_1, p.PIN_2, p.PIN_3, ChannelDirection::Disabled),
        (channel_2_task, 2, sm0_2, p.PIN_4, p.PIN_5, ChannelDirection::Disabled),
        (channel_3_task, 3, sm0_3, p.PIN_6, p.PIN_7, ChannelDirection::Disabled),
    ]);

    spawn_channels!(spawner, c1, tx_1, rx_1, [
        (channel_4_task, 4, sm1_0, p.PIN_8, p.PIN_9, ChannelDirection::Disabled),
        (channel_5_task, 5, sm1_1, p.PIN_10, p.PIN_11, ChannelDirection::Disabled),
        (channel_6_task, 6, sm1_2, p.PIN_12, p.PIN_13, ChannelDirection::Disabled),
        (channel_7_task, 7, sm1_3, p.PIN_14, p.PIN_15, ChannelDirection::Disabled),
    ]);

    spawn_channels!(spawner, c2, tx_2, rx_2, [
        (channel_8_task, 8, sm2_0, p.PIN_16, p.PIN_17, ChannelDirection::Disabled),
        (channel_9_task, 9, sm2_1, p.PIN_18, p.PIN_19, ChannelDirection::Disabled),
        (channel_10_task, 10, sm2_2, p.PIN_20, p.PIN_21, ChannelDirection::Disabled),
        (channel_11_task, 11, sm2_3, p.PIN_26, p.PIN_27, ChannelDirection::Disabled),
    ]);

    // 4. 若 Flash 中存在指定的开机自启动 Profile，则原子加载并激活为 Operational 模式
    if let Some(slot) = boot_slot {
        let mut guard = FLASH_MGR.lock().await;
        if let Some(ref mut m) = *guard {
            if let Ok(record) = m.load_profile(slot) {
                crate::channel::set_operating_mode(crate::channel::OperatingMode::Operational);
                crate::coordinator::apply_profile(&record).await;
            }
        }
    }
    // 否则：保持全部 12 通道处于 Disabled 静默基态，固件处于 Config 模式等待上位机配置指令

    // 保持 main 任务挂起，防止 common 等硬件资源被析构或释放
    core::future::pending::<()>().await;
}

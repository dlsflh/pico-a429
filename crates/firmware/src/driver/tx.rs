//! ARINC 429 发送驱动 (PIO + DMA)
//!
//! 利用 RP2350 PIO 状态机输出高精度的差分双极性归零码 (BPRZ) 控制信号，
//! 并使用硬件 DMA 引擎执行非阻塞异步推送，消除 CPU 逐字中断等待。

use embassy_rp::clocks::clk_sys_freq;
use embassy_rp::dma::Channel;
use embassy_rp::gpio::Pull;
use embassy_rp::pio::{Common, Direction, Instance, PioPin, ShiftConfig, ShiftDirection, StateMachine};
use embassy_rp::Peri;

use arinc429_core::baud::ArincSpeed;

use super::calculate_pio_clkdiv;

use crate::pio_programs;

pub struct Arinc429Tx<'d, T: Instance, const SM: usize> {
    sm: StateMachine<'d, T, SM>,
    dma: Channel<'d>,
    speed: ArincSpeed,
}

impl<'d, T: Instance, const SM: usize> Arinc429Tx<'d, T, SM> {
    /// 初始化 ARINC 429 TX 驱动 (PIO + DMA)
    ///
    /// `pin_a`: TX_A 引脚 (base pin)
    /// `pin_b`: TX_B 引脚 (base pin + 1，必须在物理引脚编号上相邻)
    pub fn new(
        common: &mut Common<'d, T>,
        mut sm: StateMachine<'d, T, SM>,
        dma: Channel<'d>,
        pin_a: Peri<'d, impl PioPin>,
        pin_b: Peri<'d, impl PioPin>,
        initial_speed: ArincSpeed,
    ) -> Self {
        let prg = pio_programs::arinc429_tx_program();
        let loaded = common.load_program(&prg);

        let mut pin_a = common.make_pio_pin(pin_a);
        let mut pin_b = common.make_pio_pin(pin_b);
        pin_a.set_pull(Pull::None);
        pin_b.set_pull(Pull::None);

        sm.set_pin_dirs(Direction::Out, &[&pin_a, &pin_b]);

        let mut cfg = embassy_rp::pio::Config::default();
        cfg.use_program(&loaded, &[]);
        cfg.set_set_pins(&[&pin_a, &pin_b]);
        cfg.set_out_pins(&[&pin_a, &pin_b]);

        // LSB 优先移出 (ARINC 429 规范：Bit 1 先发)
        cfg.shift_out = ShiftConfig {
            auto_fill: false,
            direction: ShiftDirection::Right,
            threshold: 32,
        };

        let sys_clk = clk_sys_freq();
        let clkdiv = calculate_pio_clkdiv(sys_clk, initial_speed);
        cfg.clock_divider = clkdiv;

        sm.set_config(&cfg);
        sm.set_enable(true);

        Self {
            sm,
            dma,
            speed: initial_speed,
        }
    }

    /// 获取当前速率
    pub fn speed(&self) -> ArincSpeed {
        self.speed
    }

    /// 动态切换波特率 (高速 100k / 低速 12.5k)
    pub fn set_speed(&mut self, speed: ArincSpeed) {
        self.speed = speed;
        let sys_clk = clk_sys_freq();
        let clkdiv = calculate_pio_clkdiv(sys_clk, speed);
        self.sm.set_clock_divider(clkdiv);
    }

    /// 发送一个原始 32 位 ARINC 字 (通过 DMA 异步推送到 PIO TX FIFO)
    pub async fn send_raw(&mut self, raw_word: u32) {
        let buf = [raw_word];
        self.sm.tx().dma_push(&mut self.dma, &buf, false).await;
    }
}

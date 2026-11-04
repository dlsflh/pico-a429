//! ARINC 429 接收驱动 (PIO + DMA)
//!
//! 利用 RP2350 PIO 状态机捕获来自差分收发芯片的 RX_A 和 RX_B 脉冲。
//! 具备硬件级字间隙 (Gap) 检测、脉冲中心采样、字内空闲超时抗滑移机制，
//! 并使用 DMA 进行非阻塞帧拉取，集成 256 位高性能 Label 过滤器。

use embassy_rp::clocks::clk_sys_freq;
use embassy_rp::dma::Channel;
use embassy_rp::gpio::Pull;
use embassy_rp::pio::{Common, Direction, Instance, PioPin, ShiftConfig, ShiftDirection, StateMachine};
use embassy_rp::Peri;

use arinc429_core::baud::ArincSpeed;
use arinc429_core::filter::LabelFilter;

use super::calculate_pio_clkdiv;

use crate::pio_programs;

pub struct Arinc429Rx<'d, T: Instance, const SM: usize> {
    sm: StateMachine<'d, T, SM>,
    dma: Channel<'d>,
    speed: ArincSpeed,
    filter: LabelFilter,
    dma_buf: [u32; 1],
}

impl<'d, T: Instance, const SM: usize> Arinc429Rx<'d, T, SM> {
    /// 初始化 ARINC 429 RX 驱动 (PIO + DMA)
    ///
    /// `pin_a`: RX_A 引脚 (base pin)
    /// `pin_b`: RX_B 引脚 (base pin + 1，必须在物理引脚编号上相邻)
    pub fn new(
        common: &mut Common<'d, T>,
        mut sm: StateMachine<'d, T, SM>,
        dma: Channel<'d>,
        pin_a: Peri<'d, impl PioPin>,
        pin_b: Peri<'d, impl PioPin>,
        initial_speed: ArincSpeed,
        filter: LabelFilter,
    ) -> Self {
        let prg = pio_programs::arinc429_rx_program();
        let loaded = common.load_program(&prg);

        let mut pin_a = common.make_pio_pin(pin_a);
        let mut pin_b = common.make_pio_pin(pin_b);
        pin_a.set_pull(Pull::Down);
        pin_b.set_pull(Pull::Down);

        sm.set_pin_dirs(Direction::In, &[&pin_a, &pin_b]);

        let mut cfg = embassy_rp::pio::Config::default();
        cfg.use_program(&loaded, &[]);
        cfg.set_in_pins(&[&pin_a, &pin_b]);

        // LSB 优先移入 (Bit 1 最先到达，存入最低位 bit 0)，满 32 位由硬件 Autopush 自动推入 FIFO 并清空 ISR
        cfg.shift_in = ShiftConfig {
            auto_fill: true,
            direction: ShiftDirection::Right,
            threshold: 32,
        };
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
            filter,
            dma_buf: [0u32; 1],
        }
    }

    /// 动态切换波特率 (高速 100k / 低速 12.5k)
    pub fn set_speed(&mut self, speed: ArincSpeed) {
        self.speed = speed;
        let sys_clk = clk_sys_freq();
        let clkdiv = calculate_pio_clkdiv(sys_clk, speed);
        self.sm.set_clock_divider(clkdiv);
    }

    /// 获取当前波特率
    pub fn speed(&self) -> ArincSpeed {
        self.speed
    }

    /// 配置或更新 Label 过滤器
    pub fn set_filter(&mut self, filter: LabelFilter) {
        self.filter = filter;
    }

    /// 获取当前过滤器的引用
    pub fn filter(&self) -> &LabelFilter {
        &self.filter
    }

    /// 获取当前过滤器的可变引用
    pub fn filter_mut(&mut self) -> &mut LabelFilter {
        &mut self.filter
    }

    /// 异步通过 DMA 从 PIO RX FIFO 读取一个 32 位原始字 (使用持久生命周期缓冲区，规避栈悬垂)
    pub async fn recv_raw(&mut self) -> u32 {
        self.sm.rx().dma_pull(&mut self.dma, &mut self.dma_buf, false).await;
        self.dma_buf[0]
    }
}

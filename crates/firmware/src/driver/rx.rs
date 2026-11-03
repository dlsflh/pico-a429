//! ARINC 429 接收驱动 (PIO-based)
//!
//! 利用 RP2350 PIO 状态机捕获来自差分收发芯片的 RX_A 和 RX_B 脉冲。
//! 具备硬件级字间隙 (Gap) 检测与自同步抗滑移机制，并无缝集成 256 位高性能 Label 过滤器。

use embassy_rp::clocks::clk_sys_freq;
use embassy_rp::gpio::Pull;
use embassy_rp::pio::{Common, Direction, Instance, PioPin, ShiftConfig, ShiftDirection, StateMachine};
use embassy_rp::Peri;
use fixed::FixedU32;

use arinc429_core::filter::LabelFilter;
use arinc429_core::word::ArincWord;

use crate::driver::tx::ArincSpeed;
use crate::pio_programs;

/// 接收统计计数器
#[derive(Debug, Default, Clone, Copy)]
pub struct RxStats {
    pub total_received: u32,
    pub passed_filter: u32,
    pub dropped_by_filter: u32,
    pub parity_errors: u32,
}

pub struct Arinc429Rx<'d, T: Instance, const SM: usize> {
    sm: StateMachine<'d, T, SM>,
    speed: ArincSpeed,
    filter: LabelFilter,
    stats: RxStats,
}

impl<'d, T: Instance, const SM: usize> Arinc429Rx<'d, T, SM> {
    /// 初始化 ARINC 429 RX 驱动
    ///
    /// `pin_a`: RX_A 引脚 (base pin)
    /// `pin_b`: RX_B 引脚 (base pin + 1，必须在物理引脚编号上相邻)
    pub fn new(
        common: &mut Common<'d, T>,
        mut sm: StateMachine<'d, T, SM>,
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

        // LSB 优先移入 (Bit 1 最先到达，存入最低位 bit 0)，满 32 位硬件自动推入 FIFO 并清空 ISR
        cfg.shift_in = ShiftConfig {
            auto_fill: true,
            direction: ShiftDirection::Right,
            threshold: 32,
        };

        let sys_clk = clk_sys_freq();
        let clkdiv = calculate_clkdiv(sys_clk, initial_speed);
        cfg.clock_divider = clkdiv;

        sm.set_config(&cfg);
        sm.set_enable(true);

        Self {
            sm,
            speed: initial_speed,
            filter,
            stats: RxStats::default(),
        }
    }

    /// 动态切换波特率 (高速 100k / 低速 12.5k)
    pub fn set_speed(&mut self, speed: ArincSpeed) {
        self.speed = speed;
        let sys_clk = clk_sys_freq();
        let clkdiv = calculate_clkdiv(sys_clk, speed);
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

    /// 获取统计信息
    pub fn stats(&self) -> RxStats {
        self.stats
    }

    /// 清空统计计数器
    pub fn reset_stats(&mut self) {
        self.stats = RxStats::default();
    }

    /// 异步阻塞接收一个原始 32 位 ARINC 字 (从 PIO RX FIFO 拉取)
    pub async fn recv_raw(&mut self) -> u32 {
        self.sm.rx().wait_pull().await
    }

    /// 异步接收并进行自动奇偶校验与 Label 过滤
    ///
    /// 若数据被过滤器丢弃，则返回 None；若通过过滤则返回 Some((word, is_parity_valid))
    pub async fn recv_filtered(&mut self) -> Option<(ArincWord, bool)> {
        let raw = self.recv_raw().await;
        self.stats.total_received += 1;

        let word = ArincWord::from_raw(raw);
        let parity_ok = word.is_parity_valid();
        if !parity_ok {
            self.stats.parity_errors += 1;
        }

        let label = word.label();
        let sdi = word.sdi();

        if self.filter.matches(label, sdi, parity_ok) {
            self.stats.passed_filter += 1;
            Some((word, parity_ok))
        } else {
            self.stats.dropped_by_filter += 1;
            None
        }
    }
}

/// 计算 PIO 状态机时钟分频比
fn calculate_clkdiv(sys_clk: u32, speed: ArincSpeed) -> FixedU32<fixed::types::extra::U8> {
    let target_hz = match speed {
        ArincSpeed::High100k => 1_000_000,
        ArincSpeed::Low12k5 => 125_000,
    };
    let div_float = sys_clk as f64 / target_hz as f64;
    FixedU32::from_num(div_float)
}

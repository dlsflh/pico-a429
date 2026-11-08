//! 12 通道通用软件定义设备驱动 (Universal Software-Defined Channel Device)
//!
//! 支持每个通道在 TX (发送)、RX (接收) 与 Disabled (停用/低功耗) 之间动态原子切换，
//! 严格遵循“先停机 -> 清空 FIFO -> 重绑引脚与微码 -> 重启”的安全生命周期，彻底消灭硬件竞态与总线毛刺。

use embassy_futures::select::{select, Either};
use embassy_rp::clocks::clk_sys_freq;
use embassy_rp::gpio::Pull;
use embassy_rp::pio::{
    Common, Direction, Instance, LoadedProgram, Pin, PioPin, ShiftConfig, ShiftDirection,
    StateMachine,
};
use embassy_rp::Peri;
use embassy_time::Instant;

use arinc429_core::baud::ArincSpeed;
use arinc429_core::filter::LabelFilter;
use arinc429_core::word::ArincWord;

use crate::channel::{
    ChannelControlCmd, ChannelDirection, InboundArincFrame, CHANNEL_CTRL_QUEUES,
    CHANNEL_TX_QUEUES, GLOBAL_RX_PUBSUB,
};
use crate::driver::calculate_pio_clkdiv;

pub struct ChannelDevice<'d, T: Instance, const SM: usize> {
    sm: StateMachine<'d, T, SM>,
    pin_a: Pin<'d, T>,
    pin_b: Pin<'d, T>,
    tx_prog: &'d LoadedProgram<'d, T>,
    rx_prog: &'d LoadedProgram<'d, T>,
    direction: ChannelDirection,
    speed: ArincSpeed,
    filter: LabelFilter,
}

impl<'d, T: Instance, const SM: usize> ChannelDevice<'d, T, SM> {
    /// 构造通道通用硬件设备并完成引脚分配与初始模式装载
    pub fn new(
        common: &mut Common<'d, T>,
        sm: StateMachine<'d, T, SM>,
        pin_a: Peri<'d, impl PioPin>,
        pin_b: Peri<'d, impl PioPin>,
        tx_prog: &'d LoadedProgram<'d, T>,
        rx_prog: &'d LoadedProgram<'d, T>,
        initial_dir: ChannelDirection,
        initial_speed: ArincSpeed,
        filter: LabelFilter,
    ) -> Self {
        let pin_a = common.make_pio_pin(pin_a);
        let pin_b = common.make_pio_pin(pin_b);

        let mut dev = Self {
            sm,
            pin_a,
            pin_b,
            tx_prog,
            rx_prog,
            direction: ChannelDirection::Disabled,
            speed: initial_speed,
            filter,
        };

        dev.reconfigure(initial_dir, initial_speed);
        dev
    }

    /// 查询当前工作方向
    #[inline]
    pub fn direction(&self) -> ChannelDirection {
        self.direction
    }

    /// 查询当前波特率
    #[inline]
    pub fn speed(&self) -> ArincSpeed {
        self.speed
    }

    /// 获取过滤器只读引用
    #[inline]
    pub fn filter(&self) -> &LabelFilter {
        &self.filter
    }

    /// 获取过滤器可变引用
    #[inline]
    pub fn filter_mut(&mut self) -> &mut LabelFilter {
        &mut self.filter
    }

    /// 动态原子重构通道方向与速率 (Stop -> Clear -> Configure -> Start)
    pub fn reconfigure(&mut self, dir: ChannelDirection, speed: ArincSpeed) {
        // 1. 停用状态机并清退 FIFO
        self.sm.set_enable(false);
        self.direction = dir;
        self.speed = speed;

        let sys_clk = clk_sys_freq();
        let clkdiv = calculate_pio_clkdiv(sys_clk, speed);

        match dir {
            ChannelDirection::Tx => {
                // 配置物理引脚为无上拉输出或在 Config 模式下保持高阻输入
                self.pin_a.set_pull(Pull::None);
                self.pin_b.set_pull(Pull::None);

                let mut cfg = embassy_rp::pio::Config::default();
                cfg.use_program(self.tx_prog, &[]);
                cfg.set_set_pins(&[&self.pin_a, &self.pin_b]);
                cfg.set_out_pins(&[&self.pin_a, &self.pin_b]);

                // LSB 优先移出 (Bit 1 先发)
                cfg.shift_out = ShiftConfig {
                    auto_fill: false,
                    direction: ShiftDirection::Right,
                    threshold: 32,
                };
                cfg.clock_divider = clkdiv;

                self.sm.set_config(&cfg);

                if !crate::channel::is_config_mode() {
                    self.sm
                        .set_pin_dirs(Direction::Out, &[&self.pin_a, &self.pin_b]);
                    self.sm.restart();
                    self.sm.set_enable(true);
                } else {
                    self.sm
                        .set_pin_dirs(Direction::In, &[&self.pin_a, &self.pin_b]);
                    self.sm.set_enable(false);
                }
            }
            ChannelDirection::Rx => {
                // 配置物理引脚为弱下拉输入 (适配差分输入零电平)
                self.pin_a.set_pull(Pull::Down);
                self.pin_b.set_pull(Pull::Down);
                self.sm
                    .set_pin_dirs(Direction::In, &[&self.pin_a, &self.pin_b]);

                let mut cfg = embassy_rp::pio::Config::default();
                cfg.use_program(self.rx_prog, &[]);
                cfg.set_in_pins(&[&self.pin_a, &self.pin_b]);

                // LSB 优先自动移入
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
                cfg.clock_divider = clkdiv;

                self.sm.set_config(&cfg);

                if !crate::channel::is_config_mode() {
                    self.sm.restart();
                    self.sm.set_enable(true);
                } else {
                    self.sm.set_enable(false);
                }
            }
            ChannelDirection::Disabled => {
                self.pin_a.set_pull(Pull::None);
                self.pin_b.set_pull(Pull::None);
                self.sm
                    .set_pin_dirs(Direction::In, &[&self.pin_a, &self.pin_b]);
                self.sm.set_enable(false);
            }
        }
    }

    /// 暂停物理收发工作 (切换引脚为高阻输入以确保物理总线静默，并清空 PIO 硬件 FIFO 与时钟计数器)
    pub fn pause(&mut self) {
        self.sm.set_enable(false);
        self.sm.restart();
        if self.direction == ChannelDirection::Tx {
            self.sm
                .set_pin_dirs(Direction::In, &[&self.pin_a, &self.pin_b]);
        }
    }

    /// 恢复物理收发工作 (必须受全局 OperatingMode 严格把关)
    pub fn resume(&mut self) {
        if self.direction != ChannelDirection::Disabled && !crate::channel::is_config_mode() {
            if self.direction == ChannelDirection::Tx {
                self.sm
                    .set_pin_dirs(Direction::Out, &[&self.pin_a, &self.pin_b]);
            }
            self.sm.restart();
            self.sm.set_enable(true);
        } else {
            self.sm.set_enable(false);
            if self.direction == ChannelDirection::Tx {
                self.sm
                    .set_pin_dirs(Direction::In, &[&self.pin_a, &self.pin_b]);
            }
        }
    }

    /// 处理单条通道控制命令
    fn handle_cmd(&mut self, cmd: ChannelControlCmd) {
        match cmd {
            ChannelControlCmd::Configure {
                direction,
                high_speed,
                filter,
            } => {
                let speed = if high_speed {
                    ArincSpeed::High100k
                } else {
                    ArincSpeed::Low12k5
                };
                if let Some(f) = filter {
                    self.filter = f;
                }
                self.reconfigure(direction, speed);
            }
            ChannelControlCmd::Filter(action) => match action {
                arinc429_core::protocol::FilterAction::Allow(label) => {
                    let _ = self.filter.allow_label(label);
                }
                arinc429_core::protocol::FilterAction::Block(label) => {
                    let _ = self.filter.block_label(label);
                }
                arinc429_core::protocol::FilterAction::Clear => {
                    self.filter.reset();
                }
            },
            ChannelControlCmd::SetFullFilter(filter) => {
                self.filter = filter;
            }
            ChannelControlCmd::Pause => {
                self.pause();
            }
            ChannelControlCmd::Resume => {
                self.resume();
            }
        }
    }

    /// 通道异步主循环：完全响应驱动，零轮询开销
    pub async fn run(&mut self, channel_id: u8) -> ! {
        let publisher = GLOBAL_RX_PUBSUB.immediate_publisher();

        loop {
            // 尽最大努力先排空滞留的控制命令
            while let Ok(cmd) = CHANNEL_CTRL_QUEUES[channel_id as usize].try_receive() {
                self.handle_cmd(cmd);
            }

            let is_operational =
                !crate::channel::is_config_mode() && self.direction != ChannelDirection::Disabled;

            if !is_operational {
                // 挂起或处于 Config 维护模式或停用状态：休眠等待控制命令激活
                let cmd = CHANNEL_CTRL_QUEUES[channel_id as usize].receive().await;
                self.handle_cmd(cmd);
            } else if self.direction == ChannelDirection::Tx {
                // 发送模式：等待发送字或控制命令
                match select(
                    CHANNEL_TX_QUEUES[channel_id as usize].receive(),
                    CHANNEL_CTRL_QUEUES[channel_id as usize].receive(),
                )
                .await
                {
                    Either::First(raw_word) => {
                        self.sm.tx().wait_push(raw_word).await;
                    }
                    Either::Second(cmd) => {
                        self.handle_cmd(cmd);
                    }
                }
            } else {
                // 接收模式：等待接收字或控制命令
                match select(
                    self.sm.rx().wait_pull(),
                    CHANNEL_CTRL_QUEUES[channel_id as usize].receive(),
                )
                .await
                {
                    Either::First(raw) => {
                        let word = ArincWord::from_raw(raw);
                        let parity_ok = word.is_parity_valid();
                        let now_us = Instant::now().as_micros() as u32;

                        let frame = InboundArincFrame {
                            channel_id,
                            timestamp_us: now_us,
                            word,
                            parity_ok,
                            high_speed: self.speed == ArincSpeed::High100k,
                        };

                        if self.filter.matches(word.label(), parity_ok) {
                            publisher.publish_immediate(frame);
                        }
                    }
                    Either::Second(cmd) => {
                        self.handle_cmd(cmd);
                    }
                }
            }
        }
    }
}

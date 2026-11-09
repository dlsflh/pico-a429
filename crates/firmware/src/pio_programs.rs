//! PIO 汇编程序定义 (RP2350 / RP2040)
//!
//! 在静态通道方向拓扑下，TX 与 RX 程序独立运行于专用状态机：
//! - 发送驱动 (TX: 11 指令): 统一运行于 20 周期/bit (2 MHz / 250 kHz)，严格满足 50% BPRZ 占空比与 4.0-bit 间隙；
//! - 接收驱动 (RX: 20 指令): 统一运行于 20 周期/bit (2 MHz / 250 kHz)，具备 3.4-bit 自愈间隙检测与脉冲中段二次采样。
//! - 单块 PIO 内存总计 11 + 20 = 31 / 32 指令，完美常驻！
//!
//! 注：微代码已固化为常量机器码（Bitwise Identical），彻底消除编译期宏展开与语法解析器开销。

/// ARINC 429 TX 机器码 (严格占用 11 条指令，运行于 1 bit = 20 PIO 周期)
pub const ARINC429_TX_INSTRUCTIONS: [u16; 11] = [
    0x80a0, // [0]  get_word:   pull block
    0xe03f, // [1]              set x, 31
    0x6041, // [2]  bit_loop:   out y, 1
    0x0066, // [3]              jmp !y, send_zero
    0xe801, // [4]  send_one:   set pins, 0b01 [8]
    0x0007, // [5]              jmp emit_null
    0xe902, // [6]  send_zero:  set pins, 0b10 [9]
    0xe600, // [7]  emit_null:  set pins, 0b00 [6]
    0x0042, // [8]              jmp x--, bit_loop
    0xe04f, // [9]  send_gap:   set y, 15
    0x048a, // [10] gap_loop:   jmp y--, gap_loop [4]
];

/// ARINC 429 差分 BPRZ 发送状态机程序 (严格占用 11 条指令，运行于 1 bit = 20 PIO 周期)
///
/// 严格满足 1 bit = 20 个 PIO 周期 (100k 时目标主频 2 MHz, 12.5k 时目标主频 250 kHz):
/// - 逻辑 1: 前半周 10 周期输出 0b01 (TX_A 高, TX_B 低), 后半周 10 周期输出 0b00 (Null)
/// - 逻辑 0: 前半周 10 周期输出 0b10 (TX_A 低, TX_B 高), 后半周 10 周期输出 0b00 (Null)
/// - 严格保证 50.0% 双极性归零码占空比
/// - 字间隙: 发送完毕后自动延时 80 周期 (严格等于 4.0 个 bit 周期的标准 ARINC 429 Inter-word Gap)
pub fn arinc429_tx_program() -> pio::Program<32> {
    let mut code = pio::ArrayVec::new();
    for &insn in &ARINC429_TX_INSTRUCTIONS {
        code.push(insn);
    }
    pio::Program {
        code,
        origin: None,
        wrap: pio::Wrap { source: 10, target: 0 },
        side_set: pio::SideSet::default(),
        version: pio::PioVersion::V0,
    }
}

/// ARINC 429 RX 机器码 (占用 20 条指令，运行于 1 bit = 20 PIO 周期)
pub const ARINC429_RX_INSTRUCTIONS: [u16; 20] = [
    0xe030, // [0]  reset_gap:        set x, 16
    0xa0e0, // [1]  gap_or_next_bit:  mov osr, pins
    0x6042, // [2]                    out y, 2
    0x0065, // [3]                    jmp !y, count_null
    0x000a, // [4]                    jmp verify_pulse
    0x0041, // [5]  count_null:       jmp x--, gap_or_next_bit
    0xa0c3, // [6]  sync_ready:       mov isr, null
    0xa0e0, // [7]  wait_first_bit:   mov osr, pins
    0x6042, // [8]                    out y, 2
    0x0067, // [9]                    jmp !y, wait_first_bit
    0xa0e0, // [10] verify_pulse:     mov osr, pins
    0x6042, // [11]                   out y, 2
    0x0060, // [12]                   jmp !y, reset_gap
    0x4041, // [13]                   in y, 1
    0xe030, // [14]                   set x, 16
    0xa0e0, // [15] wait_null:        mov osr, pins  <- wrap target
    0x6042, // [16]                   out y, 2
    0x0060, // [17]                   jmp !y, reset_gap
    0x004f, // [18]                   jmp x--, wait_null
    0xa0c3, // [19]                   mov isr, null  <- wrap source
];

/// ARINC 429 差分双引脚极性采样与自愈间隙接收状态机程序 (占用 20 条指令，运行于 1 bit = 20 PIO 周期)
///
/// 特性：
/// 1. 20 周期/bit 架构 (100k -> 2 MHz, 12.5k -> 250 kHz): 脉冲窗口扩展至 10 周期，二次采样严格落在 4.1~8.0 周期中段；
/// 2. 字间 Gap 与字内断帧超时合一: 3.4-bit 低电平窗口 (`gap_or_next_bit`) 自动判别，空闲期 0% 漏帧，杜绝残帧位滑移；
/// 3. 支持严苛 4.0-bit 背靠背传输: 在下降沿后 3.6-bit (72 周期) 提前就位，零吞帧风险；
/// 4. 常高故障 (Line Fault) 闭环抑制: 利用 `.wrap_target` 原地清空 ISR 并挂起等待故障解除，杜绝垃圾帧雪崩。
pub fn arinc429_rx_program() -> pio::Program<32> {
    let mut code = pio::ArrayVec::new();
    for &insn in &ARINC429_RX_INSTRUCTIONS {
        code.push(insn);
    }
    pio::Program {
        code,
        origin: None,
        wrap: pio::Wrap { source: 19, target: 15 },
        side_set: pio::SideSet::default(),
        version: pio::PioVersion::V0,
    }
}

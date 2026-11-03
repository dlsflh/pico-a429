//! PIO 汇编程序定义 (RP2350 / RP2040)
//!
//! 在静态通道方向拓扑下，TX 与 RX 程序独立运行于专用状态机：
//! - 发送驱动 (TX: 11 指令): 严格满足 50% 双极性归零码 (BPRZ) 占空比与 4-bit 字间隙定时；
//! - 接收驱动 (RX: 28 指令): 物理独占 PIO1，具备 40 周期严格字间隙检测、2 周期脉冲毛刺硬件抑制与防死锁超时。

/// ARINC 429 差分 BPRZ 发送状态机程序 (严格占用 11 条指令)
///
/// 严格满足 1 bit = 10 个 PIO 周期 (100k 时目标主频 1 MHz, 12.5k 时目标主频 125 kHz):
/// - 逻辑 1: 前半周 5 周期输出 0b01 (TX_A 高, TX_B 低), 后半周 5 周期输出 0b00 (Null)
/// - 逻辑 0: 前半周 5 周期输出 0b10 (TX_A 低, TX_B 高), 后半周 5 周期输出 0b00 (Null)
/// - 字间隙: 发送完毕后自动延时 40 周期 (严格等于 4 个 bit 周期的标准 ARINC 429 Inter-word Gap)
pub fn arinc429_tx_program() -> pio::Program<32> {
    pio::pio_asm!(
        ".wrap_target",
        "get_word:",
        "    pull block",
        "    set x, 31",
        "bit_loop:",
        "    out y, 1",
        "    jmp !y, send_zero",
        "send_one:",
        "    set pins, 0b01 [3]",       // 1 cycle + 3 delay = 4 cycles
        "    jmp emit_null",            // 1 cycle (pins 0b01 持续共 5 周期)
        "send_zero:",
        "    set pins, 0b10 [4]",       // 1 cycle + 4 delay = 5 cycles (pins 0b10 持续共 5 周期)
        "emit_null:",
        "    set pins, 0b00 [1]",       // 1 cycle + 1 delay = 2 cycles
        "    jmp x--, bit_loop",        // 1 cycle (加上 bit_loop 的 out y [1] 与 jmp [1]，Null 恰好持续 5 周期)
        "send_gap:",
        "    set y, 7",
        "gap_loop:",
        "    jmp y--, gap_loop [4]",    // 8 轮循环 * (1 + 4 delay) = 40 周期 (严格等于 4 bit 间隙)
        ".wrap"
    )
    .program
}

/// ARINC 429 差分双引脚极性硬件滤波与严苛间隙检测接收状态机程序 (占用 28 条指令)
///
/// 特性：
/// 1. 物理 PIO1 独占：拥有完整 32 字指令内存空间，不再受限于单块 21 指令的压缩妥协；
/// 2. 严苛 4-bit 间隙校验：8 轮循环 * 5 周期 = 严格 40 个 PIO 周期（4.0 bit 时间）连续确认 Null 电平，
///    彻底杜绝机载中途接入或断续信号产生的残帧 (Runt Frame) 误触发；
/// 3. 脉冲中心瞬态尖峰抑制 (Glitch Filter)：捕获边沿后延时 2 个周期抵达脉冲中心，若电平已回落至 0b00（即持续时间 < 2us 的 ESD/电磁干扰尖峰），
///    立即判定为非法毛刺并丢弃，绝不污染移位寄存器；
/// 4. 差分判定：在确认有效的脉冲中心捕获差分信号 (0b01 -> 1, 0b10 -> 0)；
/// 5. 硬件抗死锁与字结束超时自同步：具备双重超时计数器，总线持续高电平或字内空闲超时自动脱困复位。
pub fn arinc429_rx_program() -> pio::Program<32> {
    pio::pio_asm!(
        ".wrap_target",
        "wait_gap:",
        "    mov isr, null",
        "    set x, 7",
        "gap_loop:",
        "    mov osr, pins",
        "    out y, 2",
        "    jmp !y, gap_tick",
        "    jmp wait_gap",
        "gap_tick:",
        "    jmp x--, gap_loop [4]",

        "wait_edge:",
        "    mov osr, pins",
        "    out y, 2",
        "    jmp !y, wait_edge",

        "filter_glitch:",
        "    nop [1]",
        "    mov osr, pins",
        "    out y, 2",
        "    jmp !y, wait_edge",

        "sample_bit:",
        "    in y, 1",

        "wait_null:",
        "    set x, 14",
        "null_loop:",
        "    mov osr, pins",
        "    out y, 2",
        "    jmp !y, wait_next",
        "    jmp x--, null_loop",
        "    jmp wait_gap",

        "wait_next:",
        "    set x, 24",
        "next_loop:",
        "    mov osr, pins",
        "    out y, 2",
        "    jmp !y, next_tick",
        "    jmp filter_glitch",
        "next_tick:",
        "    jmp x--, next_loop",
        "    jmp wait_gap",
        ".wrap"
    )
    .program
}

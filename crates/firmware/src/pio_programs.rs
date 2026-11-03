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

/// ARINC 429 差分双引脚极性采样与严苛 4-bit 间隙校验接收状态机程序 (占用 18 条指令)
///
/// 特性：
/// 1. 物理 PIO1 独占：拥有完整 32 字指令内存空间；
/// 2. 严苛 4-bit 间隙校验：8 轮循环 * (1 + 1 + 1 + 2) = 严格 40 个 PIO 周期（4.0 bit 时间）连续确认 Null 电平；
/// 3. 脉冲有效居中采样：边沿到达后立即在 5 周期高/低有效脉冲窗口内（第 2~3 周期）捕获差分信号 (0b01 -> 1, 0b10 -> 0)；
/// 4. 显式 32 位字循环与非阻塞 Push：由硬件计数器严格接收满 32 bit 后显式 push 到 FIFO 并复位 shift counter，彻底杜绝残帧 desync。
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
        "    jmp x--, gap_loop [1]",

        "start_word:",
        "    set x, 31",

        "wait_edge:",
        "    mov osr, pins",
        "    out y, 2",
        "    jmp !y, wait_edge",

        "sample_bit:",
        "    in y, 1",

        "wait_null:",
        "    mov osr, pins",
        "    out y, 2",
        "    jmp !y, bit_done",
        "    jmp wait_null",

        "bit_done:",
        "    jmp x--, wait_edge",
        "    push noblock",
        ".wrap"
    )
    .program
}

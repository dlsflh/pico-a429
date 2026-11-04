//! PIO 汇编程序定义 (RP2350 / RP2040)
//!
//! 在静态通道方向拓扑下，TX 与 RX 程序独立运行于专用状态机：
//! - 发送驱动 (TX: 11 指令): 统一运行于 20 周期/bit (2 MHz / 250 kHz)，严格满足 50% BPRZ 占空比与 4.0-bit 间隙；
//! - 接收驱动 (RX: 20 指令): 统一运行于 20 周期/bit (2 MHz / 250 kHz)，具备 3.4-bit 自愈间隙检测与脉冲中段二次采样。
//! - 单块 PIO 内存总计 11 + 20 = 31 / 32 指令，完美常驻！

/// ARINC 429 差分 BPRZ 发送状态机程序 (严格占用 11 条指令，运行于 1 bit = 20 PIO 周期)
///
/// 严格满足 1 bit = 20 个 PIO 周期 (100k 时目标主频 2 MHz, 12.5k 时目标主频 250 kHz):
/// - 逻辑 1: 前半周 10 周期输出 0b01 (TX_A 高, TX_B 低), 后半周 10 周期输出 0b00 (Null)
/// - 逻辑 0: 前半周 10 周期输出 0b10 (TX_A 低, TX_B 高), 后半周 10 周期输出 0b00 (Null)
/// - 严格保证 50.0% 双极性归零码占空比
/// - 字间隙: 发送完毕后自动延时 80 周期 (严格等于 4.0 个 bit 周期的标准 ARINC 429 Inter-word Gap)
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
        "    set pins, 0b01 [8]",       // 1 cycle + 8 delay = 9 cycles
        "    jmp emit_null",            // 1 cycle (pins 0b01 持续共 10 周期)
        "send_zero:",
        "    set pins, 0b10 [9]",       // 1 cycle + 9 delay = 10 cycles (pins 0b10 持续共 10 周期)
        "emit_null:",
        "    set pins, 0b00 [6]",       // 1 cycle + 6 delay = 7 cycles
        "    jmp x--, bit_loop",        // 1 cycle (加上 bit_loop 的 out y [1] 与 jmp [1]，Null 恰好持续 10 周期)
        "send_gap:",
        "    set y, 15",
        "gap_loop:",
        "    jmp y--, gap_loop [4]",    // 16 轮循环 * (1 + 4 delay) = 80 周期 (严格等于 4.0 bit 间隙)
        ".wrap"
    )
    .program
}

/// ARINC 429 差分双引脚极性采样与自愈间隙接收状态机程序 (占用 20 条指令，运行于 1 bit = 20 PIO 周期)
///
/// 特性：
/// 1. 20 周期/bit 架构 (100k -> 2 MHz, 12.5k -> 250 kHz): 脉冲窗口扩展至 10 周期，二次采样严格落在 4.1~8.0 周期中段；
/// 2. 字间 Gap 与字内断帧超时合一: 3.4-bit 低电平窗口 (`gap_or_next_bit`) 自动判别，空闲期 0% 漏帧，杜绝残帧位滑移；
/// 3. 支持严苛 4.0-bit 背靠背传输: 在下降沿后 3.6-bit (72 周期) 提前就位，零吞帧风险；
/// 4. 常高故障 (Line Fault) 闭环抑制: 利用 `.wrap_target` 原地清空 ISR 并挂起等待故障解除，杜绝垃圾帧雪崩。
pub fn arinc429_rx_program() -> pio::Program<32> {
    pio::pio_asm!(
        "reset_gap:",
        "    set x, 16",                  // [0] 17 轮 * 4 周期 = 68 周期 (3.4 bits 低电平计时)
        "gap_or_next_bit:",
        "    mov osr, pins",              // [1] (1) T0: 首次采样
        "    out y, 2",                   // [2] (1) T0+1
        "    jmp !y, count_null",         // [3] (1) T0+2: 仍为 00，继续累计低电平时间
        "    jmp verify_pulse",           // [4] (1) T0+3: <3.4 bits 内出现边沿 -> 当前帧的下一位！
        "count_null:",
        "    jmp x--, gap_or_next_bit",   // [5] (1) 4 周期轮询环

        // --- 低电平已持续 ~3.7 bits，帧间 Gap 验证通过！---
        "sync_ready:",
        "    mov isr, null",              // [6] 清空任何未满 32 位的残帧/杂波，复位硬件移位计数器
        "wait_first_bit:",
        "    mov osr, pins",              // [7] (1) T0: 帧首位采样
        "    out y, 2",                   // [8] (1) T0+1
        "    jmp !y, wait_first_bit",     // [9] (1) 无限死等新帧 Bit 1 (空闲期 0% 漏帧！)

        "verify_pulse:",
        "    mov osr, pins",              // [10] (1) 二次采样 (4.1~8.0 周期处)，处于 10 周期脉冲中段
        "    out y, 2",                   // [11] (1)
        "    jmp !y, reset_gap",          // [12] (1) 若已掉回 00 则为窄毛刺，跳回 reset_gap
        "    in y, 1",                    // [13] (1) 移入有效位 (满 32 位由硬件 Autopush 推入 FIFO)

        "    set x, 16",                  // [14] (1) 归零看门狗 (68 周期上限)
        ".wrap_target",
        "wait_null:",
        "    mov osr, pins",              // [15] (1)
        "    out y, 2",                   // [16] (1)
        "    jmp !y, reset_gap",          // [17] (1) 成功归零，跳去测量低电平持续时间
        "    jmp x--, wait_null",         // [18] (1) 等待归零
        "    mov isr, null",              // [19] (1) 常高故障超时：清空残帧，由 .wrap 跳回 wait_null 挂起
        ".wrap"
    )
    .program
}

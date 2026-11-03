//! PIO 汇编程序定义
//!
//! 提供符合 ARINC 429 规范的高精度发送 (TX) 与带间隙自同步/抗高噪声的接收 (RX) 状态机微代码。

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
        "    set pins, 0b01 [3]",
        "    jmp emit_null",
        "send_zero:",
        "    set pins, 0b10 [4]",
        "emit_null:",
        "    set pins, 0b00 [1]",
        "    jmp x--, bit_loop",
        "send_gap:",
        "    set y, 7",
        "gap_loop:",
        "    nop [3]",
        "    jmp y--, gap_loop",
        ".wrap"
    )
    .program
}

pub fn arinc429_rx_program() -> pio::Program<32> {
    pio::pio_asm!(
        ".wrap_target",
        "wait_gap:",
        "    mov isr, null",
        "    set y, 5",
        "gap_loop:",
        "    mov osr, pins",
        "    out x, 2",
        "    jmp !x, gap_tick",
        "    jmp wait_gap",
        "gap_tick:",
        "    jmp y--, gap_loop",
        "start_word:",
        "    set x, 31",
        "wait_pulse:",
        "    mov osr, pins",
        "    out y, 2",
        "    jmp !y, wait_pulse",
        "    in pins, 1",
        "wait_null:",
        "    mov osr, pins",
        "    out y, 2",
        "    jmp !y, bit_done",
        "    jmp wait_null",
        "bit_done:",
        "    jmp x--, wait_pulse",
        ".wrap"
    )
    .program
}

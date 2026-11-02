//! PIO 汇编程序定义
//!
//! 提供 ARINC 429 的发送 (TX) 与带间隙自同步的接收 (RX) 状态机微代码。

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
        "    set pins, 0b01 [1]",
        "    jmp finish_bit",
        "send_zero:",
        "    set pins, 0b10 [2]",
        "finish_bit:",
        "    set pins, 0b00 [3]",
        "    jmp x--, bit_loop",
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
        "    set y, 15",
        "gap_loop:",
        "    mov osr, pins",
        "    out x, 2",
        "    jmp !x, gap_count_down",
        "    jmp wait_gap",
        "gap_count_down:",
        "    jmp y--, gap_loop",
        "start_word:",
        "    set x, 31",
        "bit_recv_loop:",
        "wait_pulse:",
        "    mov osr, pins",
        "    out y, 2",
        "    jmp !y, wait_pulse",
        "    nop [1]",
        "    in pins, 1",
        "wait_return_null:",
        "    mov osr, pins",
        "    out y, 2",
        "    jmp !y, pulse_done",
        "    jmp wait_return_null",
        "pulse_done:",
        "    jmp x--, bit_recv_loop",
        "    push noblock",
        ".wrap"
    )
    .program
}

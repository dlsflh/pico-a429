MEMORY {
    FLASH : ORIGIN = 0x10000000, LENGTH = 4096K
    RAM   : ORIGIN = 0x20000000, LENGTH = 512K
}

SECTIONS {
    .start_block (ADDR(.vector_table) + SIZEOF(.vector_table)) : ALIGN(4) {
        KEEP(*(.start_block));
        . = ALIGN(4);
    } > FLASH
} INSERT BEFORE .text;

_stext = ALIGN(ADDR(.start_block) + SIZEOF(.start_block), 8);

PROVIDE(_start = Reset);

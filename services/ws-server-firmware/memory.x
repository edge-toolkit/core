/* The RP2350's memory as the Pico 2 W wires it: 4 MiB of flash and 512 KiB of striped SRAM. */
MEMORY {
    FLASH : ORIGIN = 0x10000000, LENGTH = 4096K
    RAM : ORIGIN = 0x20000000, LENGTH = 512K
    SRAM4 : ORIGIN = 0x20080000, LENGTH = 4K
    SRAM5 : ORIGIN = 0x20081000, LENGTH = 4K
}

/* The boot ROM looks for the image definition block within the first 4 KiB of flash, after the vector table. */
SECTIONS {
    .start_block : ALIGN(4)
    {
        __start_block_addr = .;
        KEEP(*(.start_block));
        KEEP(*(.boot_info));
    } > FLASH
} INSERT AFTER .vector_table;

/* Code follows the block, on the 8-byte boundary cortex-m-rt aligns .text to. */
_stext = ALIGN(ADDR(.start_block) + SIZEOF(.start_block), 8);

/* Binary info entries, which picotool reads to describe the image. */
SECTIONS {
    .bi_entries : ALIGN(4)
    {
        __bi_entries_start = .;
        KEEP(*(.bi_entries));
        . = ALIGN(4);
        __bi_entries_end = .;
    } > FLASH
} INSERT AFTER .text;

/* The block that closes the image's block loop. */
SECTIONS {
    .end_block : ALIGN(4)
    {
        __end_block_addr = .;
        KEEP(*(.end_block));
    } > FLASH
} INSERT AFTER .uninit;

PROVIDE(start_to_end = __end_block_addr - __start_block_addr);
PROVIDE(end_to_start = __start_block_addr - __end_block_addr);

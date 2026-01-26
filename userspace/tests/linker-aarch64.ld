/* ARM64 Userspace program linker script */
ENTRY(_start)

SECTIONS {
    /* Start at user-accessible region (0x4100_0000 per kernel MMU setup) */
    . = 0x41000000;
    
    .text : ALIGN(4K) {
        *(.text .text.*)
    }
    
    .rodata : ALIGN(4K) {
        *(.rodata .rodata.*)
    }
    
    .data : ALIGN(4K) {
        *(.data .data.*)
    }
    
    .bss : ALIGN(4K) {
        *(.bss .bss.*)
    }
    
    /* Stack allocated separately by kernel */
}

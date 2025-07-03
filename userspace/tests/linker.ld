/* Userspace program linker script */
ENTRY(_start)

SECTIONS {
    . = 0x10000000; /* Start at 256MB (userspace area) */
    
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
    
    /* Stack grows down from higher address (we allocate stack separately) */
    /* . = 0x1000000; */
    /* _stack_top = .; */
}
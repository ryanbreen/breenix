/* Userspace program linker script */
ENTRY(_start)

SECTIONS {
    . = 0x40000000; /* Start at 1GB (userspace area) - TEMPORARY FIX for PML4[0] conflict */
    
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
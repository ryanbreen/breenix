/* ARM64 Userspace program linker script */
ENTRY(_start)

SECTIONS {
    /* ARM64 userspace starts at 4KB (page 1) */
    /* Page 0 is left unmapped to catch null pointer dereferences */
    . = 0x1000;

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

    /* Stack will be allocated separately by the kernel */
}

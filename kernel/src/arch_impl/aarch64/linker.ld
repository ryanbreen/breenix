/*
 * ARM64 Breenix Kernel Linker Script
 *
 * Memory layout for ARM64 (QEMU virt machine or Parallels).
 * For QEMU virt: RAM starts at 0x4000_0000
 * Kernel loaded at 0x4008_0000 to leave room for DTB
 */

OUTPUT_FORMAT("elf64-littleaarch64")
OUTPUT_ARCH(aarch64)
ENTRY(_start)

SECTIONS
{
    /* Kernel load address - 1GB + 512KB */
    . = 0x40080000;

    /* Boot code must come first - contains _start */
    .text.boot : ALIGN(4K) {
        KEEP(*(.text.boot))
    }

    /* Exception vector table - MUST be 2KB (0x800) aligned */
    . = ALIGN(2K);
    .text.vectors : {
        KEEP(*(.text.vectors))
    }

    /* Main code section */
    .text : ALIGN(4K) {
        *(.text .text.*)
    }

    /* Read-only data */
    .rodata : ALIGN(4K) {
        *(.rodata .rodata.*)
    }

    /* Initialized data */
    .data : ALIGN(4K) {
        *(.data .data.*)
    }

    /* BSS - zero initialized */
    .bss : ALIGN(4K) {
        __bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        __bss_end = .;
    }

    /* Stack section - 64KB stack */
    .bss.stack (NOLOAD) : ALIGN(16) {
        __stack_bottom = .;
        . = . + 65536;  /* Reserve 64KB for stack */
        __stack_top = .;
    }

    /DISCARD/ : {
        *(.comment)
        *(.note*)
        *(.eh_frame*)
    }

    _end = .;
}

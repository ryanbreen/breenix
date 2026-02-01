/*
 * ARM64 Breenix Kernel Linker Script (High-Half Kernel)
 *
 * Memory layout for ARM64 (QEMU virt machine or Parallels).
 * Physical RAM starts at 0x4000_0000
 * Kernel is loaded at 0x4008_0000 to leave room for DTB
 *
 * Boot code stays in low physical memory and sets up the MMU.
 * The main kernel is linked in the higher half and mapped via TTBR1.
 */

OUTPUT_FORMAT("elf64-littleaarch64")
OUTPUT_ARCH(aarch64)
ENTRY(_start)

/* Physical load base and virtual base for high-half kernel */
KERNEL_PHYS_BASE = 0x40080000;
KERNEL_VIRT_BASE = 0xFFFF000000000000; /* HHDM base */
VMLINUX_OFFSET   = KERNEL_VIRT_BASE;

SECTIONS
{
    /* Low physical boot code (executes before MMU is enabled) */
    . = KERNEL_PHYS_BASE;

    /* Boot code must come first - contains _start */
    .text.boot : ALIGN(4K) {
        KEEP(*(.text.boot))
    }

    /* Boot exception vectors (low) - 2KB aligned */
    . = ALIGN(2K);
    .text.vectors.boot : {
        KEEP(*(.text.vectors.boot))
    }

    /* Boot-time BSS (low, includes boot stack + early tables) */
    .bss.boot (NOLOAD) : ALIGN(16) {
        __boot_bss_start = .;
        *(.bss.boot .bss.boot.*)
        __boot_bss_end = .;
    }

    __kernel_phys_start = .;

    /* Switch to higher-half VMA for the main kernel (direct map) */
    . = KERNEL_VIRT_BASE + .;
    __kernel_virt_start = .;

    /* Runtime exception vector table (high) - MUST be 2KB aligned */
    . = ALIGN(2K);
    .text.vectors : AT(ADDR(.text.vectors) - VMLINUX_OFFSET) {
        KEEP(*(.text.vectors))
    }

    /* Main code section */
    . = ALIGN(4K);
    .text : AT(ADDR(.text) - VMLINUX_OFFSET) {
        *(.text .text.*)
    }

    /* Read-only data */
    . = ALIGN(4K);
    .rodata : AT(ADDR(.rodata) - VMLINUX_OFFSET) {
        *(.rodata .rodata.*)
    }

    /* Initialized data */
    . = ALIGN(4K);
    .data : AT(ADDR(.data) - VMLINUX_OFFSET) {
        *(.data .data.*)
    }

    /* BSS - zero initialized */
    . = ALIGN(4K);
    __bss_start = .;
    .bss (NOLOAD) : AT(ADDR(.bss) - VMLINUX_OFFSET) {
        *(.bss .bss.*)
        *(COMMON)
    }
    __bss_end = .;

    /* Stack section - 64KB stack */
    . = ALIGN(16);
    .bss.stack (NOLOAD) : AT(ADDR(.bss.stack) - VMLINUX_OFFSET) {
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

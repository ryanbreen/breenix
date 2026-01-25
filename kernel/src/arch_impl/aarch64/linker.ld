/*
 * ARM64 Breenix Kernel Linker Script
 * 
 * Memory layout for ARM64 UEFI boot.
 * The kernel is loaded by the bootloader at a configurable address.
 */

OUTPUT_FORMAT("elf64-littleaarch64")
OUTPUT_ARCH(aarch64)
ENTRY(_start)

SECTIONS
{
    /* Kernel loaded at 2MB aligned address */
    . = 0x40000000;

    .text : ALIGN(4K) {
        *(.text.boot)
        *(.text .text.*)
    }

    .rodata : ALIGN(4K) {
        *(.rodata .rodata.*)
    }

    .data : ALIGN(4K) {
        *(.data .data.*)
    }

    .bss : ALIGN(4K) {
        __bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        __bss_end = .;
    }

    /* Exception vector table must be 2KB aligned */
    .vectors : ALIGN(2K) {
        *(.vectors)
    }

    /DISCARD/ : {
        *(.comment)
        *(.note*)
        *(.eh_frame*)
    }

    _end = .;
}

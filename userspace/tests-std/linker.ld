/* Userspace program linker script for Rust std programs
 *
 * This linker script is required to place userspace programs at 0x40000000
 * to avoid conflicts with kernel page table mappings at lower addresses.
 */
ENTRY(_start)

SECTIONS {
    . = 0x40000000; /* Start at 1GB (userspace area) */

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

    /* Thread-local storage */
    .tdata : ALIGN(4K) {
        *(.tdata .tdata.*)
    }

    .tbss : ALIGN(4K) {
        *(.tbss .tbss.*)
    }
}

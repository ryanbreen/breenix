/* ARM64 Userspace linker script for musl libc programs on Breenix */
ENTRY(_start)

SECTIONS {
    /* Start at 1GB (userspace area) - matches kernel MMU setup */
    . = 0x40000000;

    .text : ALIGN(4K) {
        *(.text .text.*)
    }

    .rodata : ALIGN(4K) {
        *(.rodata .rodata.*)
    }

    .init_array : ALIGN(8) {
        __init_array_start = .;
        *(.init_array .init_array.*)
        __init_array_end = .;
    }

    .fini_array : ALIGN(8) {
        __fini_array_start = .;
        *(.fini_array .fini_array.*)
        __fini_array_end = .;
    }

    /* Global Offset Table - needed for PIC code in libc */
    .got : ALIGN(8) {
        *(.got .got.*)
    }

    .got.plt : ALIGN(8) {
        *(.got.plt)
    }

    .data.rel.ro : ALIGN(4K) {
        *(.data.rel.ro .data.rel.ro.*)
    }

    .data : ALIGN(4K) {
        *(.data .data.*)
    }

    .bss : ALIGN(4K) {
        *(.bss .bss.*)
        *(COMMON)
    }

    /* Thread-local storage */
    .tdata : ALIGN(8) {
        *(.tdata .tdata.*)
    }

    .tbss : ALIGN(8) {
        *(.tbss .tbss.*)
    }

    /* Stack allocated separately by kernel */

    /DISCARD/ : {
        *(.note.GNU-stack)
        *(.comment)
        *(.eh_frame)
        *(.eh_frame_hdr)
    }
}

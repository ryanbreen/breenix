/* Breenix kernel linker script */
OUTPUT_FORMAT(elf64-x86-64)
OUTPUT_ARCH(i386:x86-64)
ENTRY(_start)

KERNEL_BASE = 0x100000;  /* Current low-half base - will change to 0xffffffff80000000 in Phase 3 */

SECTIONS {
    . = KERNEL_BASE;

    __kernel_image_start = .;

    .text ALIGN(4K) : {
        __kernel_text_start = .;
        KEEP(*(.text.boot))  /* Boot/trampoline code */
        *(.text .text.*)
        __kernel_text_end = .;
    }

    .rodata ALIGN(4K) : {
        __kernel_rodata_start = .;
        *(.rodata .rodata.*)
        __kernel_rodata_end = .;
    }

    .data ALIGN(4K) : {
        __kernel_data_start = .;
        *(.data .data.*)
        __kernel_data_end = .;
    }

    .bss ALIGN(4K) : {
        __kernel_bss_start = .;
        *(.bss .bss.* COMMON)
        . = ALIGN(4K);
        __kernel_bss_end = .;
    }

    __kernel_image_end = .;

    /* Discard sections */
    /DISCARD/ : {
        *(.comment)
        *(.eh_frame)
        *(.note.gnu.build-id)
    }
}
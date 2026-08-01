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
L2_BLOCK_SIZE    = 2M;

SECTIONS
{
    /* Low physical boot code (executes before MMU is enabled) */
    . = KERNEL_PHYS_BASE;

    /* Boot code must come first - contains _start */
    __boot_text_start = .;
    .text.boot : ALIGN(4K) {
        KEEP(*(.text.boot))
    }

    /* Boot exception vectors (low) - 2KB aligned */
    . = ALIGN(2K);
    .text.vectors.boot : {
        KEEP(*(.text.vectors.boot))
    }

    /* The initial page tables use 2MB L2 blocks. Keep writable boot BSS
     * out of the block that contains the pre-MMU executable code. */
    . = ALIGN(L2_BLOCK_SIZE);
    __boot_text_block_end = .;

    /* Boot-time BSS (low, includes boot stack + early tables) */
    .bss.boot (NOLOAD) : ALIGN(4K) {
        __boot_bss_start = .;
        *(.bss.boot .bss.boot.*)
        __boot_bss_end = .;
    }

    . = ALIGN(L2_BLOCK_SIZE);
    __kernel_phys_start = .;

    /* Switch to higher-half VMA for the main kernel (direct map) */
    . = KERNEL_VIRT_BASE + .;
    __kernel_virt_start = .;
    __kernel_text_start = .;
    __kernel_text_phys_start = __kernel_text_start - VMLINUX_OFFSET;

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

    /* Separate executable text from non-executable read-only data at the
     * L2-block granularity used by both boot paths. */
    . = ALIGN(L2_BLOCK_SIZE);
    __kernel_text_end = .;
    __kernel_text_phys_end = __kernel_text_end - VMLINUX_OFFSET;

    /* Read-only data */
    __kernel_rodata_start = .;
    .rodata : AT(ADDR(.rodata) - VMLINUX_OFFSET) {
        *(.rodata .rodata.*)
    }

    . = ALIGN(L2_BLOCK_SIZE);
    __kernel_rodata_end = .;
    __kernel_rodata_phys_end = __kernel_rodata_end - VMLINUX_OFFSET;

    /* Initialized data */
    __kernel_writable_start = .;
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

    __kernel_writable_end = .;
    __kernel_writable_phys_end = __kernel_writable_end - VMLINUX_OFFSET;

    ASSERT((__boot_text_block_end & (L2_BLOCK_SIZE - 1)) == 0,
           "boot text W^X boundary must be 2MB aligned")
    ASSERT((__kernel_text_phys_start & (L2_BLOCK_SIZE - 1)) == 0,
           "kernel text start must be 2MB aligned")
    ASSERT((__kernel_text_phys_end & (L2_BLOCK_SIZE - 1)) == 0,
           "kernel text W^X boundary must be 2MB aligned")
    ASSERT((__kernel_rodata_phys_end & (L2_BLOCK_SIZE - 1)) == 0,
           "kernel rodata W^X boundary must be 2MB aligned")
    ASSERT((__kernel_writable_start & (L2_BLOCK_SIZE - 1)) == 0,
           "kernel writable start must be 2MB aligned")
    ASSERT(__kernel_writable_phys_end <= 0x50000000,
           "kernel writable image overlaps the fixed DMA region")

    /* DMA buffers - placed in Non-Cacheable region at physical 0x50000000.
     * The loader maps this 2MB block with MAIR index 2 (NC) so the xHC
     * controller and CPU see consistent data without cache maintenance.
     * Zeroed at runtime by kernel init, not by loader (NOLOAD section). */
    . = KERNEL_VIRT_BASE + 0x50000000;
    .dma (NOLOAD) : AT(0x50000000) {
        __dma_start = .;
        *(.dma .dma.*)
        __dma_end = .;
    }

    /DISCARD/ : {
        *(.comment)
        *(.note*)
        *(.eh_frame*)
    }

    _end = .;
}

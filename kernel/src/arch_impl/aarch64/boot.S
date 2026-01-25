/*
 * ARM64 Boot Code for Breenix
 *
 * This is the entry point for the ARM64 kernel. It:
 * 1. Sets up the initial stack
 * 2. Zeros the BSS section
 * 3. Drops from EL2 to EL1 if needed
 * 4. Jumps to Rust kernel_main
 *
 * Entry conditions (from UEFI or bootloader):
 * - Running at EL2 or EL1
 * - MMU may be on with identity mapping, or off
 * - Interrupts disabled
 */

.section .text.boot
.global _start

_start:
    // Disable interrupts (should already be disabled)
    msr daifset, #0xf

    // Check current exception level
    mrs x0, currentel
    lsr x0, x0, #2
    and x0, x0, #3

    // If at EL2, drop to EL1
    cmp x0, #2
    b.eq drop_to_el1

    // If at EL1, continue to init
    cmp x0, #1
    b.eq el1_init

    // If at EL3 or EL0, something is wrong - just hang
    b hang

drop_to_el1:
    // Configure EL2 for EL1 execution

    // HCR_EL2: Set RW bit (AArch64 for EL1), clear all virtualization
    mov x0, #(1 << 31)      // RW = 1 (AArch64 at EL1)
    msr hcr_el2, x0

    // SCTLR_EL1: Reset value (MMU off, caches off)
    mov x0, #0
    orr x0, x0, #(1 << 29)  // LSMAOE
    orr x0, x0, #(1 << 28)  // nTLSMD
    orr x0, x0, #(1 << 11)  // EOS (exception return on stack)
    msr sctlr_el1, x0

    // SPSR_EL2: Return to EL1h (EL1 with SP_EL1)
    mov x0, #0x3c5          // M[4:0] = EL1h (0b00101), DAIF masked
    msr spsr_el2, x0

    // ELR_EL2: Return address (el1_init)
    adr x0, el1_init
    msr elr_el2, x0

    // Return to EL1
    eret

el1_init:
    // Now running at EL1

    // Enable FP/SIMD (CPACR_EL1.FPEN = 0b11)
    // This is required because Rust code may use FP/SIMD instructions
    mov x0, #(3 << 20)      // FPEN = 0b11 (no trapping of FP/SIMD)
    msr cpacr_el1, x0
    isb

    // Set up stack pointer
    ldr x0, =__stack_top
    mov sp, x0

    // Zero BSS section
    ldr x0, =__bss_start
    ldr x1, =__bss_end
zero_bss:
    cmp x0, x1
    b.ge bss_done
    str xzr, [x0], #8
    b zero_bss
bss_done:

    // Set VBAR_EL1 to our exception vector table
    ldr x0, =exception_vectors
    msr vbar_el1, x0
    isb

    // Jump to Rust kernel_main
    bl kernel_main

    // If kernel_main returns, hang
hang:
    wfi
    b hang

/*
 * Exception Vector Table
 *
 * ARM64 requires 16 entries, each 128 bytes (0x80), aligned to 2048 bytes.
 * 4 exception types × 4 source contexts = 16 vectors
 */
.section .text.vectors
.balign 0x800
.global exception_vectors
exception_vectors:

// Current EL with SP_EL0 (shouldn't happen in kernel)
.balign 0x80
curr_el_sp0_sync:
    b unhandled_exception
.balign 0x80
curr_el_sp0_irq:
    b unhandled_exception
.balign 0x80
curr_el_sp0_fiq:
    b unhandled_exception
.balign 0x80
curr_el_sp0_serror:
    b unhandled_exception

// Current EL with SP_ELx (kernel mode)
.balign 0x80
curr_el_spx_sync:
    b sync_exception_handler
.balign 0x80
curr_el_spx_irq:
    b irq_handler
.balign 0x80
curr_el_spx_fiq:
    b unhandled_exception
.balign 0x80
curr_el_spx_serror:
    b unhandled_exception

// Lower EL using AArch64 (user mode)
.balign 0x80
lower_el_aarch64_sync:
    b sync_exception_handler
.balign 0x80
lower_el_aarch64_irq:
    b irq_handler
.balign 0x80
lower_el_aarch64_fiq:
    b unhandled_exception
.balign 0x80
lower_el_aarch64_serror:
    b unhandled_exception

// Lower EL using AArch32 (not supported)
.balign 0x80
lower_el_aarch32_sync:
    b unhandled_exception
.balign 0x80
lower_el_aarch32_irq:
    b unhandled_exception
.balign 0x80
lower_el_aarch32_fiq:
    b unhandled_exception
.balign 0x80
lower_el_aarch32_serror:
    b unhandled_exception

/*
 * Exception handlers
 */
.section .text

sync_exception_handler:
    // Save all registers
    sub sp, sp, #272        // 33 registers × 8 bytes + 8 padding
    stp x0, x1, [sp, #0]
    stp x2, x3, [sp, #16]
    stp x4, x5, [sp, #32]
    stp x6, x7, [sp, #48]
    stp x8, x9, [sp, #64]
    stp x10, x11, [sp, #80]
    stp x12, x13, [sp, #96]
    stp x14, x15, [sp, #112]
    stp x16, x17, [sp, #128]
    stp x18, x19, [sp, #144]
    stp x20, x21, [sp, #160]
    stp x22, x23, [sp, #176]
    stp x24, x25, [sp, #192]
    stp x26, x27, [sp, #208]
    stp x28, x29, [sp, #224]
    mrs x0, elr_el1
    mrs x1, spsr_el1
    stp x30, x0, [sp, #240]
    str x1, [sp, #256]

    // Call Rust handler
    mov x0, sp              // Pass frame pointer
    mrs x1, esr_el1         // Pass ESR
    mrs x2, far_el1         // Pass FAR
    bl handle_sync_exception

    // Restore registers
    ldp x0, x1, [sp, #240]
    msr elr_el1, x1
    ldr x1, [sp, #256]
    msr spsr_el1, x1
    ldp x0, x1, [sp, #0]
    ldp x2, x3, [sp, #16]
    ldp x4, x5, [sp, #32]
    ldp x6, x7, [sp, #48]
    ldp x8, x9, [sp, #64]
    ldp x10, x11, [sp, #80]
    ldp x12, x13, [sp, #96]
    ldp x14, x15, [sp, #112]
    ldp x16, x17, [sp, #128]
    ldp x18, x19, [sp, #144]
    ldp x20, x21, [sp, #160]
    ldp x22, x23, [sp, #176]
    ldp x24, x25, [sp, #192]
    ldp x26, x27, [sp, #208]
    ldp x28, x29, [sp, #224]
    ldr x30, [sp, #240]
    add sp, sp, #272
    eret

irq_handler:
    // Save caller-saved registers
    sub sp, sp, #272
    stp x0, x1, [sp, #0]
    stp x2, x3, [sp, #16]
    stp x4, x5, [sp, #32]
    stp x6, x7, [sp, #48]
    stp x8, x9, [sp, #64]
    stp x10, x11, [sp, #80]
    stp x12, x13, [sp, #96]
    stp x14, x15, [sp, #112]
    stp x16, x17, [sp, #128]
    stp x18, x19, [sp, #144]
    stp x20, x21, [sp, #160]
    stp x22, x23, [sp, #176]
    stp x24, x25, [sp, #192]
    stp x26, x27, [sp, #208]
    stp x28, x29, [sp, #224]
    mrs x0, elr_el1
    mrs x1, spsr_el1
    stp x30, x0, [sp, #240]
    str x1, [sp, #256]

    // Call Rust IRQ handler
    bl handle_irq

    // Restore registers
    ldp x0, x1, [sp, #240]
    msr elr_el1, x1
    ldr x1, [sp, #256]
    msr spsr_el1, x1
    ldp x0, x1, [sp, #0]
    ldp x2, x3, [sp, #16]
    ldp x4, x5, [sp, #32]
    ldp x6, x7, [sp, #48]
    ldp x8, x9, [sp, #64]
    ldp x10, x11, [sp, #80]
    ldp x12, x13, [sp, #96]
    ldp x14, x15, [sp, #112]
    ldp x16, x17, [sp, #128]
    ldp x18, x19, [sp, #144]
    ldp x20, x21, [sp, #160]
    ldp x22, x23, [sp, #176]
    ldp x24, x25, [sp, #192]
    ldp x26, x27, [sp, #208]
    ldp x28, x29, [sp, #224]
    ldr x30, [sp, #240]
    add sp, sp, #272
    eret

unhandled_exception:
    // Read exception info
    mrs x0, esr_el1
    mrs x1, elr_el1
    mrs x2, far_el1
    // Hang - in a real system we'd print diagnostic info
    b hang

/*
 * Stack is now defined in the linker script (64KB)
 * Symbols __stack_bottom and __stack_top are provided by linker.ld
 */

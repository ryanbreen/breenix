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
 * - MMU may be on (from firmware) or off
 * - Interrupts disabled
 */

.section .text.boot
.global _start

// High-half kernel base (must match linker.ld)
.equ KERNEL_VIRT_BASE, 0xFFFF000000000000

// Descriptor bits
.equ DESC_VALID,        (1 << 0)
.equ DESC_BLOCK,        (1 << 0)       // 0b01 for L1 block
.equ DESC_TABLE,        (1 << 0) | (1 << 1) // 0b11
.equ DESC_AF,           (1 << 10)
.equ DESC_SH_INNER,     (0b11 << 8)
.equ DESC_SH_NONE,      (0b00 << 8)
.equ DESC_ATTR_DEVICE,  (0 << 2)
.equ DESC_ATTR_NORMAL,  (1 << 2)
.equ DESC_AP_KERNEL,    (0 << 6)       // EL1 RW, EL0 no access
.equ DESC_PXN,          (1 << 53)
.equ DESC_UXN,          (1 << 54)

.equ BLOCK_FLAGS_DEVICE, (DESC_BLOCK | DESC_AF | DESC_SH_NONE | DESC_AP_KERNEL | DESC_ATTR_DEVICE | DESC_PXN | DESC_UXN)
.equ BLOCK_FLAGS_NORMAL, (DESC_BLOCK | DESC_AF | DESC_SH_INNER | DESC_AP_KERNEL | DESC_ATTR_NORMAL)

// MAIR attributes
.equ MAIR_ATTR_DEVICE,  0x00
.equ MAIR_ATTR_NORMAL,  0xFF
.equ MAIR_EL1_VALUE,    (MAIR_ATTR_DEVICE | (MAIR_ATTR_NORMAL << 8))

// TCR configuration (4KB granule, 48-bit VA, inner-shareable, WBWA)
.equ TCR_T0SZ,           16
.equ TCR_T1SZ,           (16 << 16)
.equ TCR_TG0_4K,         (0b00 << 14)
.equ TCR_TG1_4K,         (0b01 << 30)
.equ TCR_SH0_INNER,      (0b11 << 12)
.equ TCR_SH1_INNER,      (0b11 << 28)
.equ TCR_ORGN0_WBWA,     (0b01 << 10)
.equ TCR_IRGN0_WBWA,     (0b01 << 8)
.equ TCR_ORGN1_WBWA,     (0b01 << 26)
.equ TCR_IRGN1_WBWA,     (0b01 << 24)
.equ TCR_VALUE,          (TCR_T0SZ | TCR_T1SZ | TCR_TG0_4K | TCR_TG1_4K | TCR_SH0_INNER | TCR_SH1_INNER | TCR_ORGN0_WBWA | TCR_IRGN0_WBWA | TCR_ORGN1_WBWA | TCR_IRGN1_WBWA)

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

    // Set up boot stack pointer (low)
    ldr x0, =__boot_stack_top
    mov sp, x0

    // Zero kernel BSS section (linked in higher half)
    ldr x0, =__bss_start
    ldr x1, =__bss_end
    ldr x2, =KERNEL_VIRT_BASE
    sub x0, x0, x2
    sub x1, x1, x2
zero_bss:
    cmp x0, x1
    b.ge bss_done
    str xzr, [x0], #8
    b zero_bss
bss_done:

    // Set VBAR_EL1 to boot exception vectors (low)
    ldr x0, =exception_vectors_boot
    msr vbar_el1, x0
    isb

    // Set up MMU and enable high-half mapping
    bl setup_mmu

    // Switch to high-half boot stack
    ldr x0, =__boot_stack_top
    ldr x1, =KERNEL_VIRT_BASE
    add x0, x0, x1
    mov sp, x0

    // Switch to runtime exception vectors (high)
    ldr x0, =exception_vectors
    msr vbar_el1, x0
    isb

    // Jump to Rust kernel_main (high)
    ldr x0, =kernel_main
    br x0

    // If kernel_main returns, hang
hang:
    wfi
    b hang

/*
 * Boot Exception Vector Table (low)
 *
 * Minimal handlers until MMU is enabled and runtime vectors are installed.
 */
.section .text.vectors.boot
.balign 0x800
.global exception_vectors_boot
exception_vectors_boot:
    .rept 16
    b boot_unhandled_exception
    .balign 0x80
    .endr

boot_unhandled_exception:
    wfi
    b boot_unhandled_exception

/*
 * Exception Vector Table (runtime, high)
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
    b lower_el_sync_dispatch
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
// -----------------------------------------------------------------------------
// MMU setup (boot-time, low)
// -----------------------------------------------------------------------------
.section .text.boot

setup_mmu:
    str x30, [sp, #-16]!    // Save link register
    // Zero page tables
    ldr x0, =ttbr0_l0
    bl zero_table
    ldr x0, =ttbr0_l1
    bl zero_table
    ldr x0, =ttbr1_l0
    bl zero_table
    ldr x0, =ttbr1_l1
    bl zero_table

    // TTBR0 L0[0] -> L1
    ldr x0, =ttbr0_l0
    ldr x1, =ttbr0_l1
    orr x1, x1, #DESC_TABLE
    str x1, [x0]

    // TTBR0 L1[0] = device (0x0000_0000 - 0x3FFF_FFFF)
    ldr x0, =ttbr0_l1
    ldr x1, =0x00000000
    ldr x2, =BLOCK_FLAGS_DEVICE
    orr x1, x1, x2
    str x1, [x0, #0]

    // TTBR0 L1[1] = normal (0x4000_0000 - 0x7FFF_FFFF)
    ldr x1, =0x40000000
    ldr x2, =BLOCK_FLAGS_NORMAL
    orr x1, x1, x2
    str x1, [x0, #8]

    // TTBR1 L0[0] -> L1
    ldr x0, =ttbr1_l0
    ldr x1, =ttbr1_l1
    orr x1, x1, #DESC_TABLE
    str x1, [x0]

    // TTBR1 L1[0] = device (high-half direct map, includes PL011 @ 0x0900_0000)
    ldr x0, =ttbr1_l1
    ldr x1, =0x00000000
    ldr x2, =BLOCK_FLAGS_DEVICE
    orr x1, x1, x2
    str x1, [x0, #0]

    // TTBR1 L1[1] = normal (high-half direct map)
    ldr x1, =0x40000000
    ldr x2, =BLOCK_FLAGS_NORMAL
    orr x1, x1, x2
    str x1, [x0, #8]

    // TTBR1 L1[2] = normal (high-half direct map)
    ldr x1, =0x80000000
    ldr x2, =BLOCK_FLAGS_NORMAL
    orr x1, x1, x2
    str x1, [x0, #16]

    // TTBR1 L1[3] = normal (high-half direct map)
    ldr x1, =0xC0000000
    ldr x2, =BLOCK_FLAGS_NORMAL
    orr x1, x1, x2
    str x1, [x0, #24]

    // MAIR / TCR
    ldr x0, =MAIR_EL1_VALUE
    msr mair_el1, x0
    ldr x0, =TCR_VALUE
    msr tcr_el1, x0
    isb

    // TTBRs
    ldr x0, =ttbr0_l0
    msr ttbr0_el1, x0
    ldr x0, =ttbr1_l0
    msr ttbr1_el1, x0
    dsb ishst
    isb

    // Enable MMU
    mrs x0, sctlr_el1
    // Clear WXN (bit 19)
    mov x1, #(1 << 19)
    bic x0, x0, x1
    // Set M (bit 0)
    orr x0, x0, #1
    msr sctlr_el1, x0
    isb

    ldr x30, [sp], #16      // Restore link register
    ret

// Zero a 4KB table at x0
zero_table:
    mov x1, #512              // 512 entries * 8 bytes = 4096
    mov x2, x0
zero_table_loop:
    str xzr, [x2], #8
    subs x1, x1, #1
    b.ne zero_table_loop
    ret

.section .text

/*
 * Dispatch handler for synchronous exceptions from EL0 (userspace).
 *
 * SVCs (syscalls) are routed to syscall_entry_from_el0 in syscall_entry.S,
 * which has proper interrupt masking, reschedule checks, TTBR0 handling,
 * and PREEMPT_ACTIVE management.
 *
 * All other sync exceptions (page faults, etc.) go to the generic
 * sync_exception_handler which passes ESR/FAR to the Rust handler.
 *
 * Uses x16/x17 as scratch (intra-procedure call scratch registers per ABI).
 */
lower_el_sync_dispatch:
    mrs x16, esr_el1
    lsr x17, x16, #26          // Extract EC field (bits [31:26])
    cmp x17, #0x15              // EC 0x15 = SVC instruction from AArch64
    b.eq syscall_entry_from_el0
    b sync_exception_handler

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

    // Check if we need to reschedule before returning from IRQ
    // from_el0 = 1 if SPSR_EL1.M[3:2] indicates EL0 (bit 2 == 0)
    mrs x1, spsr_el1
    and x1, x1, #0x4
    cmp x1, #0
    cset x1, eq

    // Pre-set user_rsp_scratch to current SP + 272 (the return SP if no switch)
    // If a context switch happens, Rust code will overwrite this with new thread's SP
    // PERCPU_USER_RSP_SCRATCH_OFFSET = 40
    mrs x2, tpidr_el1
    add x3, sp, #272
    str x3, [x2, #40]

    mov x0, sp
    bl check_need_resched_and_switch_arm64

    // Restore registers from exception frame (may have been modified by Rust for new thread)
    // First restore ELR and SPSR using x16 as scratch
    ldr x16, [sp, #248]      // x16 = frame.elr (offset 248 = 240 + 8)
    msr elr_el1, x16
    ldr x16, [sp, #256]      // x16 = frame.spsr
    msr spsr_el1, x16

    // Restore all general-purpose registers from the exception frame
    // x0 is particularly critical - it contains the fork() return value!
    // NOTE: x16 is restored LATER via per-CPU scratch (see below)
    ldp x0, x1, [sp, #0]
    ldp x2, x3, [sp, #16]
    ldp x4, x5, [sp, #32]
    ldp x6, x7, [sp, #48]
    ldp x8, x9, [sp, #64]
    ldp x10, x11, [sp, #80]
    ldp x12, x13, [sp, #96]
    ldp x14, x15, [sp, #112]
    // Skip x16 here - will be restored via per-CPU scratch after SP switch
    ldr x17, [sp, #136]      // Restore x17 only
    ldp x18, x19, [sp, #144]
    ldp x20, x21, [sp, #160]
    ldp x22, x23, [sp, #176]
    ldp x24, x25, [sp, #192]
    ldp x26, x27, [sp, #208]
    ldp x28, x29, [sp, #224]
    ldr x30, [sp, #240]

    // Save frame.x16 to per-CPU ERET scratch (offset 96), using x16/x17 as scratch
    mrs x16, tpidr_el1       // x16 = percpu base
    ldr x17, [sp, #128]      // x17 = frame.x16 (temp)
    str x17, [x16, #96]      // percpu.eret_scratch = frame.x16

    // Re-restore x17 from frame (was used as temp above)
    ldr x17, [sp, #136]      // x17 = frame.x17 (final value)

    // Set SP from user_rsp_scratch (offset 40)
    ldr x16, [x16, #40]      // x16 = user_rsp_scratch
    mov sp, x16               // SP = correct stack top

    // Restore x16 from per-CPU ERET scratch
    mrs x16, tpidr_el1
    ldr x16, [x16, #96]      // x16 = saved frame.x16

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

// -----------------------------------------------------------------------------
// Boot-time BSS (low): page tables + boot stack
// -----------------------------------------------------------------------------
.section .bss.boot
.balign 4096
.global ttbr0_l0
ttbr0_l0:
    .skip 4096
.global ttbr0_l1
ttbr0_l1:
    .skip 4096
.global ttbr1_l0
ttbr1_l0:
    .skip 4096
.global ttbr1_l1
ttbr1_l1:
    .skip 4096

.balign 16
.global __boot_stack_bottom
__boot_stack_bottom:
    .skip 65536
.global __boot_stack_top
__boot_stack_top:

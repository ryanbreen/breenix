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

    // Read CPU ID from MPIDR_EL1 (Aff0 field, bits 7:0)
    // Use callee-saved register to preserve across calls
    mrs x19, mpidr_el1
    and x19, x19, #0xFF
    cbnz x19, secondary_cpu_entry   // CPUs 1+ go to parking loop

    // CPU 0 continues to existing boot path...

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
    // x0 = hw_config_ptr (0 = QEMU, non-zero = Parallels loader passes HardwareConfig*)
    ldr x1, =kernel_main
    mov x0, #0
    br x1

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

    // Pre-set user_rsp_scratch = sp + 272 (the return SP if no redirect).
    // If handle_sync_exception redirects to idle, Rust will overwrite this
    // with the idle thread's boot stack, just like the IRQ handler does.
    // PERCPU_USER_RSP_SCRATCH_OFFSET = 40
    mrs x2, tpidr_el1
    add x3, sp, #272
    str x3, [x2, #40]          // user_rsp_scratch = sp + 272

    // Call Rust handler
    mov x0, sp              // Pass frame pointer
    mrs x1, esr_el1         // Pass ESR
    mrs x2, far_el1         // Pass FAR
    bl handle_sync_exception

    // Read ELR from exception frame (modified by Rust handler if redirected)
    ldr x1, [sp, #248]       // x1 = frame.elr

    // DIAGNOSTIC: Check for corrupted ELR before ERET (sync handler path)
    cmp x1, #0x1000
    b.hs 10f
    // ELR < 0x1000 — write diagnostic breadcrumbs to PL011 UART
    stp x2, x3, [sp, #16]    // Temporarily save x2, x3
    mov x2, #0x09000000
    movk x2, #0xFFFF, lsl #48
    mov x3, #'!'
    strb w3, [x2]
    mov x3, #'S'              // 'S' = Sync handler bad ELR
    strb w3, [x2]
    // Print the corrupted ELR (x1) as hex
    mov x3, #60
11: lsr x4, x1, x3
    and x4, x4, #0xF
    cmp x4, #10
    b.lt 12f
    add x4, x4, #('a' - 10)
    b 13f
12: add x4, x4, #'0'
13: strb w4, [x2]
    subs x3, x3, #4
    b.ge 11b
    // Print SP
    mov x3, #'S'
    strb w3, [x2]
    mov x3, #'P'
    strb w3, [x2]
    mov x4, sp
    mov x3, #60
14: lsr x5, x4, x3
    and x5, x5, #0xF
    cmp x5, #10
    b.lt 15f
    add x5, x5, #('a' - 10)
    b 16f
15: add x5, x5, #'0'
16: strb w5, [x2]
    subs x3, x3, #4
    b.ge 14b
    mov x3, #'\n'
    strb w3, [x2]
    ldp x2, x3, [sp, #16]    // Restore x2, x3
    // Redirect to idle_loop_arm64 instead of crashing
    adrp x1, idle_loop_arm64
    add x1, x1, :lo12:idle_loop_arm64
    // Write safe values to frame
    str x1, [sp, #248]       // frame.elr = idle_loop_arm64
    mov x2, #0x5              // EL1h, interrupts enabled
    str x2, [sp, #256]       // frame.spsr = 0x5
10:
    msr elr_el1, x1
    ldr x1, [sp, #256]       // x1 = frame.spsr
    msr spsr_el1, x1
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

    // Set SP based on ERET destination (same logic as IRQ handler):
    // - EL0 return: use kernel_stack_top (offset 16) — always correct
    // - EL1 return: use user_rsp_scratch (offset 40) — kernel resume SP
    // CRITICAL: Use x16 (scratch, saved in eret_scratch) for SPSR check.
    // Do NOT use x17 — it holds the restored frame value needed after ERET.
    mrs x16, spsr_el1        // x16 = SPSR (x16 is scratch)
    and x16, x16, #0xF       // x16 = M[3:0]
    cbnz x16, 70f            // If M != 0 (EL1), use user_rsp_scratch
    // EL0 return: use kernel_stack_top (offset 16)
    mrs x16, tpidr_el1
    ldr x16, [x16, #16]      // x16 = kernel_stack_top
    b 71f
70: // EL1 return: use user_rsp_scratch (offset 40)
    mrs x16, tpidr_el1
    ldr x16, [x16, #40]      // x16 = user_rsp_scratch
71: mov sp, x16               // SP = correct stack

    // Restore x16 from per-CPU ERET scratch
    mrs x16, tpidr_el1
    ldr x16, [x16, #96]      // x16 = saved frame.x16

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
    //
    // CRITICAL: Read from the SAVED frame, not the hardware SPSR_EL1 register.
    // handle_irq may trigger nested synchronous exceptions (data aborts, CoW
    // faults, etc.) whose handlers overwrite SPSR_EL1 with an EL1h value.
    // After the nested handler's ERET, SPSR_EL1 no longer reflects the original
    // exception context. Reading from the frame (saved before handle_irq) gives
    // the correct value.
    ldr x1, [sp, #256]      // Load original SPSR from saved exception frame
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

    // Read ELR from exception frame (written by dispatch function if context switched).
    // The deferred requeue mechanism prevents cross-CPU frame overwrite: the old
    // thread stays in per-CPU DEFERRED_REQUEUE until the next timer tick on THIS
    // CPU, so no other CPU can dispatch it while we still read from the frame.
    ldr x16, [sp, #248]      // x16 = frame.elr

    // DIAGNOSTIC: Check for corrupted ELR before ERET.
    cmp x16, #0x1000
    b.hs 1f
    // ELR < 0x1000 — write diagnostic breadcrumbs to PL011 UART
    stp x0, x1, [sp, #0]     // Temporarily save x0, x1 (will restore below)
    mov x0, #0x09000000      // UART physical base (QEMU virt)
    movk x0, #0xFFFF, lsl #48 // HHDM: 0xFFFF000009000000
    mov x1, #'!'
    strb w1, [x0]             // '!' = IRQ ERET with bad ELR
    mov x1, #'E'
    strb w1, [x0]
    // Print the corrupted ELR value (x16) as hex
    mov x1, x16              // x1 = ELR value to print
    mov x3, #60              // shift = 60 (start with highest nibble)
2:  lsr x4, x1, x3           // x4 = (ELR >> shift) & 0xF
    and x4, x4, #0xF
    cmp x4, #10
    b.lt 3f
    add x4, x4, #('a' - 10)
    b 4f
3:  add x4, x4, #'0'
4:  strb w4, [x0]
    subs x3, x3, #4
    b.ge 2b
    // Print SP
    mov x1, #'S'
    strb w1, [x0]
    mov x1, sp
    mov x3, #60
5:  lsr x4, x1, x3
    and x4, x4, #0xF
    cmp x4, #10
    b.lt 6f
    add x4, x4, #('a' - 10)
    b 7f
6:  add x4, x4, #'0'
7:  strb w4, [x0]
    subs x3, x3, #4
    b.ge 5b
    mov x1, #'\n'
    strb w1, [x0]
    ldp x0, x1, [sp, #0]     // Restore x0, x1
    // Redirect to idle_loop_arm64 instead of crashing
    adrp x16, idle_loop_arm64
    add x16, x16, :lo12:idle_loop_arm64
    // Write safe values to frame
    str x16, [sp, #248]      // frame.elr = idle_loop_arm64
    mov x1, #0x5              // EL1h, interrupts enabled
    str x1, [sp, #256]       // frame.spsr = 0x5
    ldr x1, [sp, #8]          // Re-restore x1 from frame
1:
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

    // Set SP based on ERET destination:
    // - EL0 return (SPSR.M[3:0] == 0): use kernel_stack_top (offset 16).
    //   This is the definitive source for where the next exception frame
    //   should be pushed. kernel_stack_top is always kept in sync by
    //   the context switch dispatch path, unlike user_rsp_scratch which
    //   can become stale if a thread was initially launched via
    //   return_to_userspace() or if context.sp was saved from a wrong stack.
    // - EL1 return (SPSR.M[3:0] != 0): use user_rsp_scratch (offset 40).
    //   For kernel threads, this is the kernel SP where execution resumes.
    // CRITICAL: Use x16 (scratch, saved in eret_scratch) for SPSR check.
    // Do NOT use x17 — it holds the restored frame value needed after ERET.
    mrs x16, spsr_el1        // x16 = SPSR (x16 is scratch)
    and x16, x16, #0xF       // x16 = M[3:0]
    cbnz x16, 60f            // If M != 0 (EL1), use user_rsp_scratch
    // EL0 return: use kernel_stack_top (offset 16)
    mrs x16, tpidr_el1
    ldr x16, [x16, #16]      // x16 = kernel_stack_top
    b 61f
60: // EL1 return: use user_rsp_scratch (offset 40)
    mrs x16, tpidr_el1
    ldr x16, [x16, #40]      // x16 = user_rsp_scratch
61: mov sp, x16               // SP = correct stack

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
// Secondary CPU entry (boot-time, low)
// -----------------------------------------------------------------------------
.section .text.boot

.global secondary_cpu_entry
secondary_cpu_entry:
    // PSCI CPU_ON entry point for secondary CPUs.
    // PSCI starts us at EL1 with:
    //   x0 = context_id (cpu_id, passed by release_cpu)
    //   MMU off, caches off, interrupts masked (DAIF set)
    //
    // For the spin-table fallback (non-PSCI), we also branch here from _start
    // with x19 = cpu_id. In that case we may be at EL2.

    // Mask all interrupts (should already be masked by PSCI, but be safe)
    msr daifset, #0xf

    // Save cpu_id in callee-saved register
    // PSCI passes it in x0; spin-table path has it in x19
    // Check if x19 is already set (non-zero means spin-table path)
    cbnz x19, 1f
    mov x19, x0              // PSCI path: cpu_id from x0
1:

    // Check current exception level
    mrs x0, currentel
    lsr x0, x0, #2
    and x0, x0, #3
    cmp x0, #2
    b.eq secondary_drop_to_el1
    // EL1 (normal PSCI path) - fall through

secondary_el1_init:
    // Enable FP/SIMD (CPACR_EL1.FPEN = 0b11)
    mov x0, #(3 << 20)
    msr cpacr_el1, x0
    isb

    // Set up per-CPU boot stack (physical addresses, before MMU)
    // Stack top = 0x4100_0000 + (cpu_id + 1) * 0x20_0000
    // This gives each CPU a 2MB stack region:
    //   CPU 1: 0x4120_0000 (top) .. 0x4100_0000
    //   CPU 2: 0x4140_0000 (top) .. 0x4120_0000
    //   CPU 3: 0x4160_0000 (top) .. 0x4140_0000
    mov x0, x19              // cpu_id
    add x0, x0, #1           // cpu_id + 1
    lsl x0, x0, #21          // * 0x20_0000 (2MB)
    ldr x1, =0x41000000      // base of per-CPU stack region
    add x0, x0, x1
    mov sp, x0

    // DO NOT zero BSS - CPU 0 handles that
    // DO NOT call setup_mmu - reuse CPU 0's page tables

    // Set VBAR_EL1 to boot exception vectors (low) for now
    ldr x0, =exception_vectors_boot
    msr vbar_el1, x0
    isb

    // Load the same MMU config as CPU 0 (MAIR, TCR, TTBRs)
    ldr x0, =MAIR_EL1_VALUE
    msr mair_el1, x0
    ldr x0, =TCR_VALUE
    msr tcr_el1, x0
    isb

    ldr x0, =ttbr0_l0
    msr ttbr0_el1, x0
    ldr x0, =ttbr1_l0
    msr ttbr1_el1, x0
    dsb ishst
    isb

    // Enable MMU
    mrs x0, sctlr_el1
    mov x1, #(1 << 19)      // Clear WXN bit
    bic x0, x0, x1
    orr x0, x0, #1          // Set M bit (MMU enable)
    msr sctlr_el1, x0
    isb

    // Switch to high-half stack (SP += KERNEL_VIRT_BASE)
    mov x0, sp
    ldr x1, =KERNEL_VIRT_BASE
    add x0, x0, x1
    mov sp, x0

    // Switch to runtime exception vectors (high)
    ldr x0, =exception_vectors
    msr vbar_el1, x0
    isb

    // Call secondary_cpu_entry_rust(cpu_id)
    // x19 still holds cpu_id
    mov x0, x19
    ldr x1, =secondary_cpu_entry_rust
    br x1

secondary_drop_to_el1:
    // Configure EL2 for EL1 execution (same as CPU 0)
    mov x0, #(1 << 31)      // HCR_EL2: RW = 1 (AArch64 at EL1)
    msr hcr_el2, x0

    mov x0, #0
    orr x0, x0, #(1 << 29)  // LSMAOE
    orr x0, x0, #(1 << 28)  // nTLSMD
    orr x0, x0, #(1 << 11)  // EOS
    msr sctlr_el1, x0

    mov x0, #0x3c5          // SPSR_EL2: EL1h, DAIF masked
    msr spsr_el2, x0

    adr x0, secondary_el1_init
    msr elr_el2, x0
    eret

secondary_hang:
    wfi
    b secondary_hang

// -----------------------------------------------------------------------------
// Physical address of secondary_cpu_entry, stored in .rodata (high-half).
// Rust code cannot reference secondary_cpu_entry directly because the
// .text.boot -> .text gap exceeds AArch64 ADRP range (+/- 4 GiB).
// -----------------------------------------------------------------------------
.section .rodata
.balign 8
.global SECONDARY_CPU_ENTRY_PHYS
SECONDARY_CPU_ENTRY_PHYS:
    .quad secondary_cpu_entry

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

.balign 64
.global CPU_RELEASE_ADDR
CPU_RELEASE_ADDR:
    .skip 64   // 8 CPUs * 8 bytes

.balign 16
.global __boot_stack_bottom
__boot_stack_bottom:
    .skip 65536
.global __boot_stack_top
__boot_stack_top:

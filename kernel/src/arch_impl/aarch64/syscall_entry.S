/*
 * ARM64 Syscall Entry Assembly for Breenix
 *
 * This file provides the syscall entry and exit paths for ARM64.
 * ARM64 uses SVC #0 to trigger synchronous exceptions for syscalls.
 *
 * ARM64 syscall convention (Linux compatible):
 *   X8  = syscall number
 *   X0-X5 = arguments (6 args max)
 *   X0  = return value (or negative errno on error)
 *
 * Key differences from x86_64:
 *   - No PUSH/POP - use STP/LDP pairs
 *   - No CR3 - use TTBR0_EL1 for user page table, TTBR1_EL1 for kernel
 *   - No GS segment - use TPIDR_EL1 for per-CPU data
 *   - ERET instead of IRETQ
 *   - SP_EL0 holds user stack, SP_EL1 holds kernel stack
 *
 * Frame layout matches Aarch64ExceptionFrame in exception_frame.rs:
 *   [sp, #0]   = x0
 *   [sp, #8]   = x1
 *   ...
 *   [sp, #232] = x29 (frame pointer)
 *   [sp, #240] = x30 (link register)
 *   [sp, #248] = ELR_EL1 (return address)
 *   [sp, #256] = SPSR_EL1 (saved program status)
 *   Total: 264 bytes (33 * 8), rounded to 272 for 16-byte alignment
 */

.section .text.syscall
.global syscall_entry_from_el0
.global syscall_return_to_userspace_aarch64
.global check_signals_on_syscall_return

/* External Rust functions */
.extern rust_syscall_handler_aarch64
.extern check_need_resched_and_switch_arm64
.extern trace_eret_to_el0

/*
 * Syscall entry point from EL0 (userspace)
 *
 * This is called when userspace executes SVC #0.
 * On entry:
 *   - CPU is at EL1 (switched by exception)
 *   - SP is SP_EL1 (kernel stack, if configured)
 *   - User SP is in SP_EL0
 *   - ELR_EL1 = return address (instruction after SVC)
 *   - SPSR_EL1 = saved PSTATE
 *   - ESR_EL1 = exception syndrome (EC=0x15 for SVC from AArch64)
 *   - X0-X7 contain syscall arguments and number
 *
 * Note: This is a separate entry point from the generic exception handler
 * to allow syscall-specific optimizations (no ESR/FAR needed).
 */
syscall_entry_from_el0:
    /*
     * CRITICAL: Disable interrupts immediately to prevent races
     * during register save sequence. Timer interrupts at 1000 Hz
     * could corrupt partially-saved state.
     */
    msr daifset, #0x2       /* Mask IRQ (bit 1) */

    /*
     * Switch to kernel stack if not already on it.
     * ARM64 uses SP_EL1 for kernel exceptions, but we need to
     * ensure we're using a proper kernel stack for this thread.
     *
     * CRITICAL: We must save x9/x10 BEFORE using them as scratch
     * for the per-CPU lookup. Otherwise userspace gets kernel
     * addresses leaked into x9/x10 on syscall return.
     *
     * Strategy: temporarily push x9/x10 to the current SP_EL1 stack,
     * do the per-CPU lookup, then either:
     *   - No switch: pop x9/x10 and save normally
     *   - Switch: copy stashed x9/x10 from old stack into new frame
     */
    stp x9, x10, [sp, #-16]!    /* Stash user x9/x10 on current stack */

    mrs x9, tpidr_el1            /* x9 = per-CPU base */
    cbz x9, .Lno_stack_switch
    ldr x10, [x9, #16]          /* x10 = kernel_stack_top */
    cbz x10, .Lno_stack_switch

    /*
     * Stack switch needed: x10 = new kernel stack top.
     * User x9/x10 are stashed at [sp] on the OLD stack.
     * Save old SP so we can copy them into the new frame.
     */
    mov x9, sp                   /* x9 = old SP (stash location) */
    mov sp, x10                  /* Switch to per-CPU kernel stack */
    sub sp, sp, #272             /* Allocate exception frame */

    /* Copy real user x9/x10 from old stack stash into frame */
    ldr x10, [x9]               /* x10 = user x9 */
    str x10, [sp, #72]          /* frame.x9 = user x9 */
    ldr x10, [x9, #8]           /* x10 = user x10 */
    str x10, [sp, #80]          /* frame.x10 = user x10 */
    /* x8, x11 were NOT clobbered in this path */
    str x8, [sp, #64]           /* frame.x8 = syscall number */
    str x11, [sp, #88]          /* frame.x11 = user x11 */
    b .Lsave_common

.Lno_stack_switch:
    /*
     * No stack switch needed. Pop stashed x9/x10 (restoring
     * their real user values) and save normally.
     */
    ldp x9, x10, [sp], #16      /* Pop user x9/x10 from stash */
    sub sp, sp, #272             /* Allocate exception frame */
    stp x8, x9, [sp, #64]       /* frame.x8 = syscall number, frame.x9 */
    stp x10, x11, [sp, #80]     /* frame.x10, frame.x11 */

.Lsave_common:
    /*
     * Save all other general-purpose registers.
     * x8-x11 are already saved by the path-specific code above.
     * All other registers are unclobbered in both paths.
     */
    stp x0, x1, [sp, #0]
    stp x2, x3, [sp, #16]
    stp x4, x5, [sp, #32]
    stp x6, x7, [sp, #48]
    /* x8-x11 already saved above */
    stp x12, x13, [sp, #96]
    stp x14, x15, [sp, #112]
    stp x16, x17, [sp, #128]
    stp x18, x19, [sp, #144]
    stp x20, x21, [sp, #160]
    stp x22, x23, [sp, #176]
    stp x24, x25, [sp, #192]
    stp x26, x27, [sp, #208]
    stp x28, x29, [sp, #224]    /* x29 = frame pointer */

    /* Save x30 (link register) and ELR_EL1 (return address) */
    mrs x10, elr_el1
    stp x30, x10, [sp, #240]

    /* Save SPSR_EL1 (saved program status) */
    mrs x10, spsr_el1
    str x10, [sp, #256]

    /*
     * Save user stack pointer (SP_EL0) to per-CPU scratch area.
     * This allows proper restoration even if context switch occurs.
     */
    mrs x9, tpidr_el1
    cbz x9, .Lskip_sp_save
    mrs x10, sp_el0
    str x10, [x9, #40]          /* user_rsp_scratch offset = 40 */
.Lskip_sp_save:

    /*
     * CRITICAL: Save process TTBR0_EL1 before any potential page table switch.
     * This mirrors x86_64's CR3 save for process page table tracking.
     */
    mrs x9, tpidr_el1
    cbz x9, .Lskip_ttbr_save
    mrs x10, ttbr0_el1
    str x10, [x9, #80]          /* saved_process_cr3 offset = 80 */
.Lskip_ttbr_save:

    /*
     * Clear PREEMPT_ACTIVE flag at syscall entry.
     * This flag is set during syscall return to prevent context switches
     * from saving kernel register values as userspace context.
     *
     * preempt_count is at offset 32, PREEMPT_ACTIVE = 0x10000000 (bit 28)
     */
    mrs x9, tpidr_el1
    cbz x9, .Lskip_preempt_clear
    ldr w10, [x9, #32]
    bic w10, w10, #0x10000000   /* Clear bit 28 */
    str w10, [x9, #32]
.Lskip_preempt_clear:

    /*
     * Call the Rust syscall handler.
     * Argument: x0 = pointer to exception frame
     */
    mov x0, sp
    bl rust_syscall_handler_aarch64

    /*
     * After syscall handler returns, check for rescheduling.
     * The return value is already set in frame->x0 by the handler.
     */

    /* Disable interrupts for register restoration */
    msr daifset, #0x2           /* Mask IRQ */

    /*
     * Set PREEMPT_ACTIVE to protect register restoration sequence.
     * This prevents timer interrupts from attempting context switch
     * while we have mixed kernel/user state.
     */
    mrs x9, tpidr_el1
    cbz x9, .Lskip_preempt_set
    ldr w10, [x9, #32]
    orr w10, w10, #0x10000000   /* Set bit 28 */
    str w10, [x9, #32]
.Lskip_preempt_set:

    /*
     * Pre-set user_rsp_scratch = sp + 272 (the return SP if no context switch).
     * If a context switch occurs, the Rust code will overwrite this with the
     * new thread's SP. This mirrors the IRQ handler's approach in boot.S.
     * PERCPU_USER_RSP_SCRATCH_OFFSET = 40
     */
    mrs x9, tpidr_el1
    cbz x9, .Lskip_rsp_scratch_set
    add x10, sp, #272
    str x10, [x9, #40]

.Lskip_rsp_scratch_set:

    /*
     * Check if rescheduling is needed before returning to userspace.
     * Pass frame pointer for potential context switch.
     * CRITICAL: Set from_el0=true (x1=1) explicitly. Without this, x1 contains
     * whatever value rust_syscall_handler_aarch64 left (undefined by ABI), causing
     * the context switch code to incorrectly treat this as a kernel-mode return.
     */
    mov x0, sp
    mov x1, #1
    bl check_need_resched_and_switch_arm64

    /*
     * Restore registers from exception frame.
     *
     * Read ELR/SPSR from the exception frame (written by dispatch function
     * if a context switch occurred). The deferred requeue mechanism prevents
     * cross-CPU frame overwrite: the old thread stays in per-CPU
     * DEFERRED_REQUEUE until the next timer tick on THIS CPU.
     *
     * Order:
     * 1. Restore system registers (SPSR, ELR) from frame
     * 2. Restore x2-x30 from frame (skip x0/x1 for now)
     * 3. Do TTBR0 switch using x0/x1 as scratch (they're not restored yet)
     * 4. Restore x0/x1 from frame (still valid - frame not deallocated)
     * 5. Deallocate frame and ERET
     */

    /* Read ELR from exception frame */
    ldr x10, [sp, #248]      /* x10 = frame.elr */

    /* DIAGNOSTIC: Check for corrupted ELR before ERET (was missing in SVC path) */
    cmp x10, #0x1000
    b.hs .Lsvc_elr_ok
    /* ELR < 0x1000 — write diagnostic to UART */
    mov x0, #0x09000000
    movk x0, #0xFFFF, lsl #48
    mov x1, #'!'
    strb w1, [x0]
    mov x1, #'S'             /* 'S' = SVC ERET with bad ELR */
    strb w1, [x0]
    mov x1, #'\n'
    strb w1, [x0]
    /* Redirect to idle_loop_arm64 */
    adrp x10, idle_loop_arm64
    add x10, x10, :lo12:idle_loop_arm64
    /* Force SPSR to EL1h - write to frame */
    mov x1, #0x5
    str x1, [sp, #256]       /* frame.spsr = 0x5 (EL1h) */
    str x10, [sp, #248]      /* frame.elr = idle_loop_arm64 */
.Lsvc_elr_ok:
    msr elr_el1, x10

    /* Read SPSR from exception frame */
    ldr x10, [sp, #256]      /* x10 = frame.spsr */
    msr spsr_el1, x10

    /* Restore x30 (link register) */
    ldr x30, [sp, #240]

    /* Restore x28, x29 */
    ldp x28, x29, [sp, #224]

    /* Restore x26, x27 */
    ldp x26, x27, [sp, #208]

    /* Restore x24, x25 */
    ldp x24, x25, [sp, #192]

    /* Restore x22, x23 */
    ldp x22, x23, [sp, #176]

    /* Restore x20, x21 */
    ldp x20, x21, [sp, #160]

    /* Restore x18, x19 */
    ldp x18, x19, [sp, #144]

    /* Restore x16, x17 */
    ldp x16, x17, [sp, #128]

    /* Restore x14, x15 */
    ldp x14, x15, [sp, #112]

    /* Restore x12, x13 */
    ldp x12, x13, [sp, #96]

    /* Restore x10, x11 */
    ldp x10, x11, [sp, #80]

    /* Restore x8, x9 */
    ldp x8, x9, [sp, #64]

    /* Restore x6, x7 */
    ldp x6, x7, [sp, #48]

    /* Restore x4, x5 */
    ldp x4, x5, [sp, #32]

    /* Restore x2, x3 - skip x0/x1 for now */
    ldp x2, x3, [sp, #16]

    /*
     * Check if TTBR0 switch is needed (for context switch).
     * Read next_cr3 from per-CPU data at offset 64.
     * Use x0/x1 as scratch since they haven't been restored yet.
     */
    mrs x0, tpidr_el1
    cbz x0, .Lno_ttbr_switch

    ldr x1, [x0, #64]           /* next_cr3 offset = 64 */
    cbz x1, .Lrestore_saved_ttbr

    /* Clear next_cr3 BEFORE switching (avoid accessing after switch) */
    str xzr, [x0, #64]

    /* Perform TTBR0 switch with full TLB invalidation */
    dsb ishst
    msr ttbr0_el1, x1
    isb
    tlbi vmalle1is
    dsb ish
    isb
    b .Lafter_ttbr_check

.Lrestore_saved_ttbr:
    /* No context switch - restore original process TTBR0 */
    ldr x1, [x0, #80]           /* saved_process_cr3 offset = 80 */
    cbz x1, .Lafter_ttbr_check
    dsb ishst
    msr ttbr0_el1, x1
    isb
    tlbi vmalle1is
    dsb ish
    isb

.Lafter_ttbr_check:
.Lno_ttbr_switch:

    /*
     * Clear PREEMPT_ACTIVE before restoring x0/x1.
     * Use x0/x1 as scratch (they haven't been restored yet).
     * Must clear BEFORE ERET so next IRQ can do context switches.
     */
    mrs x0, tpidr_el1
    cbz x0, .Lskip_preempt_final_clear
    ldr w1, [x0, #32]
    bic w1, w1, #0x10000000   /* Clear bit 28 */
    str w1, [x0, #32]
.Lskip_preempt_final_clear:

    /*
     * Restore x0/x1 and switch SP for ERET.
     *
     * Challenge: we need to set SP = user_rsp_scratch AND restore x0/x1
     * from the frame, but after changing SP the frame may not be addressable
     * (if a context switch moved us to a different kernel stack).
     *
     * Solution: save frame.x0 to per-CPU scratch (offset 96), restore x1
     * from the frame, switch SP, then restore x0 from per-CPU scratch.
     *
     * x0 = tpidr_el1 from PREEMPT_ACTIVE clear above.
     */

    /* Save frame.x0 to per-CPU ERET scratch */
    ldr x1, [sp, #0]           /* x1 = frame.x0 */
    str x1, [x0, #96]          /* percpu.eret_scratch = frame.x0 */

    /* Restore x1 from frame (final value) */
    ldr x1, [sp, #8]           /* x1 = frame.x1 */

    /* Set SP from user_rsp_scratch */
    ldr x0, [x0, #40]          /* x0 = user_rsp_scratch */
    mov sp, x0                  /* SP = correct kernel stack top */

    /* Restore x0 from per-CPU ERET scratch */
    mrs x0, tpidr_el1
    ldr x0, [x0, #96]          /* x0 = saved frame.x0 */

    eret

    /* Should never reach here - debug marker */
    mov x0, #0xDEAD
    b .


/*
 * syscall_return_to_userspace_aarch64
 *
 * Used when starting a new userspace thread (first entry to userspace).
 * This sets up a minimal exception frame and ERETs to userspace.
 *
 * Arguments:
 *   x0 = user entry point (ELR_EL1)
 *   x1 = user stack pointer (SP_EL0)
 *   x2 = user PSTATE/flags (SPSR_EL1)
 */
syscall_return_to_userspace_aarch64:
    /* Disable interrupts */
    msr daifset, #0xf

    /* Set up return state */
    msr elr_el1, x0             /* Entry point */
    msr sp_el0, x1              /* User stack */
    msr spsr_el1, x2            /* User PSTATE (EL0, interrupts enabled) */

    /*
     * Check for TTBR0 switch (process page table).
     */
    mrs x9, tpidr_el1
    cbz x9, .Lfirst_entry_no_ttbr

    ldr x10, [x9, #64]          /* next_cr3 offset = 64 */
    cbz x10, .Lfirst_entry_restore_ttbr

    /* Clear next_cr3 and switch with full TLB invalidation */
    str xzr, [x9, #64]
    dsb ishst
    msr ttbr0_el1, x10
    isb
    tlbi vmalle1is
    dsb ish
    isb
    b .Lfirst_entry_after_ttbr

.Lfirst_entry_restore_ttbr:
    /* Try saved process TTBR0 */
    ldr x10, [x9, #80]
    cbz x10, .Lfirst_entry_after_ttbr
    dsb ishst
    msr ttbr0_el1, x10
    isb
    tlbi vmalle1is
    dsb ish
    isb

.Lfirst_entry_after_ttbr:
.Lfirst_entry_no_ttbr:

    /*
     * Set SP to this thread's kernel_stack_top so that subsequent
     * exceptions from EL0 (timer IRQs, syscalls) use the correct
     * kernel stack instead of the stale boot stack.
     *
     * Without this, SP_EL1 retains whatever value it had before
     * this function was called (typically the boot stack from
     * kernel_main), so the first timer IRQ from userspace would
     * push its exception frame on the wrong stack.
     */
    mrs x9, tpidr_el1
    cbz x9, .Lskip_sp_fix
    ldr x10, [x9, #16]          /* kernel_stack_top (per-CPU offset 16) */
    cbz x10, .Lskip_sp_fix
    mov sp, x10
.Lskip_sp_fix:

    /*
     * Clear all registers to prevent information leaks.
     * x0 will be used for initial return value (0).
     */
    mov x0, #0
    mov x1, #0
    mov x2, #0
    mov x3, #0
    mov x4, #0
    mov x5, #0
    mov x6, #0
    mov x7, #0
    mov x8, #0
    mov x9, #0
    mov x10, #0
    mov x11, #0
    mov x12, #0
    mov x13, #0
    mov x14, #0
    mov x15, #0
    mov x16, #0
    mov x17, #0
    mov x18, #0
    mov x19, #0
    mov x20, #0
    mov x21, #0
    mov x22, #0
    mov x23, #0
    mov x24, #0
    mov x25, #0
    mov x26, #0
    mov x27, #0
    mov x28, #0
    mov x29, #0
    mov x30, #0

    /* Return to userspace */
    eret

    /* Should never reach here */
    mov x0, #0xCC
    b .

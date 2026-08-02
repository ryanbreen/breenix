//! ARM64 CPU context and context switching.
//!
//! This module provides:
//! - Context switching between kernel threads
//! - Return to userspace (EL0) mechanism
//!
//! Note: The CpuContext type is defined in task/thread.rs to maintain a single
//! source of truth for thread state. This module re-exports it for convenience.

use core::arch::asm;

// Re-export CpuContext from the canonical location
pub use crate::task::thread::CpuContext;

/// Return to userspace from the current kernel context.
///
/// This sets up the exception return frame and uses ERET to jump to EL0.
///
/// # Safety
///
/// - `entry` must be a valid userspace address
/// - `user_sp` must be a valid, mapped user stack
/// - Interrupts should be properly configured
#[inline(never)]
pub unsafe fn return_to_userspace(entry: u64, user_sp: u64) -> ! {
    asm!(
        // Set up ELR_EL1 (return address)
        "msr elr_el1, {entry}",

        // Set up SP_EL0 (user stack pointer)
        "msr sp_el0, {user_sp}",

        // Set up SPSR_EL1 for return to EL0
        // Mode = 0 (EL0t), DAIF = 0 (interrupts enabled)
        "mov x0, #0",
        "msr spsr_el1, x0",

        // CRITICAL: Set SP to per-CPU kernel_stack_top before ERET.
        //
        // After ERET to EL0, SP_EL1 retains whatever value SP had before
        // the ERET instruction. On the first exception from EL0 (timer IRQ,
        // syscall, page fault), hardware uses SP_EL1 for the exception frame.
        //
        // Without this, SP_EL1 retains the boot stack from kernel_main,
        // causing the first timer IRQ to push its frame on the boot stack
        // instead of the thread's allocated kernel stack. This shares the
        // boot stack between the user thread's IRQ handling and the idle
        // thread, leading to stack corruption and crashes (e.g., x19=0 in
        // handle_irq after timer_interrupt_handler returns).
        //
        // Per-CPU layout: tpidr_el1 + 16 = kernel_stack_top
        "mrs x0, tpidr_el1",
        "cbz x0, 1f",
        "ldr x0, [x0, #16]",
        "cbz x0, 1f",
        "mov sp, x0",
        "1:",

        // Clear all general purpose registers for security
        "mov x0, #0",
        "mov x1, #0",
        "mov x2, #0",
        "mov x3, #0",
        "mov x4, #0",
        "mov x5, #0",
        "mov x6, #0",
        "mov x7, #0",
        "mov x8, #0",
        "mov x9, #0",
        "mov x10, #0",
        "mov x11, #0",
        "mov x12, #0",
        "mov x13, #0",
        "mov x14, #0",
        "mov x15, #0",
        "mov x16, #0",
        "mov x17, #0",
        "mov x18, #0",
        "mov x19, #0",
        "mov x20, #0",
        "mov x21, #0",
        "mov x22, #0",
        "mov x23, #0",
        "mov x24, #0",
        "mov x25, #0",
        "mov x26, #0",
        "mov x27, #0",
        "mov x28, #0",
        "mov x29, #0",
        "mov x30, #0",

        // Exception return - jumps to EL0
        "eret",
        entry = in(reg) entry,
        user_sp = in(reg) user_sp,
        options(noreturn)
    )
}

/// Save the current userspace context from an exception frame.
///
/// Called when taking an exception from userspace to save the user's
/// register state for later restoration.
pub fn save_user_context(
    ctx: &mut CpuContext,
    frame: &super::exception_frame::Aarch64ExceptionFrame,
) {
    // Save x0 (important for fork return value)
    ctx.x0 = frame.x0;
    // Save callee-saved registers
    ctx.x19 = frame.x19;
    ctx.x20 = frame.x20;
    ctx.x21 = frame.x21;
    ctx.x22 = frame.x22;
    ctx.x23 = frame.x23;
    ctx.x24 = frame.x24;
    ctx.x25 = frame.x25;
    ctx.x26 = frame.x26;
    ctx.x27 = frame.x27;
    ctx.x28 = frame.x28;
    ctx.x29 = frame.x29;
    ctx.x30 = frame.x30;
    ctx.elr_el1 = frame.elr;
    ctx.spsr_el1 = frame.spsr;

    // Read SP_EL0 (user stack pointer)
    let sp_el0: u64;
    unsafe {
        asm!("mrs {}, sp_el0", out(reg) sp_el0, options(nomem, nostack));
    }
    ctx.sp_el0 = sp_el0;
}

/// Restore userspace context to an exception frame.
///
/// Called before returning to userspace to set up the exception return frame.
pub fn restore_user_context(
    frame: &mut super::exception_frame::Aarch64ExceptionFrame,
    ctx: &CpuContext,
) {
    // Restore x0 (important for fork return value - child gets 0, parent gets child PID)
    frame.x0 = ctx.x0;
    // Restore callee-saved registers
    frame.x19 = ctx.x19;
    frame.x20 = ctx.x20;
    frame.x21 = ctx.x21;
    frame.x22 = ctx.x22;
    frame.x23 = ctx.x23;
    frame.x24 = ctx.x24;
    frame.x25 = ctx.x25;
    frame.x26 = ctx.x26;
    frame.x27 = ctx.x27;
    frame.x28 = ctx.x28;
    frame.x29 = ctx.x29;
    frame.x30 = ctx.x30;
    frame.elr = ctx.elr_el1;
    frame.spsr = ctx.spsr_el1;

    // Set SP_EL0 (user stack pointer)
    unsafe {
        asm!("msr sp_el0, {}", in(reg) ctx.sp_el0, options(nomem, nostack));
    }
}

/// Read the current SP_EL0 value
#[inline]
pub fn read_sp_el0() -> u64 {
    let sp: u64;
    unsafe {
        asm!("mrs {}, sp_el0", out(reg) sp, options(nomem, nostack));
    }
    sp
}

/// Write to SP_EL0
///
/// # Safety
/// The value must be a valid stack pointer.
#[inline]
pub unsafe fn write_sp_el0(sp: u64) {
    asm!("msr sp_el0, {}", in(reg) sp, options(nomem, nostack));
}

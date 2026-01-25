//! ARM64 CPU context and context switching.
//!
//! This module provides:
//! - CPU context structure for saving/restoring thread state
//! - Context switching between kernel threads
//! - Return to userspace (EL0) mechanism

use core::arch::asm;

/// ARM64 CPU context for thread switching.
///
/// This structure holds all the state needed to resume a thread.
/// Layout must be kept in sync with assembly context switch code.
#[repr(C)]
#[derive(Debug, Clone, Default)]
pub struct CpuContext {
    // General purpose registers (callee-saved for context switch)
    pub x19: u64,
    pub x20: u64,
    pub x21: u64,
    pub x22: u64,
    pub x23: u64,
    pub x24: u64,
    pub x25: u64,
    pub x26: u64,
    pub x27: u64,
    pub x28: u64,
    pub x29: u64,  // Frame pointer
    pub x30: u64,  // Link register (return address for context switch)

    // Stack pointer
    pub sp: u64,

    // For userspace threads, we also need:
    pub sp_el0: u64,   // User stack pointer
    pub elr_el1: u64,  // Exception return address (user PC)
    pub spsr_el1: u64, // Saved program state (includes EL0 mode bits)
}

impl CpuContext {
    /// Create a new empty context
    pub const fn new() -> Self {
        Self {
            x19: 0, x20: 0, x21: 0, x22: 0,
            x23: 0, x24: 0, x25: 0, x26: 0,
            x27: 0, x28: 0, x29: 0, x30: 0,
            sp: 0,
            sp_el0: 0,
            elr_el1: 0,
            spsr_el1: 0,
        }
    }

    /// Create a context for a new kernel thread.
    ///
    /// The thread will start executing at `entry_point` with the given stack.
    pub fn new_kernel_thread(entry_point: u64, stack_top: u64) -> Self {
        Self {
            x30: entry_point,  // LR = entry point (ret will jump here)
            sp: stack_top,
            // SPSR with EL1h mode, interrupts masked initially
            spsr_el1: 0x3c5, // EL1h, DAIF masked
            ..Self::new()
        }
    }

    /// Create a context for a new userspace thread.
    ///
    /// The thread will start executing at `entry_point` in EL0 with the given
    /// user stack. Kernel stack is used for exception handling.
    pub fn new_user_thread(
        entry_point: u64,
        user_stack_top: u64,
        kernel_stack_top: u64,
    ) -> Self {
        Self {
            sp: kernel_stack_top,      // Kernel SP for exceptions
            sp_el0: user_stack_top,    // User stack pointer
            elr_el1: entry_point,      // Where to jump in userspace
            // SPSR for EL0: mode=0 (EL0t), DAIF clear (interrupts enabled)
            spsr_el1: 0x0,             // EL0t with interrupts enabled
            ..Self::new()
        }
    }
}

// Context switch is implemented in global_asm below
core::arch::global_asm!(r#"
.global switch_context
.type switch_context, @function
switch_context:
    // x0 = old context pointer, x1 = new context pointer

    // Save callee-saved registers to old context
    stp x19, x20, [x0, #0]
    stp x21, x22, [x0, #16]
    stp x23, x24, [x0, #32]
    stp x25, x26, [x0, #48]
    stp x27, x28, [x0, #64]
    stp x29, x30, [x0, #80]
    mov x2, sp
    str x2, [x0, #96]

    // Load callee-saved registers from new context
    ldp x19, x20, [x1, #0]
    ldp x21, x22, [x1, #16]
    ldp x23, x24, [x1, #32]
    ldp x25, x26, [x1, #48]
    ldp x27, x28, [x1, #64]
    ldp x29, x30, [x1, #80]
    ldr x2, [x1, #96]
    mov sp, x2

    // Return to new context (x30 has the return address)
    ret
"#);

extern "C" {
    /// Switch from the current context to a new context.
    ///
    /// # Safety
    ///
    /// Both contexts must be valid and properly initialized.
    pub fn switch_context(old: *mut CpuContext, new: *const CpuContext);
}

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
pub fn save_user_context(ctx: &mut CpuContext, frame: &super::exception_frame::Aarch64ExceptionFrame) {
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
pub fn restore_user_context(frame: &mut super::exception_frame::Aarch64ExceptionFrame, ctx: &CpuContext) {
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

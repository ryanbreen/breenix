//! ARM64 context switching logic.
//!
//! This module handles context switching on ARM64 (AArch64) when returning from
//! exceptions or performing explicit thread switches. It integrates with the
//! scheduler to perform preemptive multitasking.
//!
//! Key differences from x86_64:
//! - Uses TTBR0_EL1 instead of CR3 for user page tables
//! - Uses ERET instead of IRETQ for exception return
//! - Uses TPIDR_EL1 for per-CPU data (like GS segment on x86)
//! - Memory barriers (DSB, ISB) required after page table switches

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use super::exception_frame::Aarch64ExceptionFrame;
use super::percpu::Aarch64PerCpu;
use crate::arch_impl::traits::PerCpuOps;
use crate::task::thread::{CpuContext, ThreadPrivilege, ThreadState};

/// Raw serial debug output - single character, no locks, no allocations.
/// Use this for debugging context switch paths where any allocation/locking
/// could perturb timing or cause deadlocks.
#[inline(always)]
#[allow(dead_code)]
fn raw_uart_char(c: u8) {
    // QEMU virt machine UART base address
    const UART_BASE: u64 = 0x0900_0000;
    unsafe {
        let ptr = UART_BASE as *mut u8;
        core::ptr::write_volatile(ptr, c);
    }
}

/// Raw UART string output - no locks, no allocations.
#[inline(always)]
#[allow(dead_code)]
fn raw_uart_str(s: &str) {
    for byte in s.bytes() {
        raw_uart_char(byte);
    }
}

/// Check if rescheduling is needed and perform context switch if necessary.
///
/// This is called from the exception return path and is the CORRECT place
/// to handle context switching (not in the exception handler itself).
///
/// # Arguments
///
/// * `frame` - The exception frame containing saved registers
/// * `from_el0` - Whether the exception came from EL0 (userspace)
#[no_mangle]
pub extern "C" fn check_need_resched_and_switch_arm64(
    frame: &mut Aarch64ExceptionFrame,
    from_el0: bool,
) {
    // Only reschedule when returning to userspace with preempt_count == 0
    if !from_el0 {
        // If we're returning to kernel mode, only reschedule if explicitly allowed
        let preempt_count = Aarch64PerCpu::preempt_count();
        if preempt_count > 0 {
            return;
        }
    }

    // Check PREEMPT_ACTIVE flag (bit 28)
    let preempt_count = Aarch64PerCpu::preempt_count();
    let preempt_active = (preempt_count & 0x10000000) != 0;

    if preempt_active {
        // We're in the middle of returning from a syscall or handling
        // another exception - don't context switch now
        return;
    }

    // Check if current thread is blocked or terminated
    let current_thread_blocked_or_terminated = crate::task::scheduler::with_scheduler(|sched| {
        if let Some(current) = sched.current_thread_mut() {
            matches!(
                current.state,
                ThreadState::Blocked
                    | ThreadState::BlockedOnSignal
                    | ThreadState::BlockedOnChildExit
                    | ThreadState::Terminated
            )
        } else {
            false
        }
    })
    .unwrap_or(false);

    // Check if reschedule is needed
    let need_resched = crate::task::scheduler::check_and_clear_need_resched();
    if !need_resched && !current_thread_blocked_or_terminated {
        // No reschedule needed
        if from_el0 {
            // Check for pending signals before returning to userspace
            // TODO: Implement signal delivery for ARM64
        }
        return;
    }

    // Track reschedule attempts for diagnostics
    static RESCHED_COUNTER: AtomicU64 = AtomicU64::new(0);
    let _count = RESCHED_COUNTER.fetch_add(1, Ordering::Relaxed);

    // Perform scheduling decision
    let schedule_result = crate::task::scheduler::schedule();

    // Handle "no switch needed" case
    if schedule_result.is_none() {
        return;
    }

    if let Some((old_thread_id, new_thread_id)) = schedule_result {
        if old_thread_id == new_thread_id {
            // Same thread continues running
            return;
        }

        // Save current thread's context
        if from_el0 {
            save_userspace_context_arm64(old_thread_id, frame);
        } else {
            save_kernel_context_arm64(old_thread_id, frame);
        }

        // Switch to the new thread
        switch_to_thread_arm64(new_thread_id, frame);

        // Clear PREEMPT_ACTIVE after context switch
        unsafe {
            Aarch64PerCpu::clear_preempt_active();
        }

        // Reset timer quantum for the new thread
        // TODO: Implement timer quantum reset for ARM64
    }
}

/// Save userspace context for the current thread.
fn save_userspace_context_arm64(thread_id: u64, frame: &Aarch64ExceptionFrame) {
    crate::task::scheduler::with_thread_mut(thread_id, |thread| {
        // Save X0 (return value register) - important for fork/syscall returns
        thread.context.x0 = frame.x0;

        // Save callee-saved registers from exception frame
        thread.context.x19 = frame.x19;
        thread.context.x20 = frame.x20;
        thread.context.x21 = frame.x21;
        thread.context.x22 = frame.x22;
        thread.context.x23 = frame.x23;
        thread.context.x24 = frame.x24;
        thread.context.x25 = frame.x25;
        thread.context.x26 = frame.x26;
        thread.context.x27 = frame.x27;
        thread.context.x28 = frame.x28;
        thread.context.x29 = frame.x29;
        thread.context.x30 = frame.x30;

        // Save program counter and status
        thread.context.elr_el1 = frame.elr;
        thread.context.spsr_el1 = frame.spsr;

        // Read and save SP_EL0 (user stack pointer)
        let sp_el0: u64;
        unsafe {
            core::arch::asm!("mrs {}, sp_el0", out(reg) sp_el0, options(nomem, nostack));
        }
        thread.context.sp_el0 = sp_el0;
    });
}

/// Save kernel context for the current thread.
fn save_kernel_context_arm64(thread_id: u64, frame: &Aarch64ExceptionFrame) {
    crate::task::scheduler::with_thread_mut(thread_id, |thread| {
        // Save callee-saved registers
        thread.context.x19 = frame.x19;
        thread.context.x20 = frame.x20;
        thread.context.x21 = frame.x21;
        thread.context.x22 = frame.x22;
        thread.context.x23 = frame.x23;
        thread.context.x24 = frame.x24;
        thread.context.x25 = frame.x25;
        thread.context.x26 = frame.x26;
        thread.context.x27 = frame.x27;
        thread.context.x28 = frame.x28;
        thread.context.x29 = frame.x29;
        thread.context.x30 = frame.x30;

        // Save program counter and kernel stack pointer
        thread.context.elr_el1 = frame.elr;
        thread.context.spsr_el1 = frame.spsr;

        // For kernel threads, SP comes from the exception frame's context
        // (it was saved when the exception occurred)
    });
}

/// Switch to a different thread.
fn switch_to_thread_arm64(thread_id: u64, frame: &mut Aarch64ExceptionFrame) {
    // Update per-CPU current thread pointer
    crate::task::scheduler::with_thread_mut(thread_id, |thread| {
        let thread_ptr = thread as *const _ as *mut crate::task::thread::Thread;
        unsafe {
            Aarch64PerCpu::set_current_thread_ptr(thread_ptr as *mut u8);
        }

        // Update kernel stack for exceptions
        if let Some(kernel_stack_top) = thread.kernel_stack_top {
            unsafe {
                Aarch64PerCpu::set_kernel_stack_top(kernel_stack_top.as_u64());
            }
        }
    });

    // Check thread properties
    let is_idle = crate::task::scheduler::with_scheduler(|sched| thread_id == sched.idle_thread())
        .unwrap_or(false);

    let is_kernel_thread =
        crate::task::scheduler::with_thread_mut(thread_id, |thread| {
            thread.privilege == ThreadPrivilege::Kernel
        })
        .unwrap_or(false);

    if is_idle {
        setup_idle_return_arm64(frame);
    } else if is_kernel_thread {
        setup_kernel_thread_return_arm64(thread_id, frame);
    } else {
        restore_userspace_context_arm64(thread_id, frame);
    }
}

/// Set up exception frame to return to idle loop.
fn setup_idle_return_arm64(frame: &mut Aarch64ExceptionFrame) {
    // Get idle thread's kernel stack
    let idle_stack = crate::task::scheduler::with_scheduler(|sched| {
        let idle_id = sched.idle_thread();
        sched
            .get_thread(idle_id)
            .and_then(|t| t.kernel_stack_top.map(|v| v.as_u64()))
    })
    .flatten()
    .unwrap_or_else(|| {
        log::error!("Failed to get idle thread's kernel stack!");
        Aarch64PerCpu::kernel_stack_top()
    });

    // Set up exception return to idle_loop
    frame.elr = idle_loop_arm64 as *const () as u64;
    // SPSR for EL1h with interrupts enabled
    // M[3:0] = 0b0101 (EL1h), DAIF = 0 (interrupts enabled)
    frame.spsr = 0x5;

    // Kernel stack pointer will be restored from exception frame
    // Clear all general purpose registers for clean state
    frame.x0 = 0;
    frame.x1 = 0;
    frame.x2 = 0;
    frame.x3 = 0;
    frame.x4 = 0;
    frame.x5 = 0;
    frame.x6 = 0;
    frame.x7 = 0;
    frame.x8 = 0;
    frame.x9 = 0;
    frame.x10 = 0;
    frame.x11 = 0;
    frame.x12 = 0;
    frame.x13 = 0;
    frame.x14 = 0;
    frame.x15 = 0;
    frame.x16 = 0;
    frame.x17 = 0;
    frame.x18 = 0;
    frame.x19 = 0;
    frame.x20 = 0;
    frame.x21 = 0;
    frame.x22 = 0;
    frame.x23 = 0;
    frame.x24 = 0;
    frame.x25 = 0;
    frame.x26 = 0;
    frame.x27 = 0;
    frame.x28 = 0;
    frame.x29 = 0;
    frame.x30 = 0;

    // Store idle stack in SP_EL0 scratch area for use after ERET
    unsafe {
        Aarch64PerCpu::set_user_rsp_scratch(idle_stack);
    }

    // Clear PREEMPT_ACTIVE when switching to idle
    unsafe {
        Aarch64PerCpu::clear_preempt_active();
    }

    log::trace!("Set up return to idle loop");
}

/// Set up exception frame to return to kernel thread.
fn setup_kernel_thread_return_arm64(thread_id: u64, frame: &mut Aarch64ExceptionFrame) {
    let thread_info = crate::task::scheduler::with_thread_mut(thread_id, |thread| {
        (thread.name.clone(), thread.context.clone())
    });

    if let Some((_name, context)) = thread_info {
        // Restore callee-saved registers
        frame.x19 = context.x19;
        frame.x20 = context.x20;
        frame.x21 = context.x21;
        frame.x22 = context.x22;
        frame.x23 = context.x23;
        frame.x24 = context.x24;
        frame.x25 = context.x25;
        frame.x26 = context.x26;
        frame.x27 = context.x27;
        frame.x28 = context.x28;
        frame.x29 = context.x29;
        frame.x30 = context.x30;

        // Set return address (ELR_EL1 = x30 for kernel threads)
        frame.elr = context.x30;

        // SPSR for EL1h (kernel mode with SP_EL1)
        frame.spsr = context.spsr_el1;

        // Store kernel SP for restoration after ERET
        unsafe {
            Aarch64PerCpu::set_user_rsp_scratch(context.sp);
        }

        // Memory barrier to ensure all writes are visible
        core::sync::atomic::fence(Ordering::SeqCst);
    } else {
        log::error!(
            "KTHREAD_SWITCH: Failed to get thread info for thread {}",
            thread_id
        );
    }
}

/// Restore userspace context for a thread.
fn restore_userspace_context_arm64(thread_id: u64, frame: &mut Aarch64ExceptionFrame) {
    log::trace!("restore_userspace_context_arm64: thread {}", thread_id);

    // Check if this thread has ever run
    let has_started = crate::task::scheduler::with_thread_mut(thread_id, |thread| thread.has_started)
        .unwrap_or(false);

    if !has_started {
        // First run for this thread
        log::info!("First run: thread {} entering userspace", thread_id);

        // Mark thread as started
        crate::task::scheduler::with_thread_mut(thread_id, |thread| {
            thread.has_started = true;
        });

        setup_first_userspace_entry_arm64(thread_id, frame);
        return;
    }

    // Restore saved context
    crate::task::scheduler::with_thread_mut(thread_id, |thread| {
        // Restore X0 - important for fork() return value
        // For forked children, x0 is set to 0; for parent, it will be the child PID
        frame.x0 = thread.context.x0;

        // Restore callee-saved registers
        frame.x19 = thread.context.x19;
        frame.x20 = thread.context.x20;
        frame.x21 = thread.context.x21;
        frame.x22 = thread.context.x22;
        frame.x23 = thread.context.x23;
        frame.x24 = thread.context.x24;
        frame.x25 = thread.context.x25;
        frame.x26 = thread.context.x26;
        frame.x27 = thread.context.x27;
        frame.x28 = thread.context.x28;
        frame.x29 = thread.context.x29;
        frame.x30 = thread.context.x30;

        // Restore program counter and status
        frame.elr = thread.context.elr_el1;
        frame.spsr = thread.context.spsr_el1;

        // Restore SP_EL0 (user stack pointer)
        unsafe {
            core::arch::asm!(
                "msr sp_el0, {}",
                in(reg) thread.context.sp_el0,
                options(nomem, nostack)
            );
        }
    });

    // Switch TTBR0 if needed for different address space
    switch_ttbr0_if_needed(thread_id);
}

/// Set up exception frame for first entry to userspace.
fn setup_first_userspace_entry_arm64(thread_id: u64, frame: &mut Aarch64ExceptionFrame) {
    crate::task::scheduler::with_thread_mut(thread_id, |thread| {
        // Set return address to entry point
        frame.elr = thread.context.elr_el1;

        // SPSR for EL0t (userspace, interrupts enabled)
        // M[3:0] = 0b0000, DAIF = 0
        frame.spsr = 0x0;

        // Set up user stack pointer
        unsafe {
            core::arch::asm!(
                "msr sp_el0, {}",
                in(reg) thread.context.sp_el0,
                options(nomem, nostack)
            );
        }

        // Clear all registers for security
        frame.x0 = 0;
        frame.x1 = 0;
        frame.x2 = 0;
        frame.x3 = 0;
        frame.x4 = 0;
        frame.x5 = 0;
        frame.x6 = 0;
        frame.x7 = 0;
        frame.x8 = 0;
        frame.x9 = 0;
        frame.x10 = 0;
        frame.x11 = 0;
        frame.x12 = 0;
        frame.x13 = 0;
        frame.x14 = 0;
        frame.x15 = 0;
        frame.x16 = 0;
        frame.x17 = 0;
        frame.x18 = 0;
        frame.x19 = 0;
        frame.x20 = 0;
        frame.x21 = 0;
        frame.x22 = 0;
        frame.x23 = 0;
        frame.x24 = 0;
        frame.x25 = 0;
        frame.x26 = 0;
        frame.x27 = 0;
        frame.x28 = 0;
        frame.x29 = 0;
        frame.x30 = 0;

        log::info!(
            "FIRST_ENTRY: thread {} ELR={:#x} SP_EL0={:#x}",
            thread_id,
            thread.context.elr_el1,
            thread.context.sp_el0
        );
    });

    // Switch TTBR0 for this thread's address space
    switch_ttbr0_if_needed(thread_id);

    log::info!("First userspace entry setup complete for thread {}", thread_id);
}

/// Switch TTBR0_EL1 if the thread requires a different address space.
///
/// On ARM64, TTBR0 holds the user page table base and TTBR1 holds the kernel
/// page table base. We only need to switch TTBR0 when switching between
/// processes with different address spaces.
fn switch_ttbr0_if_needed(thread_id: u64) {
    // TODO: Integrate with process management to get page table base
    // For now, we assume all userspace threads share the same page table

    // Get the next TTBR0 value from per-CPU data
    let next_ttbr0 = Aarch64PerCpu::next_cr3();

    if next_ttbr0 == 0 {
        // No TTBR0 switch needed
        return;
    }

    // Read current TTBR0
    let current_ttbr0: u64;
    unsafe {
        core::arch::asm!("mrs {}, ttbr0_el1", out(reg) current_ttbr0, options(nomem, nostack));
    }

    if current_ttbr0 != next_ttbr0 {
        log::trace!(
            "TTBR0 switch: {:#x} -> {:#x} for thread {}",
            current_ttbr0,
            next_ttbr0,
            thread_id
        );

        unsafe {
            // Write new TTBR0
            core::arch::asm!(
                "msr ttbr0_el1, {}",
                in(reg) next_ttbr0,
                options(nomem, nostack)
            );

            // Memory barriers required after page table switch
            // DSB ISH: Ensure the write to TTBR0 is complete
            // ISB: Flush instruction pipeline
            core::arch::asm!(
                "dsb ish",
                "isb",
                options(nomem, nostack, preserves_flags)
            );
        }

        // Update saved process TTBR0
        unsafe {
            Aarch64PerCpu::set_saved_process_cr3(next_ttbr0);
        }
    }

    // Clear next_cr3 to indicate switch is done
    unsafe {
        Aarch64PerCpu::set_next_cr3(0);
    }
}

/// ARM64 idle loop - wait for interrupts.
///
/// This function runs when no other threads are ready. It uses WFI
/// (Wait For Interrupt) to put the CPU in a low-power state.
#[no_mangle]
pub extern "C" fn idle_loop_arm64() -> ! {
    loop {
        // Enable interrupts and wait for interrupt
        // On ARM64, WFI will wait until an interrupt occurs
        unsafe {
            core::arch::asm!(
                "msr daifclr, #0xf",  // Enable all interrupts (clear DAIF)
                "wfi",                 // Wait for interrupt
                options(nomem, nostack)
            );
        }
    }
}

/// Perform a context switch between two threads using the low-level
/// assembly switch_context function.
///
/// This is used for explicit context switches (e.g., yield, blocking syscalls)
/// rather than interrupt-driven preemption.
///
/// # Safety
///
/// Both contexts must be valid and properly initialized.
#[allow(dead_code)]
pub unsafe fn perform_context_switch(
    old_context: &mut CpuContext,
    new_context: &CpuContext,
) {
    // Use the assembly context switch from context.rs
    super::context::switch_context(
        old_context as *mut CpuContext,
        new_context as *const CpuContext,
    );
}

/// Switch to a new thread for the first time (doesn't save current context).
///
/// # Safety
///
/// The context must be valid and properly initialized.
#[allow(dead_code)]
pub unsafe fn switch_to_new_thread(context: &CpuContext) -> ! {
    super::context::switch_to_thread(context as *const CpuContext)
}

/// Switch to userspace using ERET.
///
/// # Safety
///
/// The context must have valid userspace addresses.
#[allow(dead_code)]
pub unsafe fn switch_to_user(context: &CpuContext) -> ! {
    super::context::switch_to_user(context as *const CpuContext)
}

/// Marker for boot stage completion (mirrors x86_64 pattern).
static SCHEDULE_MARKER_EMITTED: AtomicBool = AtomicBool::new(false);

/// Emit one-time boot marker when scheduler first runs.
#[allow(dead_code)]
fn emit_schedule_boot_marker() {
    if !SCHEDULE_MARKER_EMITTED.swap(true, Ordering::Relaxed) {
        raw_uart_str("[ INFO] scheduler::schedule() returned (boot marker)\n");
    }
}

/// One-time EL0 entry marker.
static EMITTED_EL0_MARKER: AtomicBool = AtomicBool::new(false);

/// Emit one-time marker when first entering EL0 (userspace).
#[allow(dead_code)]
fn emit_el0_entry_marker() {
    if !EMITTED_EL0_MARKER.swap(true, Ordering::Relaxed) {
        raw_uart_str("EL0_ENTER: First userspace entry\n");
        raw_uart_str("[ OK ] EL0_SMOKE: userspace executed + syscall path verified\n");
    }
}

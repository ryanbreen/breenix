//! Signal delivery to userspace
//!
//! This module handles delivering pending signals to processes when they
//! return to userspace from syscalls or interrupts.
//!
//! Architecture support:
//! - x86_64: Uses InterruptStackFrame and SavedRegisters (RAX-R15)
//! - AArch64: Uses Aarch64ExceptionFrame and SavedRegisters (X0-X30, SP, ELR, SPSR)

use super::constants::*;
use super::types::*;
use crate::process::{Process, ProcessState};

/// Check if a process has any deliverable signals
///
/// This is a fast O(1) check suitable for the hot path in context_switch.rs
#[inline]
pub fn has_deliverable_signals(process: &Process) -> bool {
    process.signals.has_deliverable_signals()
}

/// Result of signal delivery
pub enum SignalDeliveryResult {
    /// No signals were delivered
    NoAction,
    /// Signal was delivered, process state may have changed
    Delivered,
    /// Process was terminated - caller should notify parent after releasing lock
    Terminated(ParentNotification),
}

// =============================================================================
// x86_64 Signal Delivery
// =============================================================================

/// Deliver pending signals to a process (x86_64)
///
/// Called from check_need_resched_and_switch() before returning to userspace.
///
/// # Arguments
/// * `process` - The process to deliver signals to
/// * `interrupt_frame` - The interrupt frame that will be used to return to userspace
/// * `saved_regs` - The saved general-purpose registers
///
/// # Returns
/// * `SignalDeliveryResult` indicating what action was taken
///
/// IMPORTANT: If `Terminated` is returned, the caller MUST call
/// `notify_parent_of_termination_deferred` AFTER releasing the process manager lock!
#[cfg(target_arch = "x86_64")]
pub fn deliver_pending_signals(
    process: &mut Process,
    interrupt_frame: &mut x86_64::structures::idt::InterruptStackFrame,
    saved_regs: &mut crate::task::process_context::SavedRegisters,
) -> SignalDeliveryResult {
    // Process all deliverable signals in a loop (avoids unbounded recursion)
    loop {
        // Get next deliverable signal
        let sig = match process.signals.next_deliverable_signal() {
            Some(s) => s,
            None => return SignalDeliveryResult::NoAction,
        };

        // Clear pending flag for this signal
        process.signals.clear_pending(sig);

        // Get the handler for this signal
        let action = *process.signals.get_handler(sig);

        log::debug!(
            "Delivering signal {} ({}) to process {}, handler={:#x}",
            sig,
            signal_name(sig),
            process.id.as_u64(),
            action.handler
        );

        match action.handler {
            SIG_DFL => {
                // Default action may terminate/stop the process
                match deliver_default_action(process, sig) {
                    DeliverResult::Delivered => return SignalDeliveryResult::Delivered,
                    DeliverResult::Terminated(notification) => return SignalDeliveryResult::Terminated(notification),
                    DeliverResult::Ignored => {
                        // Continue loop to check for more signals
                    }
                }
            }
            SIG_IGN => {
                log::debug!(
                    "Signal {} ignored by process {}",
                    sig,
                    process.id.as_u64()
                );
                // Signal ignored - continue loop to check for more signals
            }
            handler_addr => {
                // User-defined handler - set up signal frame and return
                // Only one user handler can be delivered at a time
                if deliver_to_user_handler_x86_64(process, interrupt_frame, saved_regs, sig, handler_addr, &action) {
                    return SignalDeliveryResult::Delivered;
                }
                return SignalDeliveryResult::NoAction;
            }
        }
    }
}

// =============================================================================
// ARM64 Signal Delivery
// =============================================================================

/// Deliver pending signals to a process (ARM64)
///
/// Called from check_need_resched_and_switch() before returning to userspace.
///
/// # Arguments
/// * `process` - The process to deliver signals to
/// * `exception_frame` - The exception frame that will be used to return to userspace
/// * `saved_regs` - The saved general-purpose registers
///
/// # Returns
/// * `SignalDeliveryResult` indicating what action was taken
///
/// IMPORTANT: If `Terminated` is returned, the caller MUST call
/// `notify_parent_of_termination_deferred` AFTER releasing the process manager lock!
#[cfg(target_arch = "aarch64")]
pub fn deliver_pending_signals(
    process: &mut Process,
    exception_frame: &mut crate::arch_impl::aarch64::exception_frame::Aarch64ExceptionFrame,
    saved_regs: &mut crate::task::process_context::SavedRegisters,
) -> SignalDeliveryResult {
    // Process all deliverable signals in a loop (avoids unbounded recursion)
    loop {
        // Get next deliverable signal
        let sig = match process.signals.next_deliverable_signal() {
            Some(s) => s,
            None => return SignalDeliveryResult::NoAction,
        };

        // Clear pending flag for this signal
        process.signals.clear_pending(sig);

        // Get the handler for this signal
        let action = *process.signals.get_handler(sig);

        log::debug!(
            "Delivering signal {} ({}) to process {}, handler={:#x}",
            sig,
            signal_name(sig),
            process.id.as_u64(),
            action.handler
        );

        match action.handler {
            SIG_DFL => {
                // Default action may terminate/stop the process
                match deliver_default_action(process, sig) {
                    DeliverResult::Delivered => return SignalDeliveryResult::Delivered,
                    DeliverResult::Terminated(notification) => return SignalDeliveryResult::Terminated(notification),
                    DeliverResult::Ignored => {
                        // Continue loop to check for more signals
                    }
                }
            }
            SIG_IGN => {
                log::debug!(
                    "Signal {} ignored by process {}",
                    sig,
                    process.id.as_u64()
                );
                // Signal ignored - continue loop to check for more signals
            }
            handler_addr => {
                // User-defined handler - set up signal frame and return
                // Only one user handler can be delivered at a time
                if deliver_to_user_handler_aarch64(process, exception_frame, saved_regs, sig, handler_addr, &action) {
                    return SignalDeliveryResult::Delivered;
                }
                return SignalDeliveryResult::NoAction;
            }
        }
    }
}

// =============================================================================
// Common Signal Delivery Logic
// =============================================================================

/// Result of delivering a signal's default action
pub enum DeliverResult {
    /// Signal was delivered, process state may have changed
    Delivered,
    /// Signal was ignored or no action needed
    Ignored,
    /// Process was terminated - caller should notify parent after releasing lock
    Terminated(ParentNotification),
}

/// Deliver a signal's default action
/// Returns DeliverResult indicating what action was taken
fn deliver_default_action(process: &mut Process, sig: u32) -> DeliverResult {
    match default_action(sig) {
        SignalDefaultAction::Terminate => {
            log::info!(
                "Process {} terminated by signal {} ({})",
                process.id.as_u64(),
                sig,
                signal_name(sig)
            );
            // Exit code for signal termination is typically 128 + signal number
            // But we use negative signal number to indicate signal death
            process.terminate(-(sig as i32));

            // CRITICAL: Also mark the scheduler's copy of the thread as terminated.
            // The process.terminate() call above marks process.main_thread, but
            // the scheduler has its own copy of threads in its threads vector.
            // Without this, the scheduler would keep scheduling the terminated thread!
            if let Some(ref thread) = process.main_thread {
                let thread_id = thread.id();
                crate::task::scheduler::with_thread_mut(thread_id, |sched_thread| {
                    sched_thread.set_terminated();
                    log::info!(
                        "Signal delivery: marked scheduler thread {} as Terminated",
                        thread_id
                    );
                });
            }

            // Return notification info for parent - caller will notify after releasing lock
            if let Some(notification) = notify_parent_of_termination(process) {
                DeliverResult::Terminated(notification)
            } else {
                DeliverResult::Delivered
            }
        }
        SignalDefaultAction::CoreDump => {
            log::info!(
                "Process {} killed (core dumped) by signal {} ({})",
                process.id.as_u64(),
                sig,
                signal_name(sig)
            );
            // Core dump not implemented, just terminate
            // The 0x80 flag indicates core dump
            process.terminate(-((sig as i32) | 0x80));

            // CRITICAL: Also mark the scheduler's copy of the thread as terminated.
            if let Some(ref thread) = process.main_thread {
                let thread_id = thread.id();
                crate::task::scheduler::with_thread_mut(thread_id, |sched_thread| {
                    sched_thread.set_terminated();
                    log::info!(
                        "Signal delivery: marked scheduler thread {} as Terminated (core dump)",
                        thread_id
                    );
                });
            }

            // Return notification info for parent - caller will notify after releasing lock
            if let Some(notification) = notify_parent_of_termination(process) {
                DeliverResult::Terminated(notification)
            } else {
                DeliverResult::Delivered
            }
        }
        SignalDefaultAction::Stop => {
            log::info!(
                "Process {} stopped by signal {} ({})",
                process.id.as_u64(),
                sig,
                signal_name(sig)
            );
            process.set_blocked();
            DeliverResult::Delivered
        }
        SignalDefaultAction::Continue => {
            log::info!(
                "Process {} continued by signal {} ({})",
                process.id.as_u64(),
                sig,
                signal_name(sig)
            );
            // Only change state if process was stopped
            if matches!(process.state, ProcessState::Blocked) {
                process.set_ready();
                DeliverResult::Delivered
            } else {
                DeliverResult::Ignored
            }
        }
        SignalDefaultAction::Ignore => {
            log::debug!(
                "Signal {} ({}) ignored (default) by process {}",
                sig,
                signal_name(sig),
                process.id.as_u64()
            );
            DeliverResult::Ignored
        }
    }
}

// =============================================================================
// x86_64 User Handler Delivery
// =============================================================================

/// Set up user stack and registers to call a user-defined signal handler (x86_64)
///
/// This modifies the interrupt frame so that when we return to userspace,
/// we jump to the signal handler instead of the interrupted code.
#[cfg(target_arch = "x86_64")]
fn deliver_to_user_handler_x86_64(
    process: &mut Process,
    interrupt_frame: &mut x86_64::structures::idt::InterruptStackFrame,
    saved_regs: &mut crate::task::process_context::SavedRegisters,
    sig: u32,
    handler_addr: u64,
    action: &SignalAction,
) -> bool {
    // Get current user stack pointer from interrupt frame
    let current_rsp = interrupt_frame.stack_pointer.as_u64();
    let original_rsp = current_rsp;

    // Check if we should use the alternate signal stack
    // SA_ONSTACK flag means use alt stack if one is configured and enabled
    let use_alt_stack = (action.flags & SA_ONSTACK) != 0
        && (process.signals.alt_stack.flags & super::constants::SS_DISABLE as u32) == 0
        && process.signals.alt_stack.size > 0
        && !process.signals.alt_stack.on_stack; // Don't nest on alt stack

    let user_rsp = if use_alt_stack {
        // Use alternate stack - stack grows down, so start at top (base + size)
        let alt_top = process.signals.alt_stack.base + process.signals.alt_stack.size as u64;
        log::debug!(
            "Using alternate signal stack: base={:#x}, size={}, top={:#x}",
            process.signals.alt_stack.base,
            process.signals.alt_stack.size,
            alt_top
        );
        // Mark that we're now on the alternate stack
        process.signals.alt_stack.on_stack = true;
        alt_top
    } else {
        current_rsp
    };

    // Calculate space needed for signal frame (and optionally trampoline)
    let frame_size = SignalFrame::SIZE as u64;

    // Check if the handler provides a restorer function (SA_RESTORER flag)
    // If so, use it instead of writing trampoline to the stack.
    // This is essential for signals delivered on alternate stacks where the
    // stack may not be executable (NX bit set).
    let use_restorer = (action.flags & super::constants::SA_RESTORER) != 0 && action.restorer != 0;

    let (frame_rsp, return_addr) = if use_restorer {
        // Use the restorer function provided by the application/libc
        // Only allocate space for the signal frame (no trampoline needed)
        let frame_rsp = (user_rsp - frame_size) & !0xF; // 16-byte align
        log::debug!(
            "Using SA_RESTORER: restorer={:#x}",
            action.restorer
        );
        (frame_rsp, action.restorer)
    } else {
        // Fall back to writing trampoline on the stack
        // This works when the stack is executable (main stack without NX)
        let trampoline_size = super::trampoline::SIGNAL_TRAMPOLINE_SIZE as u64;
        let total_size = frame_size + trampoline_size;
        let frame_rsp = (user_rsp - total_size) & !0xF; // 16-byte align
        let trampoline_rsp = frame_rsp + frame_size;

        // Write trampoline code to user stack
        // SAFETY: We're writing to user memory that should be valid stack space
        unsafe {
            let trampoline_ptr = trampoline_rsp as *mut u8;
            core::ptr::copy_nonoverlapping(
                super::trampoline::SIGNAL_TRAMPOLINE.as_ptr(),
                trampoline_ptr,
                super::trampoline::SIGNAL_TRAMPOLINE_SIZE,
            );
        }

        (frame_rsp, trampoline_rsp)
    };

    // Build signal frame with saved context
    let signal_frame = SignalFrame {
        // Return address: either restorer function or trampoline on stack
        // When the handler does 'ret', it will pop this and jump there
        // MUST BE AT OFFSET 0 in the struct - verified by struct definition
        trampoline_addr: return_addr,

        // Magic number for integrity validation
        magic: SignalFrame::MAGIC,

        // Signal info
        signal: sig as u64,
        siginfo_ptr: 0, // Not implemented yet
        ucontext_ptr: 0, // Not implemented yet

        // Save current execution state
        saved_rip: interrupt_frame.instruction_pointer.as_u64(),
        saved_rsp: original_rsp,
        saved_rflags: interrupt_frame.cpu_flags.bits(),

        // Save all general-purpose registers
        saved_rax: saved_regs.rax,
        saved_rbx: saved_regs.rbx,
        saved_rcx: saved_regs.rcx,
        saved_rdx: saved_regs.rdx,
        saved_rdi: saved_regs.rdi,
        saved_rsi: saved_regs.rsi,
        saved_rbp: saved_regs.rbp,
        saved_r8: saved_regs.r8,
        saved_r9: saved_regs.r9,
        saved_r10: saved_regs.r10,
        saved_r11: saved_regs.r11,
        saved_r12: saved_regs.r12,
        saved_r13: saved_regs.r13,
        saved_r14: saved_regs.r14,
        saved_r15: saved_regs.r15,

        // Save signal mask to restore after handler
        saved_blocked: process.signals.blocked,
    };

    // Write signal frame to user stack
    // SAFETY: We're writing to user memory that should be valid stack space
    unsafe {
        let frame_ptr = frame_rsp as *mut SignalFrame;
        core::ptr::write_volatile(frame_ptr, signal_frame);
    }

    // Block signals during handler execution
    if (action.flags & SA_NODEFER) == 0 {
        // Block this signal while handler runs (prevents recursive delivery)
        process.signals.block_signals(sig_mask(sig));
    }
    // Also block any signals specified in the handler's mask
    process.signals.block_signals(action.mask);

    // Modify interrupt frame to jump to signal handler
    unsafe {
        interrupt_frame.as_mut().update(|frame| {
            frame.instruction_pointer = x86_64::VirtAddr::new(handler_addr);
            frame.stack_pointer = x86_64::VirtAddr::new(frame_rsp);
            // Keep same code segment, stack segment, and flags
        });
    }

    // Set up arguments for signal handler
    // void handler(int signum, siginfo_t *info, void *ucontext)
    saved_regs.rdi = sig as u64;           // First argument: signal number
    saved_regs.rsi = 0;                     // Second argument: siginfo_t* (not implemented)
    saved_regs.rdx = 0;                     // Third argument: ucontext_t* (not implemented)

    if use_alt_stack {
        log::info!(
            "Signal {} delivered to handler at {:#x} on ALTERNATE STACK, RSP={:#x}->{:#x}, return={:#x}",
            sig,
            handler_addr,
            user_rsp,
            frame_rsp,
            return_addr
        );
    } else {
        log::info!(
            "Signal {} delivered to handler at {:#x}, RSP={:#x}->{:#x}, return={:#x}",
            sig,
            handler_addr,
            user_rsp,
            frame_rsp,
            return_addr
        );
    }

    true
}

// =============================================================================
// ARM64 User Handler Delivery
// =============================================================================

/// Set up user stack and registers to call a user-defined signal handler (ARM64)
///
/// This modifies the exception frame so that when we return to userspace,
/// we jump to the signal handler instead of the interrupted code.
///
/// Key differences from x86_64:
/// - User stack is accessed via SP_EL0, not from the exception frame
/// - Return address goes in X30 (link register), not pushed on stack
/// - PSTATE is used instead of RFLAGS
/// - Signal trampoline uses `mov x8, #15; svc #0` for sigreturn
#[cfg(target_arch = "aarch64")]
fn deliver_to_user_handler_aarch64(
    process: &mut Process,
    exception_frame: &mut crate::arch_impl::aarch64::exception_frame::Aarch64ExceptionFrame,
    saved_regs: &mut crate::task::process_context::SavedRegisters,
    sig: u32,
    handler_addr: u64,
    action: &SignalAction,
) -> bool {
    // Get current user stack pointer from saved registers
    // On ARM64, user SP is in SP_EL0, which we save in saved_regs.sp
    let current_sp = saved_regs.sp;
    let original_sp = current_sp;

    // Check if we should use the alternate signal stack
    // SA_ONSTACK flag means use alt stack if one is configured and enabled
    let use_alt_stack = (action.flags & SA_ONSTACK) != 0
        && (process.signals.alt_stack.flags & super::constants::SS_DISABLE as u32) == 0
        && process.signals.alt_stack.size > 0
        && !process.signals.alt_stack.on_stack; // Don't nest on alt stack

    let user_sp = if use_alt_stack {
        // Use alternate stack - stack grows down, so start at top (base + size)
        let alt_top = process.signals.alt_stack.base + process.signals.alt_stack.size as u64;
        log::debug!(
            "Using alternate signal stack: base={:#x}, size={}, top={:#x}",
            process.signals.alt_stack.base,
            process.signals.alt_stack.size,
            alt_top
        );
        // Mark that we're now on the alternate stack
        process.signals.alt_stack.on_stack = true;
        alt_top
    } else {
        current_sp
    };

    // Calculate space needed for signal frame (and optionally trampoline)
    let frame_size = SignalFrame::SIZE as u64;

    // Check if the handler provides a restorer function (SA_RESTORER flag)
    // If so, use it instead of writing trampoline to the stack.
    let use_restorer = (action.flags & super::constants::SA_RESTORER) != 0 && action.restorer != 0;

    let (frame_sp, return_addr) = if use_restorer {
        // Use the restorer function provided by the application/libc
        // Only allocate space for the signal frame (no trampoline needed)
        let frame_sp = (user_sp - frame_size) & !0xF; // 16-byte align
        log::debug!(
            "Using SA_RESTORER: restorer={:#x}",
            action.restorer
        );
        (frame_sp, action.restorer)
    } else {
        // Fall back to writing trampoline on the stack
        // This works when the stack is executable (main stack without NX)
        let trampoline_size = super::trampoline::SIGNAL_TRAMPOLINE_SIZE as u64;
        let total_size = frame_size + trampoline_size;
        let frame_sp = (user_sp - total_size) & !0xF; // 16-byte align
        let trampoline_sp = frame_sp + frame_size;

        // Write trampoline code to user stack
        // SAFETY: We're writing to user memory that should be valid stack space
        unsafe {
            let trampoline_ptr = trampoline_sp as *mut u8;
            core::ptr::copy_nonoverlapping(
                super::trampoline::SIGNAL_TRAMPOLINE.as_ptr(),
                trampoline_ptr,
                super::trampoline::SIGNAL_TRAMPOLINE_SIZE,
            );
        }

        (frame_sp, trampoline_sp)
    };

    // Build signal frame with saved context
    // Copy all X registers to the saved_x array
    let saved_x: [u64; 31] = [
        saved_regs.x0, saved_regs.x1, saved_regs.x2, saved_regs.x3,
        saved_regs.x4, saved_regs.x5, saved_regs.x6, saved_regs.x7,
        saved_regs.x8, saved_regs.x9, saved_regs.x10, saved_regs.x11,
        saved_regs.x12, saved_regs.x13, saved_regs.x14, saved_regs.x15,
        saved_regs.x16, saved_regs.x17, saved_regs.x18, saved_regs.x19,
        saved_regs.x20, saved_regs.x21, saved_regs.x22, saved_regs.x23,
        saved_regs.x24, saved_regs.x25, saved_regs.x26, saved_regs.x27,
        saved_regs.x28, saved_regs.x29, saved_regs.x30,
    ];

    let signal_frame = SignalFrame {
        // Return address stored in x30/lr on ARM64
        trampoline_addr: return_addr,

        // Magic number for integrity validation
        magic: SignalFrame::MAGIC,

        // Signal info
        signal: sig as u64,
        siginfo_ptr: 0, // Not implemented yet
        ucontext_ptr: 0, // Not implemented yet

        // Save current execution state (ARM64 specific)
        saved_pc: saved_regs.elr,      // Program counter (ELR_EL1)
        saved_sp: original_sp,           // Stack pointer
        saved_pstate: saved_regs.spsr,  // Processor state (SPSR_EL1)

        // Save all general-purpose registers (X0-X30)
        saved_x,

        // Save signal mask to restore after handler
        saved_blocked: process.signals.blocked,
    };

    // Write signal frame to user stack
    // SAFETY: We're writing to user memory that should be valid stack space
    unsafe {
        let frame_ptr = frame_sp as *mut SignalFrame;
        core::ptr::write_volatile(frame_ptr, signal_frame);
    }

    // Block signals during handler execution
    if (action.flags & SA_NODEFER) == 0 {
        // Block this signal while handler runs (prevents recursive delivery)
        process.signals.block_signals(sig_mask(sig));
    }
    // Also block any signals specified in the handler's mask
    process.signals.block_signals(action.mask);

    // Modify exception frame to jump to signal handler
    // Set PC (ELR_EL1) to handler address
    exception_frame.elr = handler_addr;

    // Set X30 (link register) to the return address (trampoline or restorer)
    // When the handler returns (via RET instruction), it will jump to x30
    exception_frame.x30 = return_addr;
    saved_regs.x30 = return_addr;

    // Update stack pointer in saved registers
    // The actual SP_EL0 update happens on exception return
    saved_regs.sp = frame_sp;
    saved_regs.elr = handler_addr;

    // Set up arguments for signal handler (ARM64 ABI: X0-X2)
    // void handler(int signum, siginfo_t *info, void *ucontext)
    exception_frame.x0 = sig as u64;        // First argument: signal number
    exception_frame.x1 = 0;                  // Second argument: siginfo_t* (not implemented)
    exception_frame.x2 = 0;                  // Third argument: ucontext_t* (not implemented)
    saved_regs.x0 = sig as u64;
    saved_regs.x1 = 0;
    saved_regs.x2 = 0;

    if use_alt_stack {
        log::info!(
            "Signal {} delivered to handler at {:#x} on ALTERNATE STACK, SP={:#x}->{:#x}, return={:#x}",
            sig,
            handler_addr,
            user_sp,
            frame_sp,
            return_addr
        );
    } else {
        log::info!(
            "Signal {} delivered to handler at {:#x}, SP={:#x}->{:#x}, return={:#x}",
            sig,
            handler_addr,
            user_sp,
            frame_sp,
            return_addr
        );
    }

    true
}

// =============================================================================
// Parent Notification (Architecture-Independent)
// =============================================================================

/// Store information about parent notification that needs to happen after lock is released
///
/// This is used to defer parent notification until after the process manager lock is released,
/// avoiding deadlocks when signal delivery happens while the manager lock is held.
pub struct ParentNotification {
    pub parent_pid: crate::process::ProcessId,
    pub child_pid: crate::process::ProcessId,
}

/// Notify parent process when a child process is terminated by signal
///
/// This function:
/// 1. Sends SIGCHLD to the parent process
/// 2. Unblocks the parent's main thread if it's blocked on waitpid
///
/// This is critical for waitpid() to work correctly when children are killed by signals.
///
/// IMPORTANT: This function must be called AFTER the process manager lock is released!
/// It will try to acquire the lock internally, so calling it while the lock is held
/// will cause a deadlock.
pub fn notify_parent_of_termination_deferred(notification: &ParentNotification) {
    let parent_pid = notification.parent_pid;
    let child_pid = notification.child_pid;

    log::info!(
        "notify_parent_of_termination_deferred: notifying parent {} about child {} termination",
        parent_pid.as_u64(),
        child_pid.as_u64()
    );

    // Get process manager to find and update parent
    // This is safe because we're called after the caller released their lock
    let parent_thread_id = {
        let mut manager_guard = crate::process::manager();
        let Some(ref mut manager) = *manager_guard else {
            log::warn!("notify_parent_of_termination_deferred: no process manager");
            return;
        };

        // Find parent process and send SIGCHLD
        if let Some(parent_process) = manager.get_process_mut(parent_pid) {
            // Send SIGCHLD to parent
            parent_process.signals.set_pending(SIGCHLD);
            log::debug!(
                "notify_parent_of_termination_deferred: sent SIGCHLD to parent {} for child {} termination",
                parent_pid.as_u64(),
                child_pid.as_u64()
            );

            // Get parent's main thread ID for unblocking
            parent_process.main_thread.as_ref().map(|t| t.id)
        } else {
            log::warn!(
                "notify_parent_of_termination_deferred: parent process {} not found for child {}",
                parent_pid.as_u64(),
                child_pid.as_u64()
            );
            None
        }
        // manager_guard is dropped here
    };

    // Unblock parent thread if it's waiting on waitpid
    if let Some(parent_tid) = parent_thread_id {
        crate::task::scheduler::with_scheduler(|sched| {
            sched.unblock_for_child_exit(parent_tid);
        });
        log::info!(
            "notify_parent_of_termination_deferred: unblocked parent thread {} for child {} termination",
            parent_tid,
            child_pid.as_u64()
        );
    }
}

/// Internal function called from deliver_default_action
/// Returns parent notification info if parent should be notified (does NOT acquire lock)
fn notify_parent_of_termination(process: &Process) -> Option<ParentNotification> {
    let parent_pid = process.parent?;

    log::debug!(
        "notify_parent_of_termination: process {} has parent {}, notification queued",
        process.id.as_u64(),
        parent_pid.as_u64()
    );

    Some(ParentNotification {
        parent_pid,
        child_pid: process.id,
    })
}

// =============================================================================
// Timer Functions (Architecture-Independent)
// =============================================================================

/// Check if a process has an expired ITIMER_REAL and queue SIGALRM if needed
///
/// This function is called before signal delivery to tick the process's
/// interval timer. If the timer expires, it queues SIGALRM for delivery.
/// The timer automatically rearms if it has an interval set.
///
/// Returns true if SIGALRM was queued.
#[inline]
pub fn check_and_fire_itimer_real(process: &mut Process, elapsed_usec: u64) -> bool {
    if process.itimers.real.is_active() {
        if process.itimers.real.tick(elapsed_usec) {
            // Timer expired - queue SIGALRM
            process.signals.set_pending(SIGALRM);
            log::debug!(
                "ITIMER_REAL fired for process {} (elapsed {} usec)",
                process.id.as_u64(),
                elapsed_usec
            );
            return true;
        }
    }
    false
}

/// Check if a process has an expired alarm and queue SIGALRM if needed
///
/// This function is called before signal delivery to check if the process's
/// alarm timer has expired. If so, it queues SIGALRM for delivery.
///
/// Returns true if SIGALRM was queued.
#[inline]
pub fn check_and_fire_alarm(process: &mut Process) -> bool {
    if let Some(deadline) = process.alarm_deadline {
        let current_ticks = crate::time::get_ticks();
        if current_ticks >= deadline {
            // Alarm expired - clear it and queue SIGALRM
            process.alarm_deadline = None;
            process.signals.set_pending(SIGALRM);
            log::debug!(
                "Alarm fired for process {} at tick {}",
                process.id.as_u64(),
                current_ticks
            );
            return true;
        }
    }
    false
}

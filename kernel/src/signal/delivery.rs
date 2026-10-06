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
use crate::memory::process_memory::ProcessPageTable;
use crate::process::{Process, ProcessState};

/// Check for pending, unblocked signals with an observable disposition.
/// Ignored dispositions are filtered by SignalState's cached mask.
#[inline]
pub fn has_deliverable_signals(process: &Process) -> bool {
    process.signals.has_deliverable_signals()
}

/// Interruptible waits use the same eligibility check as signal delivery.
#[inline]
pub fn has_interrupting_signals(process: &Process) -> bool {
    process.signals.has_interrupting_signals()
}

/// Result of signal delivery
pub enum SignalDeliveryResult {
    /// No signals were delivered
    NoAction,
    /// Signal was delivered, process state may have changed
    Delivered,
    /// Process was terminated - caller should notify parent after releasing lock
    Terminated(ParentNotification),
    /// A caught signal's frame could not be installed on the user stack. The
    /// thread is no longer runnable and its SIGSEGV exit is deferred; the
    /// caller must not return it to user mode.
    FrameFault,
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
/// * `shared_table` - The owner's page table when `process` is a CLONE_VM thread
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
    mut shared_table: Option<&mut ProcessPageTable>,
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

        // Select under the temporary wait mask, then restore before creating
        // the handler frame or stopping. Each nested frame owns its own mask.
        if let Some(saved) = process.signals.sigsuspend_saved_mask.take() {
            process.signals.set_blocked(saved);
        }

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
                    DeliverResult::Terminated(notification) => {
                        return SignalDeliveryResult::Terminated(notification)
                    }
                    DeliverResult::Ignored => {
                        // Continue loop to check for more signals
                    }
                }
            }
            SIG_IGN => {
                log::debug!("Signal {} ignored by process {}", sig, process.id.as_u64());
                // Signal ignored - continue loop to check for more signals
            }
            handler_addr => {
                // User-defined handler - set up signal frame and return
                // Only one user handler can be delivered at a time
                if deliver_to_user_handler_x86_64(
                    process,
                    &mut shared_table,
                    interrupt_frame,
                    saved_regs,
                    sig,
                    handler_addr,
                    &action,
                ) {
                    return SignalDeliveryResult::Delivered;
                }
                return defer_frame_fault_exit(process);
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
/// * `shared_table` - The owner's page table when `process` is a CLONE_VM thread
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
    mut shared_table: Option<&mut ProcessPageTable>,
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

        // Select under the temporary wait mask, then restore before creating
        // the handler frame or stopping. Each nested frame owns its own mask.
        if let Some(saved) = process.signals.sigsuspend_saved_mask.take() {
            process.signals.set_blocked(saved);
        }

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
                    DeliverResult::Terminated(notification) => {
                        return SignalDeliveryResult::Terminated(notification)
                    }
                    DeliverResult::Ignored => {
                        // Continue loop to check for more signals
                    }
                }
            }
            SIG_IGN => {
                log::debug!("Signal {} ignored by process {}", sig, process.id.as_u64());
                // Signal ignored - continue loop to check for more signals
            }
            handler_addr => {
                // User-defined handler - set up signal frame and return
                // Only one user handler can be delivered at a time
                if deliver_to_user_handler_aarch64(
                    process,
                    &mut shared_table,
                    exception_frame,
                    saved_regs,
                    sig,
                    handler_addr,
                    &action,
                ) {
                    return SignalDeliveryResult::Delivered;
                }
                return defer_frame_fault_exit(process);
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

/// An unusable signal stack cannot silently discard a caught signal: the
/// process dies as a bad user-stack access, through the deferred SIGSEGV exit a
/// kernel-mode user fault takes. Nothing is torn down under PROCESS_MANAGER
/// here. The drain runs `handle_thread_exit`, which closes descriptors, tells
/// and reparents, and retires the row; the scheduler never dispatches the
/// thread again.
fn defer_frame_fault_exit(process: &Process) -> SignalDeliveryResult {
    if let Some(thread_id) = process.main_thread.as_ref().map(|thread| thread.id) {
        let _ = crate::task::process_task::defer_fault_sigsegv_exit(thread_id);
        crate::task::scheduler::with_thread_mut(thread_id, |thread| thread.set_terminated());
    }
    SignalDeliveryResult::FrameFault
}

/// Copy `bytes` onto the user stack through the table of the address space the
/// thread runs in: its own, or for a CLONE_VM thread the owner's.
fn write_signal_stack(
    process: &mut Process,
    shared_table: &mut Option<&mut ProcessPageTable>,
    addr: u64,
    bytes: &[u8],
) -> bool {
    let pid = process.id.as_u64();
    match process.page_table.as_deref_mut() {
        Some(table) => table.write_user_memory(addr, bytes, pid),
        None => shared_table
            .as_deref_mut()
            .is_some_and(|table| table.write_user_memory(addr, bytes, pid)),
    }
}

/// Deliver a signal's default action
/// Returns DeliverResult indicating what action was taken
fn deliver_default_action(process: &mut Process, sig: u32) -> DeliverResult {
    match default_action(sig) {
        SignalDefaultAction::Terminate => {
            crate::serial_println!(
                "[signal] Process {} ({}) terminated by signal {} ({})",
                process.id.as_u64(),
                process.name,
                sig,
                signal_name(sig)
            );
            // Exit code for signal termination is typically 128 + signal number
            // But we use negative signal number to indicate signal death
            crate::trace_count!(crate::tracing::providers::teardown::TEARDOWN_ENTRY_SIGNAL);
            process.terminate(-(sig as i32));

            // CRITICAL: Also mark the scheduler's copy of the thread as terminated.
            // The process.terminate() call above marks process.main_thread, but
            // the scheduler has its own copy of threads in its threads vector.
            // Without this, the scheduler would keep scheduling the terminated thread!
            if let Some(ref thread) = process.main_thread {
                let thread_id = thread.id();
                let marked = crate::task::scheduler::with_thread_mut(thread_id, |sched_thread| {
                    sched_thread.set_terminated();
                });
                // Logged after the scheduler lock is released.
                if marked.is_some() {
                    log::info!(
                        "Signal delivery: marked scheduler thread {} as Terminated",
                        thread_id
                    );
                }
            }

            // Return notification info for parent - caller will notify after releasing lock
            if let Some(notification) = notify_parent_of_termination(process) {
                DeliverResult::Terminated(notification)
            } else {
                DeliverResult::Delivered
            }
        }
        SignalDefaultAction::CoreDump => {
            crate::serial_println!(
                "[signal] Process {} ({}) killed (core dump) by signal {} ({})",
                process.id.as_u64(),
                process.name,
                sig,
                signal_name(sig)
            );
            // Core dump not implemented, just terminate
            // The 0x80 flag indicates core dump
            crate::trace_count!(crate::tracing::providers::teardown::TEARDOWN_ENTRY_SIGNAL);
            process.terminate(-((sig as i32) | 0x80));

            // CRITICAL: Also mark the scheduler's copy of the thread as terminated.
            if let Some(ref thread) = process.main_thread {
                let thread_id = thread.id();
                let marked = crate::task::scheduler::with_thread_mut(thread_id, |sched_thread| {
                    sched_thread.set_terminated();
                });
                // Logged after the scheduler lock is released.
                if marked.is_some() {
                    log::info!(
                        "Signal delivery: marked scheduler thread {} as Terminated (core dump)",
                        thread_id
                    );
                }
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

/// The user-mode return context an x86-64 signal handler is installed into:
/// an interrupt frame's, or the syscall return frame's.
#[cfg(target_arch = "x86_64")]
pub struct X86UserReturn {
    pub rip: u64,
    pub rsp: u64,
    pub rflags: u64,
}

/// Set up user stack and registers to call a user-defined signal handler (x86_64)
///
/// This modifies the interrupt frame so that when we return to userspace,
/// we jump to the signal handler instead of the interrupted code.
#[cfg(target_arch = "x86_64")]
fn deliver_to_user_handler_x86_64(
    process: &mut Process,
    shared_table: &mut Option<&mut ProcessPageTable>,
    interrupt_frame: &mut x86_64::structures::idt::InterruptStackFrame,
    saved_regs: &mut crate::task::process_context::SavedRegisters,
    sig: u32,
    handler_addr: u64,
    action: &SignalAction,
) -> bool {
    let mut user_return = X86UserReturn {
        rip: interrupt_frame.instruction_pointer.as_u64(),
        rsp: interrupt_frame.stack_pointer.as_u64(),
        rflags: interrupt_frame.cpu_flags.bits(),
    };
    if !install_user_handler_x86_64(
        process,
        shared_table,
        &mut user_return,
        saved_regs,
        sig,
        handler_addr,
        action,
    ) {
        return false;
    }
    // The installer accepted both addresses as canonical.
    unsafe {
        interrupt_frame.as_mut().update(|frame| {
            frame.instruction_pointer = x86_64::VirtAddr::new(user_return.rip);
            frame.stack_pointer = x86_64::VirtAddr::new(user_return.rsp);
            // Keep same code segment, stack segment, and flags
        });
    }
    true
}

/// x86-64 syscall return: deliver the next caught signal. A default
/// disposition is left pending for the interrupt return path, and a fatal one
/// was already taken by `take_fatal_default_signal`.
#[cfg(target_arch = "x86_64")]
pub fn deliver_caught_signal_on_syscall_return(
    process: &mut Process,
    mut shared_table: Option<&mut ProcessPageTable>,
    user_return: &mut X86UserReturn,
    saved_regs: &mut crate::task::process_context::SavedRegisters,
) -> SignalDeliveryResult {
    loop {
        let Some(sig) = process.signals.next_deliverable_signal() else {
            return SignalDeliveryResult::NoAction;
        };
        process.signals.clear_pending(sig);
        let action = *process.signals.get_handler(sig);
        match action.handler {
            SIG_DFL => {
                process.signals.set_pending(sig);
                return SignalDeliveryResult::NoAction;
            }
            SIG_IGN => {}
            handler_addr => {
                // The wait mask selects the signal; its frame saves the original mask.
                if let Some(saved) = process.signals.sigsuspend_saved_mask.take() {
                    process.signals.set_blocked(saved);
                }
                if install_user_handler_x86_64(
                    process,
                    &mut shared_table,
                    user_return,
                    saved_regs,
                    sig,
                    handler_addr,
                    &action,
                ) {
                    return SignalDeliveryResult::Delivered;
                }
                // The syscall return path exits the thread itself, outside PM.
                return SignalDeliveryResult::FrameFault;
            }
        }
    }
}

/// Install the handler frame for `sig` and point `user_return` at the handler.
/// Returns false, with nothing changed but the stack bytes below the
/// interrupted stack pointer, when the frame cannot be installed.
#[cfg(target_arch = "x86_64")]
fn install_user_handler_x86_64(
    process: &mut Process,
    shared_table: &mut Option<&mut ProcessPageTable>,
    user_return: &mut X86UserReturn,
    saved_regs: &mut crate::task::process_context::SavedRegisters,
    sig: u32,
    handler_addr: u64,
    action: &SignalAction,
) -> bool {
    // Get current user stack pointer from the return context
    let current_rsp = user_return.rsp;
    let original_rsp = current_rsp;

    // Check if we should use the alternate signal stack
    // SA_ONSTACK flag means use alt stack if one is configured and enabled
    let use_alt_stack = (action.flags & SA_ONSTACK) != 0
        && (process.signals.alt_stack.flags & super::constants::SS_DISABLE as u32) == 0
        && process.signals.alt_stack.size > 0
        && !process.signals.alt_stack.on_stack; // Don't nest on alt stack

    let user_rsp = if use_alt_stack {
        // Use alternate stack - stack grows down, so start at top (base + size)
        let Some(alt_top) = process.signals.alt_stack.base
            .checked_add(process.signals.alt_stack.size as u64) else {
            return false;
        };
        log::debug!(
            "Using alternate signal stack: base={:#x}, size={}, top={:#x}",
            process.signals.alt_stack.base,
            process.signals.alt_stack.size,
            alt_top
        );
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
        let Some(base) = user_rsp.checked_sub(frame_size) else {
            return false;
        };
        let frame_rsp = base & !0xF; // 16-byte align
        log::debug!("Using SA_RESTORER: restorer={:#x}", action.restorer);
        (frame_rsp, action.restorer)
    } else {
        // Fall back to writing trampoline on the stack
        // This works when the stack is executable (main stack without NX)
        let trampoline_size = super::trampoline::SIGNAL_TRAMPOLINE_SIZE as u64;
        let total_size = frame_size + trampoline_size;
        let Some(base) = user_rsp.checked_sub(total_size) else {
            return false;
        };
        let frame_rsp = base & !0xF; // 16-byte align
        let trampoline_rsp = frame_rsp + frame_size;

        if use_alt_stack && frame_rsp < process.signals.alt_stack.base {
            return false;
        }
        if !write_signal_stack(
            process,
            shared_table,
            trampoline_rsp,
            &super::trampoline::SIGNAL_TRAMPOLINE,
        ) {
            return false;
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
        siginfo_ptr: 0,  // Not implemented yet
        ucontext_ptr: 0, // Not implemented yet

        // Save current execution state
        saved_rip: user_return.rip,
        saved_rsp: original_rsp,
        saved_rflags: user_return.rflags,

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

    if use_alt_stack && frame_rsp < process.signals.alt_stack.base {
        return false;
    }
    if x86_64::VirtAddr::try_new(handler_addr).is_err()
        || x86_64::VirtAddr::try_new(frame_rsp).is_err()
    {
        return false;
    }
    let bytes = unsafe {
        core::slice::from_raw_parts(
            core::ptr::addr_of!(signal_frame) as *const u8,
            SignalFrame::SIZE,
        )
    };
    if !write_signal_stack(process, shared_table, frame_rsp, bytes) {
        return false;
    }
    if use_alt_stack {
        process.signals.alt_stack.on_stack = true;
    }

    // Block signals during handler execution
    if (action.flags & SA_NODEFER) == 0 {
        // Block this signal while handler runs (prevents recursive delivery)
        process.signals.block_signals(sig_mask(sig));
    }
    // Also block any signals specified in the handler's mask
    process.signals.block_signals(action.mask);

    // The complete frame is installed before changing the return context.
    user_return.rip = handler_addr;
    user_return.rsp = frame_rsp;

    // Set up arguments for signal handler
    // void handler(int signum, siginfo_t *info, void *ucontext)
    saved_regs.rdi = sig as u64; // First argument: signal number
    saved_regs.rsi = 0; // Second argument: siginfo_t* (not implemented)
    saved_regs.rdx = 0; // Third argument: ucontext_t* (not implemented)

    if use_alt_stack {
        log::debug!(
            "Signal {} delivered to handler at {:#x} on ALTERNATE STACK, RSP={:#x}->{:#x}, return={:#x}",
            sig,
            handler_addr,
            user_rsp,
            frame_rsp,
            return_addr
        );
    } else {
        log::debug!(
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
    shared_table: &mut Option<&mut ProcessPageTable>,
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
        let Some(alt_top) = process.signals.alt_stack.base
            .checked_add(process.signals.alt_stack.size as u64) else {
            return false;
        };
        log::debug!(
            "Using alternate signal stack: base={:#x}, size={}, top={:#x}",
            process.signals.alt_stack.base,
            process.signals.alt_stack.size,
            alt_top
        );
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
        let Some(base) = user_sp.checked_sub(frame_size) else {
            return false;
        };
        let frame_sp = base & !0xF; // 16-byte align
        log::debug!("Using SA_RESTORER: restorer={:#x}", action.restorer);
        (frame_sp, action.restorer)
    } else {
        // Fall back to writing trampoline on the stack
        // This works when the stack is executable (main stack without NX)
        let trampoline_size = super::trampoline::SIGNAL_TRAMPOLINE_SIZE as u64;
        let total_size = frame_size + trampoline_size;
        let Some(base) = user_sp.checked_sub(total_size) else {
            return false;
        };
        let frame_sp = base & !0xF; // 16-byte align
        let trampoline_sp = frame_sp + frame_size;
        if use_alt_stack && frame_sp < process.signals.alt_stack.base {
            return false;
        }

        // PM is held by the delivery caller. Copy through the owned table:
        // a raw user-VA write here can fault on fork's CoW stack and deadlock
        // trying to reacquire PM before child exit can complete its wake.
        if !write_signal_stack(
            process,
            shared_table,
            trampoline_sp,
            &super::trampoline::SIGNAL_TRAMPOLINE,
        ) {
            return false;
        }

        (frame_sp, trampoline_sp)
    };

    // Build signal frame with saved context
    // Copy all X registers to the saved_x array
    let saved_x: [u64; 31] = [
        saved_regs.x0,
        saved_regs.x1,
        saved_regs.x2,
        saved_regs.x3,
        saved_regs.x4,
        saved_regs.x5,
        saved_regs.x6,
        saved_regs.x7,
        saved_regs.x8,
        saved_regs.x9,
        saved_regs.x10,
        saved_regs.x11,
        saved_regs.x12,
        saved_regs.x13,
        saved_regs.x14,
        saved_regs.x15,
        saved_regs.x16,
        saved_regs.x17,
        saved_regs.x18,
        saved_regs.x19,
        saved_regs.x20,
        saved_regs.x21,
        saved_regs.x22,
        saved_regs.x23,
        saved_regs.x24,
        saved_regs.x25,
        saved_regs.x26,
        saved_regs.x27,
        saved_regs.x28,
        saved_regs.x29,
        saved_regs.x30,
    ];

    let signal_frame = SignalFrame {
        // Return address stored in x30/lr on ARM64
        trampoline_addr: return_addr,

        // Magic number for integrity validation
        magic: SignalFrame::MAGIC,

        // Signal info
        signal: sig as u64,
        siginfo_ptr: 0,  // Not implemented yet
        ucontext_ptr: 0, // Not implemented yet

        // Save current execution state (ARM64 specific)
        saved_pc: saved_regs.elr,      // Program counter (ELR_EL1)
        saved_sp: original_sp,         // Stack pointer
        saved_pstate: saved_regs.spsr, // Processor state (SPSR_EL1)

        // Save all general-purpose registers (X0-X30)
        saved_x,

        // Save signal mask to restore after handler
        saved_blocked: process.signals.blocked,
    };

    if use_alt_stack && frame_sp < process.signals.alt_stack.base {
        return false;
    }

    // SignalFrame is a repr(C) collection of initialized u64 fields. Read its
    // bytes from the kernel buffer rather than faulting through a user VA while PM is
    // held. The table copy validates permissions and resolves CoW first.
    let bytes = unsafe {
        core::slice::from_raw_parts(
            core::ptr::addr_of!(signal_frame) as *const u8,
            SignalFrame::SIZE,
        )
    };
    if !write_signal_stack(process, shared_table, frame_sp, bytes) {
        return false;
    }

    if use_alt_stack {
        process.signals.alt_stack.on_stack = true;
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
    exception_frame.x0 = sig as u64; // First argument: signal number
    exception_frame.x1 = 0; // Second argument: siginfo_t* (not implemented)
    exception_frame.x2 = 0; // Third argument: ucontext_t* (not implemented)
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

/// The exit status a death by `sig`'s default action reports, or None when
/// that default action does not end the process.
#[cfg(target_arch = "x86_64")]
fn fatal_exit_code(sig: u32) -> Option<i32> {
    match default_action(sig) {
        SignalDefaultAction::Terminate => Some(-(sig as i32)),
        SignalDefaultAction::CoreDump => Some(-((sig as i32) | 0x80)),
        _ => None,
    }
}

/// A signal's default action ends the whole process, not one thread: kill the
/// other live threads of `pid`'s thread group with the status `pid` died with.
/// Must be called with no process-manager lock held, from process context.
pub fn terminate_thread_group_peers(pid: crate::process::ProcessId, exit_code: i32) {
    let peers = crate::process::with_process_manager(|manager| manager.thread_group_peers(pid))
        .unwrap_or_default();
    for peer in peers {
        crate::syscall::signal::kill_process_now(peer, exit_code);
    }
}

/// Take the calling process's next deliverable signal off its pending set
/// when that signal has the default action and the action ends the process,
/// returning its number. The x86-64 syscall return path calls this under the
/// process-manager lock, so it does no logging, locking or allocation.
#[cfg(target_arch = "x86_64")]
pub fn take_fatal_default_signal(process: &mut Process) -> Option<u32> {
    let sig = process.signals.next_deliverable_signal()?;
    if !process.signals.get_handler(sig).is_default() || fatal_exit_code(sig).is_none() {
        return None;
    }
    process.signals.clear_pending(sig);
    Some(sig)
}

/// Finish an x86-64 syscall return whose pending signal `sig`, taken by
/// `take_fatal_default_signal`, ends the calling process. The death runs
/// through the exit path `sys_exit` uses, which closes descriptors and tells
/// the parent outside the process-manager lock; the rest of the thread group
/// dies with the same status. The thread then waits, preemptible, for the
/// scheduler to switch away: the syscall return path cannot switch threads
/// itself (entry.asm sets PREEMPT_ACTIVE before its reschedule check), and
/// returning would run Ring 3 code after the process died. The thread is never
/// resumed.
///
/// Called with no process-manager lock held and with the syscall's single
/// preempt_disable() still in force.
#[cfg(target_arch = "x86_64")]
pub fn exit_by_signal_on_syscall_return(sig: u32) -> ! {
    let exit_code = fatal_exit_code(sig).unwrap_or(-(sig as i32));
    exit_on_syscall_return(sig, exit_code)
}

/// Finish an x86-64 syscall return whose caught signal's frame could not be
/// installed (`SignalDeliveryResult::FrameFault`). The process dies as a bad
/// user-stack access, with the status a user page fault reports, through the
/// same exit as `exit_by_signal_on_syscall_return`.
#[cfg(target_arch = "x86_64")]
pub fn exit_frame_fault_on_syscall_return() -> ! {
    exit_on_syscall_return(SIGSEGV, -(SIGSEGV as i32))
}

#[cfg(target_arch = "x86_64")]
fn exit_on_syscall_return(sig: u32, exit_code: i32) -> ! {
    if let Some(thread_id) = crate::task::scheduler::current_thread_id() {
        let row = crate::process::with_process_manager(|manager| {
            manager
                .find_process_by_thread(thread_id)
                .map(|(pid, process)| (pid, process.name.clone()))
        })
        .flatten();
        if let Some((pid, name)) = row {
            crate::serial_println!(
                "[signal] Process {} ({}) terminated by signal {} ({})",
                pid.as_u64(),
                name,
                sig,
                signal_name(sig)
            );
            terminate_thread_group_peers(pid, exit_code);
        }
        crate::task::process_task::ProcessScheduler::handle_thread_exit(thread_id, exit_code);
    }
    crate::task::scheduler::with_scheduler(|scheduler| {
        if let Some(thread) = scheduler.current_thread_mut() {
            thread.set_terminated();
        }
    });
    crate::task::scheduler::set_need_resched();
    crate::per_cpu::preempt_enable();
    loop {
        crate::arch_halt_with_interrupts();
    }
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
            parent_process
                .main_thread
                .as_ref()
                .map(|t| (t.id, parent_process.signals.has_deliverable_signals()))
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

    // Unblock parent thread if it's waiting on waitpid or sigsuspend
    if let Some((parent_tid, signal_eligible)) = parent_thread_id {
        crate::task::scheduler::with_scheduler(|sched| {
            // Wake parent if blocked in waitpid (BlockedOnChildExit)
            sched.unblock_for_child_exit(parent_tid);
            // A child exit wakes waitpid even when SIGCHLD is ignored or blocked.
            // Signal waits only wake when there is actual delivery work.
            if signal_eligible {
                sched.unblock_for_signal(parent_tid);
            }
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
    process.check_cpu_limit();
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

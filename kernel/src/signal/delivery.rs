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
use crate::memory::anon_map::PrepareWriteError;
use crate::process::process::{JobReport, Process};
use crate::process::{ProcessId, ProcessManager};

/// Check for pending, unblocked signals with an observable disposition.
/// Ignored dispositions are filtered by SignalState's cached mask.
#[inline]
pub fn has_deliverable_signals(process: &Process) -> bool {
    process.signals.has_deliverable_signals()
}

/// What a return to user mode must act on: a deliverable signal, or a stop in
/// force. A stopped process's thread does not return to user mode,
/// even when nothing is pending, and whatever woke it.
#[inline]
pub fn needs_action_on_return_to_user(process: &Process) -> bool {
    has_deliverable_signals(process) || process.job.stopped.is_some()
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
    /// A fatal default action queued its exit; the thread must not resume.
    #[cfg(target_arch = "x86_64")]
    DeferredExit,
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
/// * `shared_table` - The address-space owner when `process` is a CLONE_VM thread
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
    mut shared_table: Option<&mut Process>,
    interrupt_frame: &mut x86_64::structures::idt::InterruptStackFrame,
    saved_regs: &mut crate::task::process_context::SavedRegisters,
) -> SignalDeliveryResult {
    // A stopped process runs no handler, and a stop is taken where its thread
    // can be held off user mode (`take_stop_locked`), not here: the next
    // scheduling point does that.
    if stop_pending_or_in_force(process) {
        crate::task::scheduler::set_need_resched();
        return SignalDeliveryResult::NoAction;
    }
    // Process all deliverable signals in a loop (avoids unbounded recursion)
    loop {
        // Get next deliverable signal
        let sig = match process.signals.next_deliverable_signal() {
            Some(s) => s,
            None => return SignalDeliveryResult::NoAction,
        };

        // A stop found behind ignored signals is left the same way.
        if is_default_stop(process, sig) {
            crate::task::scheduler::set_need_resched();
            return SignalDeliveryResult::NoAction;
        }

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
                process.signals.take(sig);
                // Default action may terminate/stop the process
                match deliver_default_action(process, sig) {
                    DeliverResult::Delivered => return SignalDeliveryResult::Delivered,
                    DeliverResult::DeferredExit => return SignalDeliveryResult::DeferredExit,
                    DeliverResult::Terminated(notification) => {
                        return SignalDeliveryResult::Terminated(notification)
                    }
                    DeliverResult::Ignored => {
                        // Continue loop to check for more signals
                    }
                }
            }
            SIG_IGN => {
                process.signals.take(sig);
                log::debug!("Signal {} ignored by process {}", sig, process.id.as_u64());
                // Signal ignored - continue loop to check for more signals
            }
            handler_addr => {
                // User-defined handler - set up signal frame and return
                // Only one user handler can be delivered at a time
                return match deliver_to_user_handler_x86_64(
                    process,
                    &mut shared_table,
                    interrupt_frame,
                    saved_regs,
                    sig,
                    handler_addr,
                    &action,
                ) {
                    Ok(()) => SignalDeliveryResult::Delivered,
                    Err(PrepareWriteError::Retry) => SignalDeliveryResult::NoAction,
                    Err(PrepareWriteError::Fault) => defer_frame_fault_exit(process),
                };
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
/// * `shared_table` - The address-space owner when `process` is a CLONE_VM thread
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
    mut shared_table: Option<&mut Process>,
    exception_frame: &mut crate::arch_impl::aarch64::exception_frame::Aarch64ExceptionFrame,
    saved_regs: &mut crate::task::process_context::SavedRegisters,
) -> SignalDeliveryResult {
    // A stopped process runs no handler, and a stop is taken where its thread
    // can be held off user mode (`take_stop_locked`), not here: the next
    // scheduling point does that.
    if stop_pending_or_in_force(process) {
        crate::task::scheduler::set_need_resched();
        return SignalDeliveryResult::NoAction;
    }
    // Process all deliverable signals in a loop (avoids unbounded recursion)
    loop {
        // Get next deliverable signal
        let sig = match process.signals.next_deliverable_signal() {
            Some(s) => s,
            None => return SignalDeliveryResult::NoAction,
        };

        // A stop found behind ignored signals is left the same way.
        if is_default_stop(process, sig) {
            crate::task::scheduler::set_need_resched();
            return SignalDeliveryResult::NoAction;
        }

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
                process.signals.take(sig);
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
                process.signals.take(sig);
                log::debug!("Signal {} ignored by process {}", sig, process.id.as_u64());
                // Signal ignored - continue loop to check for more signals
            }
            handler_addr => {
                // User-defined handler - set up signal frame and return
                // Only one user handler can be delivered at a time
                return match deliver_to_user_handler_aarch64(
                    process,
                    &mut shared_table,
                    exception_frame,
                    saved_regs,
                    sig,
                    handler_addr,
                    &action,
                ) {
                    Ok(()) => SignalDeliveryResult::Delivered,
                    Err(PrepareWriteError::Retry) => SignalDeliveryResult::NoAction,
                    Err(PrepareWriteError::Fault) => defer_frame_fault_exit(process),
                };
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
    #[cfg(target_arch = "x86_64")]
    DeferredExit,
}

/// A pending SIGKILL ends a stopped process: it is not held stopped.
fn sigkill_pending(process: &Process) -> bool {
    process.signals.pending & sig_mask(SIGKILL) != 0
}

/// Whether `sig` takes its default action in `process` and that action stops
/// the process.
fn is_default_stop(process: &Process, sig: u32) -> bool {
    matches!(default_action(sig), SignalDefaultAction::Stop)
        && process.signals.get_handler(sig).is_default()
}

/// Whether a thread of `process` has a stop to act on before it returns to
/// user mode: the process is stopped, or its next deliverable signal stops
/// it. A pending SIGKILL overrides both.
pub fn stop_pending_or_in_force(process: &Process) -> bool {
    !sigkill_pending(process)
        && (process.job.stopped.is_some()
            || process
                .signals
                .next_deliverable_signal()
                .is_some_and(|sig| is_default_stop(process, sig)))
}

/// Raise fault signal `sig` for the thread of `process` whose instruction
/// faulted, with its siginfo, as Linux's force_sig_fault: a blocked or
/// ignored fault signal is unblocked and its action reset to SIG_DFL, since
/// the faulting instruction cannot go on. Returns whether a handler will run
/// for it; otherwise its default action ends the process. PM held.
pub fn raise_fault_signal(process: &mut Process, sig: u32, info: SigInfo) -> bool {
    process.signals.force_signal(sig, info);
    process.signals.get_handler(sig).is_handler()
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
        crate::task::scheduler::terminate_thread(thread_id);
    }
    SignalDeliveryResult::FrameFault
}

/// Copy `bytes` onto the user stack through the table of the address space the
/// thread runs in: its own, or for a CLONE_VM thread the owner's. A frame
/// below the main stack's bottom grows the stack, as a user access there
/// would.
fn write_signal_stack(
    process: &mut Process,
    shared_table: &mut Option<&mut Process>,
    addr: u64,
    bytes: &[u8],
) -> Result<(), PrepareWriteError> {
    let pid = process.id.as_u64();
    let owner = if process.page_table.is_some() {
        process
    } else {
        shared_table
            .as_deref_mut()
            .ok_or(PrepareWriteError::Fault)?
    };
    crate::memory::anon_map::prepare_write(owner, addr, bytes.len())?;
    let table = owner
        .page_table
        .as_deref_mut()
        .ok_or(PrepareWriteError::Fault)?;
    if table.write_user_memory(addr, bytes, pid) {
        Ok(())
    } else {
        Err(PrepareWriteError::Fault)
    }
}

/// Deliver a signal's default action
/// Returns DeliverResult indicating what action was taken
fn deliver_default_action(process: &mut Process, sig: u32) -> DeliverResult {
    #[cfg(target_arch = "x86_64")]
    if fatal_exit_code(sig).is_some() {
        let exit_code = signal_death_exit_code(process, sig);
        if let Some(thread_id) = process.main_thread.as_ref().map(|thread| thread.id) {
            // Interrupt return holds PM: do not close descriptors, walk CoW
            // mappings or print before the parent can observe this death.
            // The normal exit worker publishes status and defers reclamation.
            if crate::task::process_task::defer_fault_exit(thread_id, exit_code) {
                crate::task::scheduler::terminate_thread(thread_id);
                return DeliverResult::DeferredExit;
            }
            // An exhausted queue retains the existing synchronous exit below;
            // never lose a death merely because deferred storage is unavailable.
        }
    }
    match default_action(sig) {
        SignalDefaultAction::Terminate => {
            // Exit code for signal termination is typically 128 + signal number
            // But we use negative signal number to indicate signal death
            crate::trace_count!(crate::tracing::providers::teardown::TEARDOWN_ENTRY_SIGNAL);
            let exit_code = signal_death_exit_code(process, sig);
            process.terminate(exit_code);

            // CRITICAL: Also mark the scheduler's copy of the thread as terminated.
            // The process.terminate() call above marks process.main_thread, but
            // the scheduler has its own copy of threads in its threads vector.
            // Without this, the scheduler would keep scheduling the terminated thread!
            if let Some(ref thread) = process.main_thread {
                let thread_id = thread.id();
                crate::task::scheduler::terminate_thread(thread_id);
            }

            // Return notification info for parent - caller will notify after releasing lock
            if let Some(notification) = notify_parent_of_termination(process) {
                DeliverResult::Terminated(notification)
            } else {
                DeliverResult::Delivered
            }
        }
        SignalDefaultAction::CoreDump => {
            // Core dump not implemented, just terminate
            // The 0x80 flag indicates core dump
            crate::trace_count!(crate::tracing::providers::teardown::TEARDOWN_ENTRY_SIGNAL);
            let exit_code = signal_death_exit_code(process, sig);
            process.terminate(exit_code);

            // CRITICAL: Also mark the scheduler's copy of the thread as terminated.
            if let Some(ref thread) = process.main_thread {
                let thread_id = thread.id();
                crate::task::scheduler::terminate_thread(thread_id);
            }

            // Return notification info for parent - caller will notify after releasing lock
            if let Some(notification) = notify_parent_of_termination(process) {
                DeliverResult::Terminated(notification)
            } else {
                DeliverResult::Delivered
            }
        }
        // Unreached: delivery leaves a stop pending for `take_stop_locked`.
        SignalDefaultAction::Stop => DeliverResult::Ignored,
        // SIGCONT continued the process when it was generated; its default
        // action leaves nothing to do here.
        SignalDefaultAction::Continue => DeliverResult::Ignored,
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
    shared_table: &mut Option<&mut Process>,
    interrupt_frame: &mut x86_64::structures::idt::InterruptStackFrame,
    saved_regs: &mut crate::task::process_context::SavedRegisters,
    sig: u32,
    handler_addr: u64,
    action: &SignalAction,
) -> Result<(), PrepareWriteError> {
    let mut user_return = X86UserReturn {
        rip: interrupt_frame.instruction_pointer.as_u64(),
        rsp: interrupt_frame.stack_pointer.as_u64(),
        rflags: interrupt_frame.cpu_flags.bits(),
    };
    install_user_handler_x86_64(
        process,
        shared_table,
        &mut user_return,
        saved_regs,
        sig,
        handler_addr,
        action,
    )?;
    // The installer accepted both addresses as canonical.
    unsafe {
        interrupt_frame.as_mut().update(|frame| {
            frame.instruction_pointer = x86_64::VirtAddr::new(user_return.rip);
            frame.stack_pointer = x86_64::VirtAddr::new(user_return.rsp);
            frame.cpu_flags =
                x86_64::registers::rflags::RFlags::from_bits_truncate(user_return.rflags);
            // Keep same code segment and stack segment
        });
    }
    Ok(())
}

/// x86-64 syscall return: deliver the next caught signal. A default
/// disposition is left pending for the interrupt return path; a fatal one was
/// already taken by `take_fatal_default_signal`, and the caller has already
/// held the thread for a stop (`stop_pending_or_in_force`).
#[cfg(target_arch = "x86_64")]
pub fn deliver_caught_signal_on_syscall_return(
    process: &mut Process,
    mut shared_table: Option<&mut Process>,
    user_return: &mut X86UserReturn,
    saved_regs: &mut crate::task::process_context::SavedRegisters,
) -> SignalDeliveryResult {
    loop {
        let Some(sig) = process.signals.next_deliverable_signal() else {
            return SignalDeliveryResult::NoAction;
        };
        let action = *process.signals.get_handler(sig);
        match action.handler {
            SIG_DFL => {
                // A stop found behind ignored signals stays pending, with the
                // sigsuspend mask, and the next scheduling point holds the
                // thread; the caller has already held it for any other stop.
                if matches!(default_action(sig), SignalDefaultAction::Stop) {
                    crate::task::scheduler::set_need_resched();
                }
                return SignalDeliveryResult::NoAction;
            }
            SIG_IGN => {
                process.signals.take(sig);
            }
            handler_addr => {
                return match install_user_handler_x86_64(
                    process,
                    &mut shared_table,
                    user_return,
                    saved_regs,
                    sig,
                    handler_addr,
                    &action,
                ) {
                    Ok(()) => SignalDeliveryResult::Delivered,
                    Err(PrepareWriteError::Retry) => SignalDeliveryResult::NoAction,
                    Err(PrepareWriteError::Fault) => SignalDeliveryResult::FrameFault,
                };
            }
        }
    }
}

/// What delivering a caught `sig` does to the dispositions and mask once its
/// frame is installed: SA_RESETHAND resets the action to SIG_DFL on entry to
/// the handler, and the handler runs with the signal (unless SA_NODEFER) and
/// its sa_mask blocked.
///
/// Dispositions are per thread-group row here. The reset is made in the row
/// whose thread takes the signal and copied to the group's other rows before
/// the process manager is released (`mark_group_reset`).
fn enter_handler(process: &mut Process, sig: u32, action: &SignalAction) {
    process.signals.take(sig);
    process.signals.thread.take_wait_mask();
    if action.flags & SA_RESETHAND != 0 {
        process.signals.set_handler(sig, SignalAction::default());
        process.signals.mark_group_reset(sig);
    }
    if (action.flags & SA_NODEFER) == 0 {
        // Block this signal while handler runs (prevents recursive delivery)
        process.signals.block_signals(sig_mask(sig));
    }
    // Also block any signals specified in the handler's mask
    process.signals.block_signals(action.mask);
}

/// The si_addr a fault signal reports, for the sigcontext's fault address.
fn fault_address(sig: u32, info: &SigInfo) -> u64 {
    if sig_mask(sig) & SYNCHRONOUS_SIGNALS != 0 && info.code > 0 {
        info.fields[0]
    } else {
        0
    }
}

/// Install the handler frame for `sig` and point `user_return` at the handler.
/// Returns an error, with nothing changed but the stack bytes below the
/// interrupted stack pointer, when the frame cannot be installed.
///
/// The frame is Linux's: the handler is called as
/// `handler(sig, &frame.info, &frame.uc)` with RSP pointing at the return
/// address, so RSP + 8 is 16-byte aligned. The FXSAVE image of the
/// interrupted x87/SSE state lies above the frame, and the handler starts
/// with the initial x87/SSE state, as on Linux. On the thread's own stack the
/// 128-byte red zone below the interrupted RSP is left alone.
#[cfg(target_arch = "x86_64")]
fn install_user_handler_x86_64(
    process: &mut Process,
    shared_table: &mut Option<&mut Process>,
    user_return: &mut X86UserReturn,
    saved_regs: &mut crate::task::process_context::SavedRegisters,
    sig: u32,
    handler_addr: u64,
    action: &SignalAction,
) -> Result<(), PrepareWriteError> {
    use crate::arch_impl::x86_64::fpu;

    const RED_ZONE: u64 = 128;
    let original_rsp = user_return.rsp;

    // Check if we should use the alternate signal stack
    // SA_ONSTACK flag means use alt stack if one is configured and enabled
    // A handler interrupting code already on the alternate stack nests below
    // it there, as one without SA_ONSTACK does on any stack.
    let use_alt_stack = (action.flags & SA_ONSTACK) != 0
        && (process.signals.alt_stack.flags & super::constants::SS_DISABLE as u32) == 0
        && process.signals.alt_stack.size > 0
        && !process.signals.alt_stack.on_stack(original_rsp);

    let top = if use_alt_stack {
        // Use alternate stack - stack grows down, so start at top (base + size)
        match process
            .signals
            .alt_stack
            .base
            .checked_add(process.signals.alt_stack.size as u64)
        {
            Some(alt_top) => alt_top,
            None => return Err(PrepareWriteError::Fault),
        }
    } else {
        match original_rsp.checked_sub(RED_ZONE) {
            Some(top) => top,
            None => return Err(PrepareWriteError::Fault),
        }
    };

    // The FXSAVE image, 64-byte aligned, at the top.
    let Some(fp_addr) = top.checked_sub(core::mem::size_of::<fpu::FpuState>() as u64) else {
        return Err(PrepareWriteError::Fault);
    };
    let fp_addr = fp_addr & !63;

    // Check if the handler provides a restorer function (SA_RESTORER flag).
    // Without one the trampoline is written to the stack, which works when
    // the stack is executable.
    let use_restorer = (action.flags & super::constants::SA_RESTORER) != 0 && action.restorer != 0;
    let (below, trampoline_addr) = if use_restorer {
        (fp_addr, None)
    } else {
        let size = super::trampoline::SIGNAL_TRAMPOLINE_SIZE as u64;
        let Some(addr) = fp_addr.checked_sub(size) else {
            return Err(PrepareWriteError::Fault);
        };
        let addr = addr & !0xF;
        (addr, Some(addr))
    };
    let Some(frame_rsp) = below
        .checked_sub(SignalFrame::SIZE as u64)
        .map(|base| base & !0xF)
        .and_then(|base| base.checked_sub(8))
    else {
        return Err(PrepareWriteError::Fault);
    };
    // A frame on the alternate stack, first or nested, must fit on it.
    let alt = &process.signals.alt_stack;
    if (use_alt_stack || alt.on_stack(original_rsp)) && !alt.on_stack(frame_rsp) {
        return Err(PrepareWriteError::Fault);
    }
    if x86_64::VirtAddr::try_new(handler_addr).is_err()
        || x86_64::VirtAddr::try_new(frame_rsp).is_err()
    {
        return Err(PrepareWriteError::Fault);
    }
    let return_addr = trampoline_addr.unwrap_or(action.restorer);

    let thread_id = process.main_thread.as_ref().map(|thread| thread.id);
    let fp_state = thread_id.map_or_else(fpu::FpuState::initial, fpu::user_state);
    let info = process.signals.next_info(sig);
    let blocked = process
        .signals
        .thread
        .wait_mask()
        .unwrap_or_else(|| process.signals.blocked());

    let signal_frame = SignalFrame {
        // When the handler does 'ret', it pops this and jumps there.
        pretcode: return_addr,
        uc: UContext {
            uc_flags: 0,
            uc_link: 0,
            uc_stack: process.signals.alt_stack.stack_t(original_rsp),
            uc_mcontext: SigContext {
                r8: saved_regs.r8,
                r9: saved_regs.r9,
                r10: saved_regs.r10,
                r11: saved_regs.r11,
                r12: saved_regs.r12,
                r13: saved_regs.r13,
                r14: saved_regs.r14,
                r15: saved_regs.r15,
                rdi: saved_regs.rdi,
                rsi: saved_regs.rsi,
                rbp: saved_regs.rbp,
                rbx: saved_regs.rbx,
                rdx: saved_regs.rdx,
                rax: saved_regs.rax,
                rcx: saved_regs.rcx,
                rsp: original_rsp,
                rip: user_return.rip,
                eflags: user_return.rflags,
                cs: crate::gdt::user_code_selector().0,
                ss: crate::gdt::user_data_selector().0,
                err: info.trap.error,
                trapno: info.trap.number,
                oldmask: blocked,
                cr2: fault_address(sig, &info),
                fpstate: fp_addr,
                ..SigContext::default()
            },
            uc_sigmask: blocked,
        },
        info: info.to_linux(sig),
    };

    if let Some(addr) = trampoline_addr {
        write_signal_stack(
            process,
            shared_table,
            addr,
            &super::trampoline::SIGNAL_TRAMPOLINE,
        )?;
    }
    write_signal_stack(process, shared_table, fp_addr, fp_state.as_bytes())?;
    // SAFETY: SignalFrame is repr(C) plain data with no padding (types.rs
    // checks each size against its fields), and every field is initialized.
    let bytes = unsafe {
        core::slice::from_raw_parts(
            core::ptr::addr_of!(signal_frame) as *const u8,
            SignalFrame::SIZE,
        )
    };
    write_signal_stack(process, shared_table, frame_rsp, bytes)?;
    enter_handler(process, sig, action);
    if let Some(thread_id) = thread_id {
        fpu::set_user_state(thread_id, &fpu::FpuState::initial());
    }

    // The complete frame is installed before changing the return context.
    user_return.rip = handler_addr;
    user_return.rsp = frame_rsp;
    // The handler starts with DF clear, as the ABI requires at a call, and
    // without single-stepping.
    const TF: u64 = 1 << 8;
    const DF: u64 = 1 << 10;
    const RF: u64 = 1 << 16;
    user_return.rflags &= !(TF | DF | RF);

    // void handler(int signum, siginfo_t *info, void *ucontext)
    saved_regs.rdi = sig as u64;
    saved_regs.rsi = frame_rsp + core::mem::offset_of!(SignalFrame, info) as u64;
    saved_regs.rdx = frame_rsp + core::mem::offset_of!(SignalFrame, uc) as u64;
    // For a handler declared without a prototype, as Linux does.
    saved_regs.rax = 0;

    log::debug!(
        "Signal {} delivered to handler at {:#x}{}, RSP={:#x}->{:#x}, return={:#x}",
        sig,
        handler_addr,
        if use_alt_stack { " on ALTERNATE STACK" } else { "" },
        original_rsp,
        frame_rsp,
        return_addr
    );

    Ok(())
}

// =============================================================================
// ARM64 User Handler Delivery
// =============================================================================

/// Set up user stack and registers to call a user-defined signal handler (ARM64)
///
/// This modifies the exception frame so that when we return to userspace,
/// we jump to the signal handler instead of the interrupted code.
///
/// The frame is Linux's: the handler is called as
/// `handler(sig, &frame.info, &frame.uc)` with SP at the frame, X30 at the
/// restorer (or the trampoline written to the stack) and X29 at a frame
/// record holding the interrupted X29 and X30. The context saves the
/// FP/SIMD registers, v0-v31 with FPSR and FPCR, which the handler starts
/// with, as on Linux.
#[cfg(target_arch = "aarch64")]
fn deliver_to_user_handler_aarch64(
    process: &mut Process,
    shared_table: &mut Option<&mut Process>,
    exception_frame: &mut crate::arch_impl::aarch64::exception_frame::Aarch64ExceptionFrame,
    saved_regs: &mut crate::task::process_context::SavedRegisters,
    sig: u32,
    handler_addr: u64,
    action: &SignalAction,
) -> Result<(), PrepareWriteError> {
    // On ARM64, user SP is in SP_EL0, which we save in saved_regs.sp
    let original_sp = saved_regs.sp;

    // Check if we should use the alternate signal stack
    // SA_ONSTACK flag means use alt stack if one is configured and enabled
    // A handler interrupting code already on the alternate stack nests below
    // it there, as one without SA_ONSTACK does on any stack.
    let use_alt_stack = (action.flags & SA_ONSTACK) != 0
        && (process.signals.alt_stack.flags & super::constants::SS_DISABLE as u32) == 0
        && process.signals.alt_stack.size > 0
        && !process.signals.alt_stack.on_stack(original_sp);

    let top = if use_alt_stack {
        // Use alternate stack - stack grows down, so start at top (base + size)
        match process
            .signals
            .alt_stack
            .base
            .checked_add(process.signals.alt_stack.size as u64)
        {
            Some(alt_top) => alt_top,
            None => return Err(PrepareWriteError::Fault),
        }
    } else {
        original_sp
    };

    // Check if the handler provides a restorer function (SA_RESTORER flag).
    // Without one the trampoline is written to the stack, which works when
    // the stack is executable.
    let use_restorer = (action.flags & super::constants::SA_RESTORER) != 0 && action.restorer != 0;
    let (below, trampoline_addr) = if use_restorer {
        (top, None)
    } else {
        let size = super::trampoline::SIGNAL_TRAMPOLINE_SIZE as u64;
        let Some(addr) = top.checked_sub(size) else {
            return Err(PrepareWriteError::Fault);
        };
        let addr = addr & !0xF;
        (addr, Some(addr))
    };
    let Some(frame_sp) = below
        .checked_sub(SignalFrame::SIZE as u64)
        .map(|base| base & !0xF)
    else {
        return Err(PrepareWriteError::Fault);
    };
    // A frame on the alternate stack, first or nested, must fit on it.
    let alt = &process.signals.alt_stack;
    if (use_alt_stack || alt.on_stack(original_sp)) && !alt.on_stack(frame_sp) {
        return Err(PrepareWriteError::Fault);
    }
    let return_addr = trampoline_addr.unwrap_or(action.restorer);

    let info = process.signals.next_info(sig);
    let blocked = process
        .signals
        .thread
        .wait_mask()
        .unwrap_or_else(|| process.signals.blocked());
    let regs = [
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

    let mut signal_frame = SignalFrame {
        info: info.to_linux(sig),
        uc: UContext {
            uc_flags: 0,
            uc_link: 0,
            uc_stack: process.signals.alt_stack.stack_t(original_sp),
            uc_sigmask: blocked,
            unused: [0; 120],
            _pad: 0,
            uc_mcontext: SigContext {
                fault_address: fault_address(sig, &info),
                regs,
                sp: original_sp,
                pc: saved_regs.elr,
                pstate: saved_regs.spsr,
                _pad: 0,
                reserved: SigContextReserved([0; 4096]),
            },
        },
        frame_record: [saved_regs.x29, saved_regs.x30],
    };
    // The FP/SIMD record, for a fault the ESR record, then the empty record
    // (the zeroed bytes after them) that ends the list.
    let mut fpsimd = FpsimdContext {
        magic: FpsimdContext::MAGIC,
        size: FpsimdContext::SIZE,
        fpsr: 0,
        fpcr: 0,
        vregs: [0; 32],
    };
    crate::arch_impl::aarch64::fpsimd::save(&mut fpsimd);
    let reserved = signal_frame.uc.uc_mcontext.reserved.0.as_mut_ptr();
    // SAFETY: the reserved area is 16-byte aligned and larger than both
    // records, which are repr(C) plain data with no padding, and the second
    // starts at a multiple of 16 bytes.
    unsafe {
        core::ptr::write(reserved as *mut FpsimdContext, fpsimd);
        if info.trap.error != 0 {
            core::ptr::write(
                reserved.add(FpsimdContext::SIZE as usize) as *mut EsrContext,
                EsrContext {
                    magic: EsrContext::MAGIC,
                    size: EsrContext::SIZE,
                    esr: info.trap.error,
                },
            );
        }
    }

    if let Some(addr) = trampoline_addr {
        // PM is held by the delivery caller. Copy through the owned table:
        // a raw user-VA write here can fault on fork's CoW stack and deadlock
        // trying to reacquire PM before child exit can complete its wake.
        write_signal_stack(
            process,
            shared_table,
            addr,
            &super::trampoline::SIGNAL_TRAMPOLINE,
        )?;
    }
    // SignalFrame is repr(C) plain data with no padding (types.rs checks each
    // size against its fields), and every field is initialized. Read its
    // bytes from the kernel buffer rather than faulting through a user VA
    // while PM is held. The table copy validates permissions and resolves
    // CoW first.
    let bytes = unsafe {
        core::slice::from_raw_parts(
            core::ptr::addr_of!(signal_frame) as *const u8,
            SignalFrame::SIZE,
        )
    };
    write_signal_stack(process, shared_table, frame_sp, bytes)?;

    enter_handler(process, sig, action);

    let info_addr = frame_sp + core::mem::offset_of!(SignalFrame, info) as u64;
    let uc_addr = frame_sp + core::mem::offset_of!(SignalFrame, uc) as u64;
    let record_addr = frame_sp + core::mem::offset_of!(SignalFrame, frame_record) as u64;

    // Modify exception frame to jump to signal handler
    exception_frame.elr = handler_addr;
    saved_regs.elr = handler_addr;

    // When the handler returns (via RET instruction), it jumps to x30
    exception_frame.x30 = return_addr;
    saved_regs.x30 = return_addr;
    exception_frame.x29 = record_addr;
    saved_regs.x29 = record_addr;

    // The actual SP_EL0 update happens on exception return
    saved_regs.sp = frame_sp;

    // void handler(int signum, siginfo_t *info, void *ucontext)
    exception_frame.x0 = sig as u64;
    exception_frame.x1 = info_addr;
    exception_frame.x2 = uc_addr;
    saved_regs.x0 = sig as u64;
    saved_regs.x1 = info_addr;
    saved_regs.x2 = uc_addr;

    log::info!(
        "Signal {} delivered to handler at {:#x}{}, SP={:#x}->{:#x}, return={:#x}",
        sig,
        handler_addr,
        if use_alt_stack { " on ALTERNATE STACK" } else { "" },
        original_sp,
        frame_sp,
        return_addr
    );

    Ok(())
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

/// A child's stop or continue that its parent has to be told about: SIGCHLD,
/// unless the parent's SIGCHLD action has SA_NOCLDSTOP, and a wake for a
/// parent waiting in waitpid or waitid.
#[derive(Debug, Clone, Copy)]
pub struct JobNotification {
    pub parent_pid: ProcessId,
    pub child_pid: ProcessId,
}

/// Tell the parent of a stop or continue, with the process-manager lock held
/// by the caller. The wake takes the scheduler lock, in the PM-then-scheduler
/// order signal delivery uses, and writes no serial output: a stop can be
/// reported from an interrupt return (`hold_stopped_thread_on_interrupt_return`),
/// with the try-locked guard that path already holds.
pub fn notify_parent_of_job_change_locked(
    manager: &mut ProcessManager,
    notification: &JobNotification,
) {
    let info = manager.get_process(notification.child_pid).and_then(|child| {
        let (code, status) = match child.job.report? {
            JobReport::Stopped(sig) => (CLD_STOPPED, sig as i32),
            JobReport::Continued => (CLD_CONTINUED, SIGCONT as i32),
        };
        Some(SigInfo::child(code, child.id.as_u64() as u32, child.cred.uid, status))
    });
    let notify = manager.get_process(notification.parent_pid)
        .is_some_and(|p| p.signals.get_handler(SIGCHLD).flags & SA_NOCLDSTOP == 0);
    let signal_wake = if notify {
        manager.queue_process_signal(notification.parent_pid, SIGCHLD, info.unwrap_or_else(SigInfo::kernel))
    } else { None };
    let Some(parent) = manager.get_process_mut(notification.parent_pid) else { return; };
    let signal_eligible = parent.signals.has_deliverable_signals();
    let Some(parent_tid) = parent.main_thread.as_ref().map(|thread| thread.id) else {
        return;
    };
    crate::task::scheduler::with_scheduler(|scheduler| {
        scheduler.unblock_for_job_change(parent_tid, signal_eligible);
        if let Some(tid) = signal_wake {
            scheduler.unblock_for_signal(tid);
            scheduler.unblock_for_child_exit(tid);
        }
    });
}

/// Stop `pid`'s process for stop signal `sig`, with PM held: every row of its
/// thread group is stopped, a pending SIGCONT is discarded, and a thread
/// waiting on no CPU to resume in user mode is parked at once. A thread
/// running on another CPU is sent there to a scheduling point, where it is
/// held; one waiting in a syscall is held at that syscall's return, and its
/// wait goes on meanwhile. The parent is told once no thread can still be
/// running in user mode. `caller_tid` is the thread taking the stop, which is
/// in the kernel and is held before it returns to user mode.
fn stop_thread_group(
    manager: &mut ProcessManager,
    pid: ProcessId,
    sig: u32,
    caller_tid: Option<u64>,
) {
    let Some(group) = manager.thread_group_of(pid) else {
        return;
    };
    crate::task::scheduler::with_scheduler(|scheduler| {
        for row in manager.group_rows_mut(group) {
            row.job.stopped = Some(sig);
            row.signals.thread.return_work.store(true, core::sync::atomic::Ordering::Release);
            row.signals.clear_pending(SIGCONT);
            let Some(thread_id) = row.main_thread.as_ref().map(|thread| thread.id) else {
                continue;
            };
            if scheduler.block_ready_user_thread(thread_id) {
                row.job.parked = true;
            } else if Some(thread_id) != caller_tid {
                scheduler.kick_thread(thread_id);
            }
        }
    });
    if let Some(leader) = manager.get_process_mut(ProcessId::new(group)) {
        // The stop replaces a continue the parent has not waited for.
        leader.job.report = None;
        leader.job.report_owed = true;
    }
    complete_stop_report(manager, group, caller_tid);
}

/// Report thread group `group`'s stop to its parent once no thread of it can
/// still be running in user mode: each is blocked, terminated, or
/// `caller_tid`, which is in the kernel and is held before it returns. Called
/// where a stop is taken and wherever one of the group's threads is held.
fn complete_stop_report(manager: &mut ProcessManager, group: u64, caller_tid: Option<u64>) {
    let leader = ProcessId::new(group);
    if !manager
        .get_process(leader)
        .is_some_and(|row| row.job.report_owed)
    {
        return;
    }
    let quiescent = crate::task::scheduler::with_scheduler(|scheduler| {
        manager.group_rows(group).all(|row| {
            row.main_thread.as_ref().is_none_or(|thread| {
                Some(thread.id) == caller_tid || !scheduler.thread_may_run(thread.id)
            })
        })
    })
    .unwrap_or(false);
    if !quiescent {
        return;
    }
    let Some(row) = manager.get_process_mut(leader) else {
        return;
    };
    let Some(sig) = row.job.stopped else {
        return;
    };
    row.job.report_owed = false;
    row.job.report = Some(JobReport::Stopped(sig));
    if let Some(parent_pid) = row.parent {
        notify_parent_of_job_change_locked(
            manager,
            &JobNotification {
                parent_pid,
                child_pid: leader,
            },
        );
    }
}

/// Generate stop signal `sig` for `pid`, with PM held. When its action is the
/// default and `pid` does not block it, the stop is taken now for the whole
/// process, or discarded when `sig` is not SIGSTOP and the process group is
/// orphaned (POSIX); either way the signal is consumed and this returns true.
/// Otherwise it returns false and the caller queues the signal: a caught stop
/// signal runs its handler, and a blocked one is taken where it is unblocked
/// (`take_stop_locked`). `caller_tid` is the generating thread, in the kernel,
/// or None from an interrupt handler.
pub fn generate_stop_locked(
    manager: &mut ProcessManager,
    pid: ProcessId,
    sig: u32,
    caller_tid: Option<u64>,
) -> bool {
    let Some(group) = manager.thread_group_of(pid) else {
        return true;
    };
    // A stop signal discards a pending SIGCONT.
    for row in manager.group_rows_mut(group) {
        row.signals.clear_pending(SIGCONT);
    }
    let Some(process) = manager.get_process(pid) else {
        return true;
    };
    if !is_default_stop(process, sig) || process.signals.is_blocked(sig) {
        return false;
    }
    // Already stopped: there is nothing more to take or report.
    if process.job.stopped.is_some() {
        return true;
    }
    if sig != SIGSTOP && manager.pgrp_is_orphaned(process.pgid, None) {
        return true;
    }
    stop_thread_group(manager, pid, sig, caller_tid);
    true
}

/// SIGCONT generated for `pid`, with PM held: every pending stop signal of
/// its process is discarded, and a stopped process is continued, its threads
/// the stop parked are made ready, and its parent is told. SIGCONT does this
/// even when it is blocked or ignored; the caller then queues it as usual.
pub fn continue_thread_group_locked(manager: &mut ProcessManager, pid: ProcessId) {
    let Some(group) = manager.thread_group_of(pid) else {
        return;
    };
    let mut was_stopped = false;
    crate::task::scheduler::with_scheduler(|scheduler| {
        for row in manager.group_rows_mut(group) {
            row.signals.discard_pending(STOP_SIGNALS);
            was_stopped |= row.job.stopped.take().is_some();
            if core::mem::take(&mut row.job.parked) {
                if let Some(thread) = row.main_thread.as_ref() {
                    let _ = scheduler.unblock(thread.id);
                }
            }
        }
    });
    if !was_stopped {
        return;
    }
    crate::task::scheduler::set_need_resched();
    let leader = ProcessId::new(group);
    let Some(row) = manager.get_process_mut(leader) else {
        return;
    };
    // A stop the parent was never told of is not reported as continued.
    if core::mem::take(&mut row.job.report_owed) {
        return;
    }
    row.job.report = Some(JobReport::Continued);
    if let Some(parent_pid) = row.parent {
        notify_parent_of_job_change_locked(
            manager,
            &JobNotification {
                parent_pid,
                child_pid: leader,
            },
        );
    }
}

/// At `thread_id`'s return to user mode, with PM held: take a pending stop
/// signal whose action is the default, and say whether the thread is to be
/// held because its process is stopped. A pending SIGKILL is never held. A
/// SIGTSTP, SIGTTIN or SIGTTOU that would stop a process in an orphaned
/// process group is discarded, judged here where the stop is taken, as
/// Linux's get_signal does, rather than where it was queued.
pub fn take_stop_locked(manager: &mut ProcessManager, thread_id: u64) -> bool {
    let Some((pid, _)) = manager.find_process_by_thread(thread_id) else {
        return false;
    };
    loop {
        let Some(process) = manager.get_process_mut(pid) else {
            return false;
        };
        if sigkill_pending(process) {
            return false;
        }
        if process.job.stopped.is_some() {
            return true;
        }
        let Some(sig) = process.signals.next_deliverable_signal() else {
            return false;
        };
        if !is_default_stop(process, sig) {
            return false;
        }
        process.signals.clear_pending(sig);
        let pgid = process.pgid;
        if sig != SIGSTOP && manager.pgrp_is_orphaned(pgid, None) {
            continue;
        }
        stop_thread_group(manager, pid, sig, Some(thread_id));
    }
}

/// Hold the current thread, `thread_id`, on an interrupt's return to user
/// mode while its process is stopped, taking a pending stop first. Called
/// before the switch decision with no lock held. Returns true when the thread
/// has been blocked: the caller then switches away, saving its user context
/// for SIGCONT to resume. Interrupt-path rules: the process manager is only
/// try-locked, on x86_64 polled for a bounded time (false when it stays busy,
/// and the next scheduling point retries),
/// the scheduler lock is the interrupt-safe one the switch takes anyway, and
/// nothing here writes serial output.
pub fn hold_stopped_thread_on_interrupt_return(thread_id: u64) -> bool {
    hold_stopped_thread_on_interrupt_return_or_busy(thread_id).unwrap_or(false)
}

/// `hold_stopped_thread_on_interrupt_return`, telling a busy process manager
/// apart: None when it was held elsewhere and nothing was decided, so the
/// caller can retry before the thread runs a user instruction rather than at
/// the next tick.
pub fn hold_stopped_thread_on_interrupt_return_or_busy(thread_id: u64) -> Option<bool> {
    #[cfg(target_arch = "x86_64")]
    let mut guard = crate::process::poll_manager()?;
    #[cfg(not(target_arch = "x86_64"))]
    let mut guard = crate::process::try_manager()?;
    let Some(manager) = guard.as_mut() else {
        return Some(false);
    };
    if !take_stop_locked(manager, thread_id) {
        return Some(false);
    }
    let blocked = crate::task::scheduler::with_scheduler(|scheduler| {
        let current = scheduler.current_thread_mut().map(|thread| thread.id);
        if current == Some(thread_id) {
            scheduler.block_current();
        }
        current == Some(thread_id)
    })
    .unwrap_or(false);
    if !blocked {
        return Some(false);
    }
    let Some(group) = manager
        .find_process_by_thread_mut(thread_id)
        .map(|(pid, row)| {
            row.job.parked = true;
            pid
        })
        .and_then(|pid| manager.thread_group_of(pid))
    else {
        return Some(true);
    };
    complete_stop_report(manager, group, None);
    Some(true)
}

/// Hold the calling thread at a syscall's return to user mode while its
/// process is stopped, taking a pending stop first. It waits as a blocked
/// syscall does, parked for SIGCONT, and returns once the process is no
/// longer stopped or a SIGKILL is pending; the caller then delivers what is
/// pending. A thread the kill path terminated, or whose row has been reaped,
/// never returns to user mode: it waits for the scheduler to switch away.
/// Called with no process-manager lock held and with the syscall's
/// preempt_disable() in force, which it is again on return.
pub fn hold_stopped_thread_on_syscall_return() {
    let Some(thread_id) = crate::task::scheduler::current_thread_id() else {
        return;
    };
    loop {
        // Block first, then look: a SIGCONT after the look finds the thread
        // parked and wakes it.
        let terminated = crate::task::scheduler::with_scheduler(|scheduler| {
            match scheduler.current_thread_mut() {
                Some(thread) if thread.state != crate::task::thread::ThreadState::Terminated => {
                    scheduler.block_current_in_syscall();
                    false
                }
                _ => true,
            }
        })
        .unwrap_or(true);
        if terminated {
            abandon_syscall_return();
        }
        let held = crate::process::with_process_manager(|manager| {
            let held = take_stop_locked(manager, thread_id);
            let (pid, row) = manager.find_process_by_thread_mut(thread_id)?;
            row.job.parked = held;
            if held {
                if let Some(group) = manager.thread_group_of(pid) {
                    complete_stop_report(manager, group, None);
                }
            }
            Some(held)
        })
        .flatten();
        match held {
            Some(true) => {
                crate::per_cpu::preempt_enable();
                crate::task::scheduler::yield_current();
                crate::arch_halt_with_interrupts();
                crate::per_cpu::preempt_disable();
            }
            Some(false) => break,
            // The row is gone: the process has exited under the thread.
            None => abandon_syscall_return(),
        }
    }
    let terminated =
        crate::task::scheduler::with_scheduler(|scheduler| match scheduler.current_thread_mut() {
            Some(thread) if thread.state != crate::task::thread::ThreadState::Terminated => {
                thread.blocked_in_syscall = false;
                thread.set_ready();
                false
            }
            _ => true,
        })
        .unwrap_or(true);
    if terminated {
        abandon_syscall_return();
    }
}

/// End a syscall return whose thread must never run user code again: it was
/// terminated, or its process's row is gone. The thread is marked terminated
/// and waits, preemptible, for the scheduler to switch away for good.
fn abandon_syscall_return() -> ! {
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

/// The exit status a death by `sig`'s default action reports, or None when
/// that default action does not end the process.
pub fn fatal_exit_code(sig: u32) -> Option<i32> {
    match default_action(sig) {
        SignalDefaultAction::Terminate => Some(-(sig as i32)),
        SignalDefaultAction::CoreDump => Some(-((sig as i32) | 0x80)),
        _ => None,
    }
}

/// The exit status a fault signal's default action ends a process with, as
/// signal delivery reports it (`fatal_exit_code`), for the fault paths that
/// end the process themselves.
pub fn fault_death_exit_code(sig: u32) -> i32 {
    fatal_exit_code(sig).unwrap_or(-(sig as i32))
}

/// The exit status `process` reports when `sig`'s default action ends it. A
/// SIGKILL that a thread-group death left pending reports the status the
/// group died with (`Process::group_exit_code`), as Linux's group_exit_code.
pub fn signal_death_exit_code(process: &Process, sig: u32) -> i32 {
    match process.group_exit_code {
        Some(code) if sig == SIGKILL => code,
        _ => fatal_exit_code(sig).unwrap_or(-(sig as i32)),
    }
}

/// A signal's default action ends the whole process, not one thread: kill the
/// other live threads of `pid`'s thread group with the status `pid` died with.
/// Must be called with no process-manager lock held, from process context.
///
/// A deferred fault exit can be drained by a thread of one of those peers on
/// its own way back to user mode (ARM64). That peer is not torn down under the
/// thread running this: it gets SIGKILL with the group's status, which its
/// return to user mode acts on.
pub fn terminate_thread_group_peers(pid: crate::process::ProcessId, exit_code: i32) {
    let current = crate::task::scheduler::current_thread_id();
    let peers = crate::process::with_process_manager(|manager| {
        let mut peers = manager.thread_group_peers(pid);
        peers.retain(|&peer| {
            let Some(row) = manager.get_process_mut(peer) else {
                return false;
            };
            let runs_this = current.is_some()
                && row.main_thread.as_ref().map(|thread| thread.id) == current;
            if runs_this {
                row.signals.set_pending(SIGKILL);
                row.group_exit_code.get_or_insert(exit_code);
            }
            !runs_this
        });
        peers
    })
    .unwrap_or_default();
    for peer in peers {
        crate::syscall::signal::kill_process_now(peer, exit_code);
    }
}

/// Take the calling process's next deliverable signal off its pending set
/// when that signal has the default action and the action ends the process,
/// returning its number. The syscall return paths call this under the
/// process-manager lock, so it does no logging, locking or allocation.
pub fn take_fatal_default_signal(process: &mut Process) -> Option<u32> {
    let sig = fatal_default_signal(process)?;
    process.signals.take(sig);
    Some(sig)
}

/// The signal `take_fatal_default_signal` would take, left pending.
pub fn fatal_default_signal(process: &Process) -> Option<u32> {
    // A pending SIGKILL ends the process before any other signal is acted on.
    if sigkill_pending(process) {
        return Some(SIGKILL);
    }
    // A stopped process dies of SIGKILL only; anything else waits for SIGCONT.
    if process.job.stopped.is_some() {
        return None;
    }
    let sig = process.signals.next_deliverable_signal()?;
    if !process.signals.get_handler(sig).is_default() || fatal_exit_code(sig).is_none() {
        return None;
    }
    Some(sig)
}

/// The x86-64 syscall return could not take the process-manager lock, so it
/// leaves pending signals to the next interrupt return. A SIGKILL that a kill
/// left pending because this thread was inside its syscall
/// (`Thread::mark_kill_pending`) must not wait for that: an interrupt return
/// ends the process in place and never retires its row (#1175). Wait for the
/// lock and leave through the syscall return's exit instead, which does not
/// return. Does nothing for a thread no such kill is pending for.
#[cfg(target_arch = "x86_64")]
pub fn exit_if_killed_on_syscall_return() {
    if !crate::per_cpu::current_thread().is_some_and(|thread| thread.kill_pending()) {
        return;
    }
    let Some(thread_id) = crate::task::scheduler::current_thread_id() else {
        return;
    };
    let killed = crate::process::with_process_manager(|manager| {
        let (_, process) = manager.find_process_by_thread_mut(thread_id)?;
        sigkill_pending(process).then(|| process.signals.clear_pending(SIGKILL))
    })
    .flatten();
    if killed.is_some() {
        exit_by_signal_on_syscall_return(SIGKILL);
    }
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
    let exit_code = crate::task::scheduler::current_thread_id()
        .and_then(|thread_id| {
            crate::process::with_process_manager(|manager| {
                manager
                    .find_process_by_thread(thread_id)
                    .map(|(_, process)| signal_death_exit_code(process, sig))
            })
            .flatten()
        })
        .unwrap_or_else(|| fatal_exit_code(sig).unwrap_or(-(sig as i32)));
    exit_on_syscall_return(exit_code)
}

/// Finish an x86-64 syscall return whose caught signal's frame could not be
/// installed (`SignalDeliveryResult::FrameFault`). The process dies as a bad
/// user-stack access, with the status a user page fault reports, through the
/// same exit as `exit_by_signal_on_syscall_return`.
#[cfg(target_arch = "x86_64")]
pub fn exit_frame_fault_on_syscall_return() -> ! {
    exit_on_syscall_return(-(SIGSEGV as i32))
}

#[cfg(target_arch = "x86_64")]
fn exit_on_syscall_return(exit_code: i32) -> ! {
    if let Some(thread_id) = crate::task::scheduler::current_thread_id() {
        let row = crate::process::with_process_manager(|manager| {
            manager
                .find_process_by_thread(thread_id)
                .map(|(pid, _)| pid)
        })
        .flatten();
        if let Some(pid) = row {
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
    let (parent_thread_id, auto_reaped, signal_wake) = {
        let mut manager_guard = crate::process::manager();
        let Some(ref mut manager) = *manager_guard else {
            log::warn!("notify_parent_of_termination_deferred: no process manager");
            return;
        };
        let child_info = manager.get_process(child_pid).map(child_exit_info);
        // A parent that declines zombies reaps the child now; the row is
        // dropped once the guard is released (condition C8).
        let auto_reaped = manager.reap_if_parent_declines(child_pid);

        let signal_wake = manager.queue_process_signal(parent_pid, SIGCHLD, child_info.unwrap_or_else(SigInfo::kernel));
        // Find parent process and wake its child-status wait
        let parent_thread_id = if let Some(parent_process) = manager.get_process_mut(parent_pid) {
            // Send SIGCHLD to parent

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
        };
        (parent_thread_id, auto_reaped, signal_wake)
        // manager_guard is dropped here
    };
    drop(auto_reaped);
    if let Some(tid) = signal_wake {
        crate::task::scheduler::with_scheduler(|s| {
            s.unblock_for_signal(tid);
            s.unblock_for_child_exit(tid);
        });
    }

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

/// The siginfo of the SIGCHLD `child`'s exit raises: how it ended, its PID
/// and real user ID, and its exit status or the signal that ended it.
pub fn child_exit_info(child: &Process) -> SigInfo {
    let (code, status) = child_exit_code_status(child.exit_code.unwrap_or(0));
    SigInfo::child(code, child.id.as_u64() as u32, child.cred.uid, status)
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

/// Collect process timers the scheduler expired, including while this thread
/// was blocked. Expiry and recipient selection belong to the scheduler.
#[inline]
pub fn collect_itimer_signals(process: &mut Process) {
    process.signals.collect_timer_signals(&process.itimers);
}

/// Consume resource-limit signals at a user-return boundary.
#[inline]
pub fn check_cpu_resource_limit(process: &mut Process) {
    process.check_cpu_limit();
}

/// Compatibility entry name used by return paths; this checks CPU limits only.
pub use check_cpu_resource_limit as check_and_fire_alarm;

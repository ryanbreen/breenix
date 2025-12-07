//! Signal delivery to userspace
//!
//! This module handles delivering pending signals to processes when they
//! return to userspace from syscalls or interrupts.

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

/// Deliver pending signals to a process
///
/// Called from check_need_resched_and_switch() before returning to userspace.
/// Returns true if the process state was modified (terminated, stopped, or
/// signal frame set up for user handler).
///
/// # Arguments
/// * `process` - The process to deliver signals to
/// * `interrupt_frame` - The interrupt frame that will be used to return to userspace
/// * `saved_regs` - The saved general-purpose registers
///
/// # Returns
/// * `true` if process state was modified
/// * `false` if no action was taken
pub fn deliver_pending_signals(
    process: &mut Process,
    interrupt_frame: &mut x86_64::structures::idt::InterruptStackFrame,
    saved_regs: &mut crate::task::process_context::SavedRegisters,
) -> bool {
    // Process all deliverable signals in a loop (avoids unbounded recursion)
    loop {
        // Get next deliverable signal
        let sig = match process.signals.next_deliverable_signal() {
            Some(s) => s,
            None => return false,
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
                if deliver_default_action(process, sig) {
                    return true;
                }
                // Default action was Ignore or Continue with no state change - check for more signals
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
                return deliver_to_user_handler(process, interrupt_frame, saved_regs, sig, handler_addr, &action);
            }
        }
    }
}

/// Deliver a signal's default action
fn deliver_default_action(process: &mut Process, sig: u32) -> bool {
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
            true
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
            true
        }
        SignalDefaultAction::Stop => {
            log::info!(
                "Process {} stopped by signal {} ({})",
                process.id.as_u64(),
                sig,
                signal_name(sig)
            );
            process.set_blocked();
            true
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
                true
            } else {
                false
            }
        }
        SignalDefaultAction::Ignore => {
            log::debug!(
                "Signal {} ({}) ignored (default) by process {}",
                sig,
                signal_name(sig),
                process.id.as_u64()
            );
            false
        }
    }
}

/// Set up user stack and registers to call a user-defined signal handler
///
/// This modifies the interrupt frame so that when we return to userspace,
/// we jump to the signal handler instead of the interrupted code.
fn deliver_to_user_handler(
    process: &mut Process,
    interrupt_frame: &mut x86_64::structures::idt::InterruptStackFrame,
    saved_regs: &mut crate::task::process_context::SavedRegisters,
    sig: u32,
    handler_addr: u64,
    action: &SignalAction,
) -> bool {
    // Get current user stack pointer from interrupt frame
    let user_rsp = interrupt_frame.stack_pointer.as_u64();

    // Calculate space needed for trampoline and signal frame
    let frame_size = SignalFrame::SIZE as u64;
    let trampoline_size = super::trampoline::SIGNAL_TRAMPOLINE_SIZE as u64;

    // Allocate space for both trampoline and signal frame on user stack
    // Stack layout (grows down):
    //   [signal frame] <- frame_rsp (16-byte aligned)
    //   [trampoline code] <- trampoline_rsp
    let total_size = frame_size + trampoline_size;
    let frame_rsp = (user_rsp - total_size) & !0xF; // 16-byte align
    let trampoline_rsp = frame_rsp + frame_size;

    // Write trampoline code to user stack first
    // SAFETY: We're writing to user memory that should be valid stack space
    unsafe {
        let trampoline_ptr = trampoline_rsp as *mut u8;
        core::ptr::copy_nonoverlapping(
            super::trampoline::SIGNAL_TRAMPOLINE.as_ptr(),
            trampoline_ptr,
            super::trampoline::SIGNAL_TRAMPOLINE_SIZE,
        );
    }

    // Build signal frame with saved context
    let signal_frame = SignalFrame {
        // Return address points to trampoline code on stack
        // When the handler does 'ret', it will pop this and jump to the trampoline
        // MUST BE AT OFFSET 0 in the struct - verified by struct definition
        trampoline_addr: trampoline_rsp,

        // Magic number for integrity validation
        magic: SignalFrame::MAGIC,

        // Signal info
        signal: sig as u64,
        siginfo_ptr: 0, // Not implemented yet
        ucontext_ptr: 0, // Not implemented yet

        // Save current execution state
        saved_rip: interrupt_frame.instruction_pointer.as_u64(),
        saved_rsp: user_rsp,
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

    log::info!(
        "Signal {} delivered to handler at {:#x}, RSP={:#x}->{:#x}, trampoline={:#x}",
        sig,
        handler_addr,
        user_rsp,
        frame_rsp,
        trampoline_rsp
    );

    true
}

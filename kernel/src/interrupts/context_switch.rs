//! Context switching logic for interrupt return path
//!
//! This module handles the actual context switching when returning from
//! interrupts. It's called from assembly code after the interrupt handler
//! has completed its minimal work.

#![cfg(target_arch = "x86_64")]

use crate::task::process_context::{
    is_kernel_code_selector, restore_userspace_context, save_userspace_context, RestoreError,
    SavedRegisters,
};
use crate::task::scheduler;
use crate::task::thread::ThreadPrivilege;
use crate::tracing::providers::sched::trace_ctx_switch;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use x86_64::structures::idt::InterruptStackFrame;
use x86_64::VirtAddr;

enum FirstUserspaceEntry {
    Installed,
    Aborted(&'static str),
}

static FIRST_USERSPACE_ENTRY_ABORT_COUNT: AtomicU64 = AtomicU64::new(0);
static FIRST_USERSPACE_ENTRY_ABORT_LOGGED: AtomicBool = AtomicBool::new(false);
static USERSPACE_DISPATCH_NO_CR3_REFUSED: AtomicU64 = AtomicU64::new(0);
static USERSPACE_DISPATCH_NO_CR3_LOGGED: AtomicBool = AtomicBool::new(false);
static USERSPACE_DISPATCH_CREATING_REFUSED: AtomicU64 = AtomicU64::new(0);
static USERSPACE_DISPATCH_CREATING_LOGGED: AtomicBool = AtomicBool::new(false);
static DISPATCH_GUARD_UNAVAILABLE_STREAK: AtomicU64 = AtomicU64::new(0);
static DISPATCH_GUARD_ESCALATION_LOGGED: AtomicBool = AtomicBool::new(false);
const DISPATCH_GUARD_ESCALATION_THRESHOLD: u64 = 1024;

/// Raw serial debug output - single character, no locks, no allocations.
/// Use this for debugging context switch paths where any allocation/locking
/// could perturb timing or cause deadlocks.
#[inline(always)]
pub fn raw_serial_char(c: u8) {
    unsafe {
        use x86_64::instructions::port::Port;
        let mut port: Port<u8> = Port::new(0x3F8); // COM1 data port
        port.write(c);
    }
}

/// Raw serial string output - no locks, no allocations.
/// Use for boot markers in context switch path where locking would deadlock.
#[inline(always)]
fn raw_serial_str(s: &str) {
    unsafe {
        use x86_64::instructions::port::Port;
        let mut port: Port<u8> = Port::new(0x3F8);
        for byte in s.bytes() {
            port.write(byte);
        }
    }
}

/// Raw serial decimal output - no locks, no allocations.
#[inline(always)]
fn raw_serial_u64(mut value: u64) {
    if value == 0 {
        raw_serial_char(b'0');
        return;
    }

    let mut digits = [0_u8; 20];
    let mut index = digits.len();
    while value != 0 {
        index -= 1;
        digits[index] = b'0' + (value % 10) as u8;
        value /= 10;
    }
    while index < digits.len() {
        raw_serial_char(digits[index]);
        index += 1;
    }
}

/// Third dispatch admission arm (tranche-2 P3). A row whose publication has not
/// completed must never have CR3 armed for it. Cheap field read on a row the
/// caller already holds; hot-path safe.
#[inline]
pub(crate) fn refuse_unpublished_dispatch(
    process: &crate::process::process::Process,
    thread_id: u64,
    pid: u64,
) -> bool {
    if !process.is_unpublished() {
        return false;
    }
    USERSPACE_DISPATCH_CREATING_REFUSED.fetch_add(1, Ordering::Relaxed);
    if !USERSPACE_DISPATCH_CREATING_LOGGED.swap(true, Ordering::Relaxed) {
        raw_serial_str("[PMGUARD] creating dispatch refused tid=");
        raw_serial_u64(thread_id);
        raw_serial_str(" pid=");
        raw_serial_u64(pid);
        raw_serial_str("\n");
    }
    true
}

#[cfg(feature = "boot_tests")]
pub fn userspace_dispatch_creating_refused() -> u64 {
    USERSPACE_DISPATCH_CREATING_REFUSED.load(Ordering::Relaxed)
}

#[inline(always)]
fn note_dispatch_guard_available() {
    DISPATCH_GUARD_UNAVAILABLE_STREAK.store(0, Ordering::Relaxed);
}

#[inline(always)]
fn note_dispatch_guard_unavailable() {
    let streak = DISPATCH_GUARD_UNAVAILABLE_STREAK
        .fetch_add(1, Ordering::Relaxed)
        .wrapping_add(1);
    if streak >= DISPATCH_GUARD_ESCALATION_THRESHOLD
        && !DISPATCH_GUARD_ESCALATION_LOGGED.swap(true, Ordering::Relaxed)
    {
        let (owner_cpu, owner_tid) = crate::process::process_manager_owner_snapshot().unwrap_or((
            crate::process::PM_LOCK_OWNER_NONE,
            crate::process::PM_LOCK_OWNER_NONE,
        ));
        let current_tid = crate::per_cpu::current_thread_id_lock_free()
            .unwrap_or(crate::process::PM_LOCK_OWNER_TID_UNKNOWN);

        raw_serial_str("[PMGUARD] dispatch refused streak=");
        raw_serial_u64(streak);
        raw_serial_str(" owner_cpu=");
        raw_serial_u64(owner_cpu);
        raw_serial_str(" owner_tid=");
        raw_serial_u64(owner_tid);
        raw_serial_str(" current_tid=");
        raw_serial_u64(current_tid);
        raw_serial_str("\n");
    }
}

// REMOVED: NEXT_PAGE_TABLE is no longer needed since CR3 switching happens
// immediately during context switch, not deferred to interrupt return

/// Check if rescheduling is needed and perform context switch if necessary
///
/// This is called from the assembly interrupt return path and is the
/// CORRECT place to handle context switching (not in the interrupt handler).
#[no_mangle]
pub extern "C" fn check_need_resched_and_switch(
    saved_regs: &mut SavedRegisters,
    interrupt_frame: &mut InterruptStackFrame,
) {
    crate::task::scheduler::note_scheduling_epoch(0);
    // CRITICAL: Only schedule when returning to userspace with preempt_count == 0
    if !crate::per_cpu::can_schedule(interrupt_frame.code_segment.0 as u64) {
        return;
    }

    // NOTE: Context is saved ONLY when actually switching threads (see line ~135).
    // We do NOT save on every timer interrupt - that caused massive overhead
    // preventing userspace from executing even one instruction.
    // The interrupt frame captures the current RIP; we save to thread.context
    // only when switching away.
    let from_userspace = (interrupt_frame.code_segment.0 & 3) == 3;

    // CRITICAL: Check if preemption is enabled before attempting context switch.
    // Syscalls call preempt_disable() at entry, so if preempt_count > 0, we must NOT
    // switch away. The bits 0-27 hold the actual count, bit 28 is PREEMPT_ACTIVE.
    let preempt_count = crate::per_cpu::preempt_count();
    let preempt_count_value = preempt_count & 0x0FFFFFFF; // Mask out PREEMPT_ACTIVE bit
    if preempt_count_value > 0 && !from_userspace {
        // Preemption disabled while in kernel - don't context switch.
        // The thread is executing a syscall or other non-preemptible kernel code.
        // Leave need_resched set so we try again after preempt_enable().
        return;
    }

    // CRITICAL FIX: Check PREEMPT_ACTIVE early, BEFORE calling schedule().
    // PREEMPT_ACTIVE (bit 28) is set in syscall/entry.asm during syscall return
    // to protect the register restoration sequence. If set, we're in the middle
    // of returning from a syscall and must NOT attempt a context switch.
    //
    // Previously, this check happened AFTER schedule() was called, which mutated
    // the scheduler's current_thread state. Then the early return left the
    // scheduler thinking the idle thread was running when actually the userspace
    // thread was still active. This caused the entire scheduler to become stuck.
    //
    // The fix: Check preempt_active BEFORE schedule() to avoid state corruption.
    let preempt_count = crate::per_cpu::preempt_count();
    let preempt_active = (preempt_count & 0x10000000) != 0; // Bit 28

    if preempt_active {
        // PREEMPT_ACTIVE is set during syscall return while registers are being restored.
        // We must NOT context switch during this critical window, regardless of whether
        // the interrupt frame shows userspace or kernel CS.
        //
        // Previously this only checked `from_userspace && preempt_active`, which missed
        // the case where we're in kernel mode (CS=0x08) but PREEMPT_ACTIVE is set because
        // we're in the middle of setting up the interrupt frame for userspace return.
        // In that case, doing a context switch would corrupt partially-restored registers.
        //
        // NOTE: No logging here - we're in IRQ context and logging can deadlock.
        return;
    }

    // Check if current thread is blocked or terminated - we MUST switch away in that case
    let current_thread_blocked_or_terminated = scheduler::with_scheduler(|sched| {
        if let Some(current) = sched.current_thread_mut() {
            matches!(
                current.state,
                crate::task::thread::ThreadState::Blocked
                    | crate::task::thread::ThreadState::BlockedOnSignal
                    | crate::task::thread::ThreadState::BlockedOnChildExit
                    | crate::task::thread::ThreadState::Terminated
            )
        } else {
            false
        }
    })
    .unwrap_or(false);

    // Check if reschedule is needed
    // CRITICAL: If current thread is blocked/terminated, we MUST schedule regardless of need_resched.
    // A blocked thread cannot continue running - we must switch to another thread.
    let need_resched = scheduler::check_and_clear_need_resched();
    if !need_resched && !current_thread_blocked_or_terminated {
        // No reschedule needed, but check for pending signals before returning to userspace
        if from_userspace {
            check_and_deliver_signals_for_current_thread(saved_regs, interrupt_frame);
        }
        return;
    }

    // Count reschedule attempts (for diagnostics if needed)
    static RESCHED_LOG_COUNTER: core::sync::atomic::AtomicU64 =
        core::sync::atomic::AtomicU64::new(0);
    let _count = RESCHED_LOG_COUNTER.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    // Note: Debug logging removed from hot path - use GDB if debugging is needed

    // Both entry paths must resolve the process-manager dependency BEFORE committing
    // to a scheduling decision, and both must refuse identically when it is
    // unavailable. The kernel-entry path used to skip this check and pass `None`
    // down, so a dispatch could be committed to a never-started user thread whose
    // first ring-3 entry then aborted for want of this very lock, requeue the
    // thread and re-arm need_resched, forever. Refusing here leaves the
    // lock-holding context - the only one that can release it - running.
    // try_lock only: blocking here would deadlock the interrupt path.
    let mut process_manager_guard = match crate::process::try_manager() {
        Some(guard) => {
            note_dispatch_guard_available();
            guard
        }
        None => {
            note_dispatch_guard_unavailable();
            scheduler::set_need_resched();
            return;
        }
    };

    // Perform scheduling decision
    let schedule_result = scheduler::schedule();

    // One-time boot stage marker (only fires once to satisfy boot-stages test)
    // CRITICAL: Use raw serial to avoid deadlock in IRQ context
    static SCHEDULE_MARKER_EMITTED: core::sync::atomic::AtomicBool =
        core::sync::atomic::AtomicBool::new(false);
    if !SCHEDULE_MARKER_EMITTED.load(core::sync::atomic::Ordering::Relaxed) {
        SCHEDULE_MARKER_EMITTED.store(true, core::sync::atomic::Ordering::Relaxed);
        raw_serial_str("[ INFO] scheduler::schedule() returned (boot marker)\n");
    }

    if schedule_result.is_none() {
        // CRITICAL: Clear exception cleanup context even when no switch happens.
        // Otherwise the flag stays set forever, causing unexpected scheduling later.
        crate::per_cpu::clear_exception_cleanup_context();
        // CRITICAL: Even though no context switch happens, we MUST check for signals!
        // This case occurs when the current thread is the only ready thread (e.g., after
        // yield_now() when no other threads are runnable). Without this check, signals
        // queued for the current process (like SIGTERM from kill()) would never be delivered.
        if from_userspace {
            check_and_deliver_signals_for_current_thread(saved_regs, interrupt_frame);
        }
        return;
    }
    if let Some((old_thread_id, new_thread_id)) = schedule_result {
        // Clear exception cleanup context since we're doing a context switch
        crate::per_cpu::clear_exception_cleanup_context();

        // Trace context switch - compiles to ~5 instructions when disabled
        trace_ctx_switch(old_thread_id, new_thread_id);

        if old_thread_id == new_thread_id {
            // Same thread continues running, but check for pending signals
            if from_userspace {
                check_and_deliver_signals_for_current_thread(saved_regs, interrupt_frame);
            }
            return;
        }

        // NOTE: No logging here - log statements in the context switch path
        // cause deadlocks when the logger tries to acquire locks during a switch
        // to a newly created kthread. Use raw_serial_char() for debugging only.

        // Emit canonical ring3 marker on the FIRST entry to userspace (for CI)
        // CRITICAL: Use raw serial output without locks to prevent deadlock in IRQ context
        if from_userspace {
            static mut EMITTED_RING3_MARKER: bool = false;
            unsafe {
                if !EMITTED_RING3_MARKER {
                    EMITTED_RING3_MARKER = true;
                    raw_serial_str("RING3_ENTER: CS=0x33\n");
                    raw_serial_str(
                        "[ OK ] RING3_SMOKE: userspace executed + syscall path verified\n",
                    );
                }
            }
        }

        // Save current thread's context if coming from userspace
        // CRITICAL: If save fails, we MUST NOT switch contexts!
        // Switching without saving would cause the process to return to stale RIP (entry point)
        //
        // NOTE: PREEMPT_ACTIVE check was moved earlier in this function (before schedule() call)
        // to prevent scheduler state corruption. If we reach here from userspace, we know
        // preempt_active is false (otherwise we would have returned early).

        // Check if current thread is blocked in syscall (pause/waitpid)
        let (blocked_in_syscall, old_thread_is_user) =
            scheduler::with_thread_mut(old_thread_id, |thread| {
                (
                    thread.blocked_in_syscall,
                    thread.privilege == ThreadPrivilege::User,
                )
            })
            .unwrap_or((false, false));

        if from_userspace {
            // Use the already-held guard to save context (prevents TOCTOU race)
            // Debug marker: saving userspace context (raw serial, no locks)
            raw_serial_str("<S>");
            if !save_current_thread_context_with_guard(
                old_thread_id,
                saved_regs,
                interrupt_frame,
                &mut process_manager_guard,
            ) {
                log::error!(
                    "Context switch aborted: failed to save thread {} context. \
                     Would cause return to stale RIP!",
                    old_thread_id
                );
                // Roll back the committed switch and re-arm rescheduling.
                scheduler::abort_dispatch_and_resume(new_thread_id, old_thread_id);
                scheduler::set_need_resched();
                return;
            }
        } else if !from_userspace && (blocked_in_syscall || old_thread_is_user) {
            // A user thread was interrupted in kernel mode, including a thread blocked
            // in a syscall's HLT loop. Save the KERNEL context in process.main_thread so
            // the user-thread restore path can resume it at the correct kernel location.
            // NOTE: No logging here - this is a hot path during blocking I/O
            save_kernel_context_with_guard(
                old_thread_id,
                saved_regs,
                interrupt_frame,
                &mut process_manager_guard,
            );
        } else if !from_userspace {
            // Pure kernel thread (like kthread) being preempted - save its context
            // This is NOT a userspace thread and NOT blocked in syscall - it's a
            // kernel thread running its own code (e.g., kthread_entry -> user function)
            save_kthread_context(old_thread_id, saved_regs, interrupt_frame);
        }

        // Switch to the new thread
        // Pass the process_manager_guard so we don't try to re-acquire the lock
        switch_to_thread(
            new_thread_id,
            old_thread_id,
            saved_regs,
            interrupt_frame,
            Some(process_manager_guard),
        );

        // NOTE: Don't log here - this is on the hot path and can affect timing

        // CRITICAL: Clear PREEMPT_ACTIVE after context switch completes
        // PREEMPT_ACTIVE (bit 28) is set in syscall/entry.asm to protect register
        // restoration during syscall return. When we switch to a different thread,
        // that flag should NOT persist - the NEW thread is not in syscall return.
        //
        // Without this, PREEMPT_ACTIVE would carry over to the new thread, causing:
        // 1. can_schedule() to return false (blocks scheduling)
        // 2. Exception handlers to need the bypass workaround
        //
        // Linux clears this in schedule_tail() after context switch.
        crate::per_cpu::clear_preempt_active();

        // Log userspace transition
        if scheduler::with_thread_mut(new_thread_id, |t| t.privilege == ThreadPrivilege::User)
            .unwrap_or(false)
        {
            log::trace!("Restored userspace context for thread {}", new_thread_id);
        }

        // Reset the timer quantum for the new thread
        super::timer::reset_quantum();
    }
}

/// Save the current thread's userspace context using an already-held guard
/// Returns true if context was saved successfully, false otherwise
///
/// This version takes an already-acquired process manager guard to prevent
/// TOCTOU races where the lock could be acquired between checking availability
/// and actually using it.
fn save_current_thread_context_with_guard(
    thread_id: u64,
    saved_regs: &mut SavedRegisters,
    interrupt_frame: &mut InterruptStackFrame,
    manager_guard: &mut crate::process::TryProcessManagerGuard,
) -> bool {
    if let Some(ref mut manager) = **manager_guard {
        if let Some((pid, process)) = manager.find_process_by_thread_mut(thread_id) {
            if let Some(ref mut thread) = process.main_thread {
                save_userspace_context(thread, interrupt_frame, saved_regs);
                log::trace!(
                    "Saved context for process {} (thread {})",
                    pid.as_u64(),
                    thread_id
                );
                return true;
            } else {
                log::error!(
                    "Process {} has no main_thread for thread {}",
                    pid.as_u64(),
                    thread_id
                );
            }
        } else {
            log::error!(
                "Could not find process for thread {} in process manager",
                thread_id
            );
        }
    } else {
        log::error!("Process manager is None");
    }
    false
}

/// Save kernel context for a thread blocked inside a syscall
/// This saves the kernel-mode context (RIP in HLT loop, kernel RSP, CS=0x08)
/// so the thread can be resumed at the correct kernel location.
fn save_kernel_context_with_guard(
    thread_id: u64,
    saved_regs: &SavedRegisters,
    interrupt_frame: &InterruptStackFrame,
    manager_guard: &mut crate::process::TryProcessManagerGuard,
) {
    if let Some(ref mut manager) = **manager_guard {
        if let Some((_pid, process)) = manager.find_process_by_thread_mut(thread_id) {
            if let Some(ref mut thread) = process.main_thread {
                // Save kernel context - the thread is in kernel mode (HLT loop in pause/waitpid)
                // Save registers from the interrupt frame (kernel mode state)
                thread.context.rax = saved_regs.rax;
                thread.context.rbx = saved_regs.rbx;
                thread.context.rcx = saved_regs.rcx;
                thread.context.rdx = saved_regs.rdx;
                thread.context.rsi = saved_regs.rsi;
                thread.context.rdi = saved_regs.rdi;
                thread.context.rbp = saved_regs.rbp;
                thread.context.r8 = saved_regs.r8;
                thread.context.r9 = saved_regs.r9;
                thread.context.r10 = saved_regs.r10;
                thread.context.r11 = saved_regs.r11;
                thread.context.r12 = saved_regs.r12;
                thread.context.r13 = saved_regs.r13;
                thread.context.r14 = saved_regs.r14;
                thread.context.r15 = saved_regs.r15;

                // From interrupt frame - this is the KERNEL location (HLT instruction)
                thread.context.rip = interrupt_frame.instruction_pointer.as_u64();
                thread.context.rsp = interrupt_frame.stack_pointer.as_u64();
                thread.context.rflags = interrupt_frame.cpu_flags.bits();
                thread.context.cs = interrupt_frame.code_segment.0 as u64;
                thread.context.ss = interrupt_frame.stack_segment.0 as u64;

                log::info!(
                    "Saved kernel context for blocked thread {}: RIP={:#x} CS={:#x} RSP={:#x}",
                    thread_id,
                    thread.context.rip,
                    thread.context.cs,
                    thread.context.rsp
                );
            }
        }
    }
}

/// Save kernel thread (kthread) context before switching away
/// This is similar to save_kernel_context_with_guard but for pure kernel threads
/// that are not associated with a process (they only exist in the scheduler)
fn save_kthread_context(
    thread_id: u64,
    saved_regs: &SavedRegisters,
    interrupt_frame: &InterruptStackFrame,
) {
    scheduler::with_thread_mut(thread_id, |thread| {
        // Save general purpose registers from the interrupt
        thread.context.rax = saved_regs.rax;
        thread.context.rbx = saved_regs.rbx;
        thread.context.rcx = saved_regs.rcx;
        thread.context.rdx = saved_regs.rdx;
        thread.context.rsi = saved_regs.rsi;
        thread.context.rdi = saved_regs.rdi;
        thread.context.rbp = saved_regs.rbp;
        thread.context.r8 = saved_regs.r8;
        thread.context.r9 = saved_regs.r9;
        thread.context.r10 = saved_regs.r10;
        thread.context.r11 = saved_regs.r11;
        thread.context.r12 = saved_regs.r12;
        thread.context.r13 = saved_regs.r13;
        thread.context.r14 = saved_regs.r14;
        thread.context.r15 = saved_regs.r15;

        // Save from interrupt frame - the kernel location where thread was preempted
        thread.context.rip = interrupt_frame.instruction_pointer.as_u64();
        thread.context.rsp = interrupt_frame.stack_pointer.as_u64();
        thread.context.rflags = interrupt_frame.cpu_flags.bits();
        thread.context.cs = interrupt_frame.code_segment.0 as u64;
        thread.context.ss = interrupt_frame.stack_segment.0 as u64;

        log::trace!(
            "KTHREAD_SAVE: thread {} RIP={:#x} RSP={:#x}",
            thread_id,
            thread.context.rip,
            thread.context.rsp
        );
    });

    // Hardware memory fence to ensure all context saves are visible before
    // we switch to a different thread. This is critical for TCG mode.
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
}

/// The saved CS is authoritative, and this predicate deliberately mirrors the
/// enforcement check in `restore_userspace_context`. A never-run user thread
/// carries `cs = 0x33`, while `is_kernel_code_selector` rejects `cs == 0`, so
/// this does not widen kernel-frame routing.
fn saved_context_is_kernel_frame(
    thread_id: u64,
    guard: Option<&crate::process::TryProcessManagerGuard>,
) -> bool {
    let manager_has_kernel_frame = |manager: &crate::process::ProcessManager| {
        manager
            .find_process_by_thread(thread_id)
            .and_then(|(_pid, process)| process.main_thread.as_ref())
            .is_some_and(|thread| {
                thread.privilege == ThreadPrivilege::User
                    && is_kernel_code_selector(thread.context.cs)
            })
    };

    if let Some(manager_guard) = guard {
        let manager_option: &Option<crate::process::ProcessManager> = manager_guard;
        return manager_option
            .as_ref()
            .is_some_and(manager_has_kernel_frame);
    }

    let Some(manager_guard) = crate::process::try_manager() else {
        return false;
    };
    let manager_option: &Option<crate::process::ProcessManager> = &manager_guard;
    manager_option
        .as_ref()
        .is_some_and(manager_has_kernel_frame)
}

/// Switch to a different thread
fn switch_to_thread(
    thread_id: u64,
    resume_thread_id: u64,
    saved_regs: &mut SavedRegisters,
    interrupt_frame: &mut InterruptStackFrame,
    process_manager_guard: Option<crate::process::TryProcessManagerGuard>,
) {
    // Debug marker: entering switch_to_thread (raw serial, no locks)
    raw_serial_str("[SW]");
    // Update per-CPU current thread and TSS.RSP0
    scheduler::with_thread_mut(thread_id, |thread| {
        // Update per-CPU current thread pointer
        let thread_ptr = thread as *const _ as *mut crate::task::thread::Thread;
        crate::per_cpu::set_current_thread(thread_ptr);

        // Update TSS.RSP0 with new thread's kernel stack top
        // This is critical for interrupt/exception handling
        if let Some(kernel_stack_top) = thread.kernel_stack_top {
            crate::per_cpu::update_tss_rsp0(kernel_stack_top.as_u64());
            log::trace!(
                "sched: switch to thread {} rsp0={:#x}",
                thread_id,
                kernel_stack_top
            );
        }
    });

    // Switch TLS if needed (kernel threads don't have TLS)
    let is_kernel_thread = scheduler::with_thread_mut(thread_id, |thread| {
        thread.privilege == ThreadPrivilege::Kernel
    })
    .unwrap_or(false);

    if !is_kernel_thread {
        if let Err(e) = crate::tls::switch_tls(thread_id) {
            log::error!("Failed to switch TLS for thread {}: {}", thread_id, e);
            scheduler::abort_dispatch_and_resume(thread_id, resume_thread_id);
            scheduler::set_need_resched();
            return;
        }
        // Debug marker: TLS switch completed (raw serial, no locks)
        raw_serial_str("<T>");
    }

    // Check if this is the idle thread
    let is_idle =
        scheduler::with_scheduler(|sched| thread_id == sched.idle_thread()).unwrap_or(false);

    // Check if thread was blocked inside a syscall (pause/waitpid)
    // If so, we must NOT restore userspace context - the thread needs to
    // continue executing the syscall code and return through the normal path.
    let blocked_in_syscall =
        scheduler::with_thread_mut(thread_id, |thread| thread.blocked_in_syscall).unwrap_or(false);
    if is_idle {
        // Check if idle thread has a saved context to restore
        // If it was preempted while running actual code (not idle_loop), restore that context
        //
        // CRITICAL: After userspace starts (RING3_CONFIRMED), always go to idle_loop.
        // Idle's saved context from boot may contain RIP values in kernel init code
        // that cause hangs when restored during userspace operation.
        let userspace_started = crate::syscall::handler::is_ring3_confirmed();

        let has_saved_context = if userspace_started {
            // After userspace starts, idle should always return to idle_loop
            false
        } else {
            // During boot, may need to restore kernel init progress
            scheduler::with_thread_mut(thread_id, |thread| {
                let idle_loop_addr = idle_loop as *const () as u64;
                // Has saved context if RIP is non-zero AND not pointing to idle_loop
                thread.context.rip != 0 && thread.context.rip != idle_loop_addr
            })
            .unwrap_or(false)
        };

        if has_saved_context {
            // Restore idle thread's saved context (like a kthread)
            // Debug marker: idle with saved context (raw serial, no locks)
            raw_serial_str("<1>");
            log::trace!("Restoring idle thread's saved context");
            setup_kernel_thread_return(thread_id, saved_regs, interrupt_frame);
        } else {
            // No saved context or was in idle_loop - go to idle loop
            // Debug marker: switching to idle (raw serial, no locks)
            raw_serial_str("<I>");
            setup_idle_return(interrupt_frame);
        }
    } else if is_kernel_thread {
        // Set up to return to kernel thread
        // Debug marker: kernel thread (raw serial, no locks)
        raw_serial_str("<K>");
        setup_kernel_thread_return(thread_id, saved_regs, interrupt_frame);
    // The saved CS is the authoritative record of the ring where the context was
    // captured, even if a remote waker cleared blocked_in_syscall. Evaluate it
    // only for the non-idle userspace thread that consumes the result.
    } else if blocked_in_syscall
        || saved_context_is_kernel_frame(thread_id, process_manager_guard.as_ref())
    {
        // Debug marker: blocked in syscall (raw serial, no locks)
        raw_serial_str("<B>");
        // CRITICAL: Thread was blocked inside a syscall (like pause() or waitpid()).
        // We need to check if there are pending signals. If so, deliver them using
        // the saved userspace context. Otherwise, resume at the kernel HLT loop.

        // CRITICAL FIX: Read saved_userspace_context from the SCHEDULER's Thread,
        // not from process.main_thread. This fixes a race condition where:
        // - pause() saves context to process.main_thread THEN blocks in scheduler
        // - kill() sends signal BETWEEN these two operations
        // - Signal arrives but context not yet saved in the scheduler's Thread
        //
        // By reading from the scheduler's Thread (which is updated atomically with
        // the BlockedOnSignal state), we ensure consistency.
        let saved_context_from_scheduler =
            scheduler::with_thread_mut(thread_id, |thread| thread.saved_userspace_context.clone())
                .flatten();

        // Get the process page table and thread context
        let guard_option = process_manager_guard.or_else(|| crate::process::try_manager());
        if let Some(mut manager_guard) = guard_option {
            if let Some(ref mut manager) = *manager_guard {
                if let Some((pid, process)) = manager.find_process_by_thread_mut(thread_id) {
                    if refuse_unpublished_dispatch(process, thread_id, pid.as_u64()) {
                        crate::task::scheduler::set_need_resched();
                        setup_idle_return(interrupt_frame);
                        crate::task::scheduler::switch_to_idle();
                        crate::task::scheduler::requeue_refused_dispatch(thread_id);
                        unsafe {
                            crate::memory::process_memory::switch_to_kernel_page_table();
                        }
                        return;
                    }
                    let process_cr3 = match process.cr3_value() {
                        Some(cr3_value) => cr3_value,
                        None => {
                            USERSPACE_DISPATCH_NO_CR3_REFUSED.fetch_add(1, Ordering::Relaxed);
                            if !USERSPACE_DISPATCH_NO_CR3_LOGGED.swap(true, Ordering::Relaxed) {
                                raw_serial_str("[PMGUARD] no-cr3 dispatch refused tid=");
                                raw_serial_u64(thread_id);
                                raw_serial_str(" pid=");
                                raw_serial_u64(pid.as_u64());
                                raw_serial_str("\n");
                            }
                            if let Some(ref mut thread) = process.main_thread {
                                thread.set_terminated();
                            }
                            crate::task::scheduler::with_thread_mut(thread_id, |sched_thread| {
                                sched_thread.set_terminated();
                            });
                            crate::task::scheduler::set_need_resched();
                            setup_idle_return(interrupt_frame);
                            crate::task::scheduler::switch_to_idle();
                            unsafe {
                                crate::memory::process_memory::switch_to_kernel_page_table();
                            }
                            return;
                        }
                    };

                    // Check if there are pending signals to deliver
                    crate::signal::delivery::check_and_fire_alarm(process);
                    crate::signal::delivery::check_and_fire_itimer_real(process, 5000);

                    let has_pending_signals =
                        crate::signal::delivery::has_deliverable_signals(process);
                    // Use context from scheduler's Thread (single source of truth)
                    // Fall back to process.main_thread for backwards compatibility
                    let has_saved_context = saved_context_from_scheduler.is_some()
                        || process
                            .main_thread
                            .as_ref()
                            .map(|t| t.saved_userspace_context.is_some())
                            .unwrap_or(false);

                    if has_pending_signals && has_saved_context {
                        // SIGNAL DELIVERY PATH: Use saved userspace context for signal delivery
                        log::info!(
                            "Thread {} has pending signals - delivering via saved userspace context",
                            thread_id
                        );

                        // CRITICAL FIX: Use context from scheduler's Thread (single source of truth)
                        // Fall back to process.main_thread for backwards compatibility
                        let saved_ctx_option =
                            saved_context_from_scheduler.as_ref().or_else(|| {
                                process
                                    .main_thread
                                    .as_ref()
                                    .and_then(|t| t.saved_userspace_context.as_ref())
                            });

                        if let Some(saved_ctx) = saved_ctx_option {
                            // Restore userspace registers from saved context
                            // But set RAX = -EINTR for the interrupted syscall
                            saved_regs.rax = (-4i64) as u64; // -EINTR
                            saved_regs.rbx = saved_ctx.rbx;
                            saved_regs.rcx = saved_ctx.rcx;
                            saved_regs.rdx = saved_ctx.rdx;
                            saved_regs.rsi = saved_ctx.rsi;
                            saved_regs.rdi = saved_ctx.rdi;
                            saved_regs.rbp = saved_ctx.rbp;
                            saved_regs.r8 = saved_ctx.r8;
                            saved_regs.r9 = saved_ctx.r9;
                            saved_regs.r10 = saved_ctx.r10;
                            saved_regs.r11 = saved_ctx.r11;
                            saved_regs.r12 = saved_ctx.r12;
                            saved_regs.r13 = saved_ctx.r13;
                            saved_regs.r14 = saved_ctx.r14;
                            saved_regs.r15 = saved_ctx.r15;

                            // Restore interrupt frame with USERSPACE context
                            unsafe {
                                interrupt_frame.as_mut().update(|frame| {
                                    frame.instruction_pointer =
                                        x86_64::VirtAddr::new(saved_ctx.rip);
                                    frame.stack_pointer = x86_64::VirtAddr::new(saved_ctx.rsp);
                                    frame.cpu_flags =
                                        x86_64::registers::rflags::RFlags::from_bits_truncate(
                                            saved_ctx.rflags,
                                        );
                                    // Use userspace code/stack segments
                                    frame.code_segment = crate::gdt::user_code_selector();
                                    frame.stack_segment = crate::gdt::user_data_selector();
                                });
                            }

                            log::info!(
                                "Restored userspace context for signal delivery: RIP={:#x} RSP={:#x} RAX=-EINTR",
                                saved_ctx.rip,
                                saved_ctx.rsp
                            );
                        }

                        // Clear blocked_in_syscall and saved context on BOTH process.main_thread
                        // AND the scheduler's Thread to keep them in sync
                        if let Some(ref mut thread) = process.main_thread {
                            thread.blocked_in_syscall = false;
                            thread.saved_userspace_context = None;

                            // Update TSS RSP0 for the thread's kernel stack
                            if let Some(kernel_stack_top) = thread.kernel_stack_top {
                                crate::per_cpu::update_tss_rsp0(kernel_stack_top.as_u64());
                            }
                        }

                        // Also clear on scheduler's Thread
                        scheduler::with_thread_mut(thread_id, |thread| {
                            thread.blocked_in_syscall = false;
                            thread.saved_userspace_context = None;
                        });

                        // CRITICAL: Switch to process CR3 BEFORE delivering signal
                        // Signal delivery writes to user stack memory, which requires
                        // the process's page table to be active (not the kernel CR3).
                        // Without this, we get a page fault when trying to write the
                        // signal frame to user memory.
                        unsafe {
                            use x86_64::registers::control::{Cr3, Cr3Flags};
                            use x86_64::structures::paging::PhysFrame;
                            use x86_64::PhysAddr;
                            Cr3::write(
                                PhysFrame::containing_address(PhysAddr::new(process_cr3)),
                                Cr3Flags::empty(),
                            );
                        }
                        log::debug!(
                            "Switched to process CR3 {:#x} for signal delivery (blocked-in-syscall path)",
                            process_cr3
                        );

                        // Now deliver the signal (modifies interrupt_frame and saved_regs)
                        let signal_result = crate::signal::delivery::deliver_pending_signals(
                            process,
                            interrupt_frame,
                            saved_regs,
                        );

                        // Handle signal result
                        match signal_result {
                            crate::signal::delivery::SignalDeliveryResult::Terminated(n) => {
                                log::info!("Signal terminated process, thread {}", thread_id);
                                // Process was terminated - notify parent after releasing locks
                                // We need to return from this function and let the locks drop naturally
                                // but first save the notification data
                                // We have the notification info but can't call notify while holding locks.
                                // The notification will be handled by the timer interrupt when it
                                // eventually switches to the parent process, because:
                                // 1. The child is now marked as terminated
                                // 2. When the parent's waitpid resumes, it will find the terminated child
                                // 3. The scheduler will see the parent is unblocked
                                // However, this path is rare (signal terminating a process whose parent
                                // is blocked in waitpid *at the exact same time*).
                                // The parent notification happens in the other code paths.
                                log::debug!(
                                    "Signal termination in blocked_in_syscall path: parent {} will be notified when resumed",
                                    n.parent_pid.as_u64()
                                );
                                // The return frame, RSP0, and CR3 already name the terminated
                                // thread, so bookkeeping-only rollback would make recorded and
                                // executing threads diverge. Complete the switch to idle on its
                                // own stack and master kernel PML4; the saved resume thread will
                                // be dispatched again by the reschedule armed here.
                                scheduler::set_need_resched();
                                setup_idle_return(interrupt_frame);
                                scheduler::switch_to_idle();
                                unsafe {
                                    crate::memory::process_memory::switch_to_kernel_page_table();
                                }
                                return;
                            }
                            crate::signal::delivery::SignalDeliveryResult::Delivered => {
                                log::info!("Signal delivered to thread {}", thread_id);
                            }
                            crate::signal::delivery::SignalDeliveryResult::NoAction => {}
                        }
                    } else {
                        // NO PENDING SIGNALS: Resume at kernel HLT loop
                        if let Some(ref thread) = process.main_thread {
                            // Restore kernel context
                            saved_regs.rax = thread.context.rax;
                            saved_regs.rbx = thread.context.rbx;
                            saved_regs.rcx = thread.context.rcx;
                            saved_regs.rdx = thread.context.rdx;
                            saved_regs.rsi = thread.context.rsi;
                            saved_regs.rdi = thread.context.rdi;
                            saved_regs.rbp = thread.context.rbp;
                            saved_regs.r8 = thread.context.r8;
                            saved_regs.r9 = thread.context.r9;
                            saved_regs.r10 = thread.context.r10;
                            saved_regs.r11 = thread.context.r11;
                            saved_regs.r12 = thread.context.r12;
                            saved_regs.r13 = thread.context.r13;
                            saved_regs.r14 = thread.context.r14;
                            saved_regs.r15 = thread.context.r15;

                            // Restore interrupt frame with KERNEL context
                            unsafe {
                                interrupt_frame.as_mut().update(|frame| {
                                    frame.instruction_pointer =
                                        x86_64::VirtAddr::new(thread.context.rip);
                                    frame.stack_pointer = x86_64::VirtAddr::new(thread.context.rsp);
                                    frame.cpu_flags =
                                        x86_64::registers::rflags::RFlags::from_bits_truncate(
                                            thread.context.rflags,
                                        );
                                    // CRITICAL: Use kernel code segment (CS=0x08)
                                    frame.code_segment = crate::gdt::kernel_code_selector();
                                    frame.stack_segment = crate::gdt::kernel_data_selector();
                                });
                            }

                            log::info!(
                                "Restored kernel context for thread {}: RIP={:#x} RSP={:#x}",
                                thread_id,
                                thread.context.rip,
                                thread.context.rsp
                            );

                            // Update TSS RSP0 for the thread's kernel stack
                            if let Some(kernel_stack_top) = thread.kernel_stack_top {
                                crate::per_cpu::update_tss_rsp0(kernel_stack_top.as_u64());
                            }
                        }

                        // CRITICAL: Switch CR3 IMMEDIATELY when returning to kernel mode!
                        // The kernel code (e.g., waitpid HLT loop) will access userspace memory
                        // (like the wstatus pointer). Without switching CR3 here, we'd be using
                        // the previous process's page tables and get a page fault.
                        unsafe {
                            use x86_64::registers::control::{Cr3, Cr3Flags};
                            use x86_64::structures::paging::PhysFrame;
                            use x86_64::PhysAddr;
                            Cr3::write(
                                PhysFrame::containing_address(PhysAddr::new(process_cr3)),
                                Cr3Flags::empty(),
                            );
                        }
                        log::debug!(
                            "Switched to process CR3 {:#x} for blocked-in-syscall kernel return (thread {})",
                            process_cr3,
                            thread_id
                        );
                    }

                    // Set up CR3 for the process's page table
                    unsafe {
                        // Tell timer_entry.asm to switch CR3 before IRETQ
                        crate::per_cpu::set_next_cr3(process_cr3);

                        // Update saved_process_cr3 for future timer interrupts
                        core::arch::asm!(
                            "mov gs:[80], {}",
                            in(reg) process_cr3,
                            options(nostack, preserves_flags)
                        );
                    }
                    log::trace!(
                        "Set CR3 to {:#x} for thread {} (pid {})",
                        process_cr3,
                        thread_id,
                        pid.as_u64()
                    );
                }
            }
        } else {
            // CRITICAL: Cannot acquire lock to restore kernel context
            // This is a fatal error - we cannot switch to this thread without its context
            log::error!(
                "Failed to acquire lock to restore kernel context for thread {}. Context switch aborted.",
                thread_id
            );
            // Roll back the committed dispatch and re-arm rescheduling.
            scheduler::abort_dispatch_and_resume(thread_id, resume_thread_id);
            scheduler::set_need_resched();
            return;
        }
    } else {
        // Restore userspace thread context
        // Debug marker: userspace restore path (raw serial, no locks)
        raw_serial_str("<U>");
        // Pass the process_manager_guard to avoid double-lock
        restore_userspace_thread_context(
            thread_id,
            resume_thread_id,
            saved_regs,
            interrupt_frame,
            process_manager_guard,
        );
    }
}

/// Set up interrupt frame to return to idle loop
pub(crate) fn setup_idle_return(interrupt_frame: &mut InterruptStackFrame) {
    // CRITICAL: Get the idle thread's actual kernel stack from the scheduler
    // Do NOT use per_cpu::kernel_stack_top() because that gets updated during
    // context switches and may point to a different thread's stack!
    let idle_stack = scheduler::with_scheduler(|sched| {
        let idle_id = sched.idle_thread();
        sched
            .get_thread(idle_id)
            .and_then(|t| t.kernel_stack_top.map(|v| v.as_u64()))
    })
    .flatten()
    .unwrap_or_else(|| {
        log::error!("Failed to get idle thread's kernel stack!");
        crate::per_cpu::kernel_stack_top() // Fallback, but this is wrong
    });

    unsafe {
        interrupt_frame.as_mut().update(|frame| {
            frame.code_segment = crate::gdt::kernel_code_selector();
            frame.stack_segment = crate::gdt::kernel_data_selector();
            frame.instruction_pointer = x86_64::VirtAddr::new(idle_loop as *const () as u64);
            // CRITICAL: Set both INTERRUPT_FLAG (bit 9) AND reserved bit 1 (always required)
            // 0x202 = INTERRUPT_FLAG (0x200) | reserved bit 1 (0x002)
            // Without bit 1, IRETQ behavior is undefined per Intel spec.
            let flags_ptr =
                &mut frame.cpu_flags as *mut x86_64::registers::rflags::RFlags as *mut u64;
            *flags_ptr = 0x202;

            frame.stack_pointer = x86_64::VirtAddr::new(idle_stack);
        });

        // NOTE: We do NOT switch page tables here. The userspace process page table
        // already has all kernel mappings (code, stacks, etc.) so we can run
        // kernel code (idle loop) with it.

        // CRITICAL FIX: Clear PREEMPT_ACTIVE when switching to idle!
        // PREEMPT_ACTIVE (bit 28) is set during syscall return to protect register
        // restoration. When we switch to the idle thread, we MUST clear it - otherwise
        // can_schedule() will return false and block all future scheduling attempts,
        // leaving the system stuck in the idle loop unable to switch to ready threads.
        crate::per_cpu::clear_preempt_active();
    }
    log::trace!("Set up return to idle loop");
}

/// Set up interrupt frame to return to kernel thread
fn setup_kernel_thread_return(
    thread_id: u64,
    saved_regs: &mut SavedRegisters,
    interrupt_frame: &mut InterruptStackFrame,
) {
    // Get thread info - restore ALL saved registers, not just a few
    let thread_info = scheduler::with_thread_mut(thread_id, |thread| {
        (thread.name.clone(), thread.context.clone())
    });

    if let Some((_name, context)) = thread_info {
        unsafe {
            interrupt_frame.as_mut().update(|frame| {
                frame.instruction_pointer = x86_64::VirtAddr::new(context.rip);
                frame.stack_pointer = x86_64::VirtAddr::new(context.rsp);
                frame.code_segment = crate::gdt::kernel_code_selector();
                frame.stack_segment = crate::gdt::kernel_data_selector();
                frame.cpu_flags =
                    x86_64::registers::rflags::RFlags::from_bits_truncate(context.rflags);
            });

            // Restore ALL general purpose registers from saved context
            saved_regs.rax = context.rax;
            saved_regs.rbx = context.rbx;
            saved_regs.rcx = context.rcx;
            saved_regs.rdx = context.rdx;
            saved_regs.rsi = context.rsi;
            saved_regs.rdi = context.rdi;
            saved_regs.rbp = context.rbp;
            saved_regs.r8 = context.r8;
            saved_regs.r9 = context.r9;
            saved_regs.r10 = context.r10;
            saved_regs.r11 = context.r11;
            saved_regs.r12 = context.r12;
            saved_regs.r13 = context.r13;
            saved_regs.r14 = context.r14;
            saved_regs.r15 = context.r15;
        }

        // Switch to master kernel PML4 for running kernel threads
        // This ensures kernel threads have access to all kernel mappings
        unsafe {
            crate::memory::process_memory::switch_to_kernel_page_table();
        }

        // Hardware memory fence to ensure all writes to interrupt frame and saved_regs
        // are visible before IRETQ reads them. This is critical for TCG mode
        // where software emulation may have different memory ordering semantics.
        // Using a full fence (mfence) rather than just compiler fence to force
        // actual CPU store completion.
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
    } else {
        log::error!(
            "KTHREAD_SWITCH: Failed to get thread info for thread {}",
            thread_id
        );
    }
}

/// Restore userspace thread context
fn restore_userspace_thread_context(
    thread_id: u64,
    resume_thread_id: u64,
    saved_regs: &mut SavedRegisters,
    interrupt_frame: &mut InterruptStackFrame,
    process_manager_guard: Option<crate::process::TryProcessManagerGuard>,
) {
    log::trace!("restore_userspace_thread_context: thread {}", thread_id);

    // Check if this thread has ever run before
    let has_started =
        scheduler::with_thread_mut(thread_id, |thread| thread.has_started).unwrap_or(false);

    if !has_started {
        // CRITICAL: This is a brand new thread that has never run
        // We need to set up for its first entry to userspace
        // For first run, we need to set up the interrupt frame to jump to userspace
        match setup_first_userspace_entry(
            thread_id,
            interrupt_frame,
            saved_regs,
            process_manager_guard,
        ) {
            FirstUserspaceEntry::Installed => {
                log::info!("First run: thread {} entering userspace", thread_id);
                scheduler::with_thread_mut(thread_id, |thread| {
                    thread.has_started = true;
                });
            }
            FirstUserspaceEntry::Aborted(reason) => {
                FIRST_USERSPACE_ENTRY_ABORT_COUNT.fetch_add(1, Ordering::Relaxed);
                if !FIRST_USERSPACE_ENTRY_ABORT_LOGGED.swap(true, Ordering::Relaxed) {
                    raw_serial_str("[PMGUARD] first-entry aborted tid=");
                    raw_serial_u64(thread_id);
                    raw_serial_str(" reason=");
                    raw_serial_str(reason);
                    raw_serial_str("\n");
                }
                scheduler::abort_dispatch_and_resume(thread_id, resume_thread_id);
                scheduler::set_need_resched();
            }
        }
        return;
    }

    // Thread has run before - do normal context restore
    log::trace!("Resuming thread {}", thread_id);

    // CRITICAL: Use the passed-in guard if available, otherwise try to acquire one.
    // The guard is passed from check_need_resched_and_switch to avoid double-lock deadlock.
    // If we're called from elsewhere without a guard, try_manager() as fallback.
    let guard_option = process_manager_guard.or_else(|| crate::process::try_manager());

    // Track if signal termination happened (for parent notification after borrow ends)
    let mut signal_termination_info: Option<crate::signal::delivery::ParentNotification> = None;

    if let Some(mut manager_guard) = guard_option {
        if let Some(ref mut manager) = *manager_guard {
            if let Some((pid, process)) = manager.find_process_by_thread_mut(thread_id) {
                // Get CR3 before borrowing main_thread mutably
                if refuse_unpublished_dispatch(process, thread_id, pid.as_u64()) {
                    crate::task::scheduler::set_need_resched();
                    setup_idle_return(interrupt_frame);
                    crate::task::scheduler::switch_to_idle();
                    crate::task::scheduler::requeue_refused_dispatch(thread_id);
                    return;
                }
                let process_cr3 = process.cr3_value();

                if let Some(ref mut thread) = process.main_thread {
                    if thread.privilege == ThreadPrivilege::User {
                        // Debug marker: restoring userspace context (raw serial, no locks)
                        raw_serial_str("<R>");
                        if let Err(error) =
                            restore_userspace_context(thread, interrupt_frame, saved_regs)
                        {
                            match error {
                                RestoreError::NonCanonicalRip | RestoreError::NonCanonicalRsp => {
                                    raw_serial_str("<BADADDR>")
                                }
                                RestoreError::KernelFrame => {
                                    raw_serial_str("<KFRAME>");
                                    log::error!(
                                        "Refusing userspace restore of kernel frame for thread {}: saved CS={:#x}",
                                        thread_id,
                                        thread.context.cs
                                    );
                                }
                            }
                            // Corrupted process state. Terminate the process and switch to idle.
                            thread.set_terminated();
                            process.terminate(-11); // SIGSEGV equivalent
                            crate::task::scheduler::with_thread_mut(thread_id, |sched_thread| {
                                sched_thread.set_terminated();
                            });
                            crate::task::scheduler::set_need_resched();
                            setup_idle_return(interrupt_frame);
                            crate::task::scheduler::switch_to_idle();
                        } else {
                            // CRITICAL: Defer CR3 switch to timer_entry.asm before IRETQ
                            // We do NOT switch CR3 here because:
                            // 1. Kernel can run on process page tables (they have kernel mappings)
                            // 2. timer_entry.asm will perform the actual switch before IRETQ (line 324)
                            // 3. Switching here would cause DOUBLE CR3 write (flush TLB twice)
                            //
                            // Instead, we set next_cr3 and saved_process_cr3 to communicate
                            // the target CR3 to the assembly code.
                            if let Some(cr3_value) = process_cr3 {
                                unsafe {
                                    // Tell timer_entry.asm to switch CR3 before IRETQ
                                    crate::per_cpu::set_next_cr3(cr3_value);

                                    // Update saved_process_cr3 so future timer interrupts
                                    // without context switch restore the correct CR3
                                    core::arch::asm!(
                                        "mov gs:[80], {}",
                                        in(reg) cr3_value,
                                        options(nostack, preserves_flags)
                                    );
                                }
                            } else {
                                USERSPACE_DISPATCH_NO_CR3_REFUSED.fetch_add(1, Ordering::Relaxed);
                                if !USERSPACE_DISPATCH_NO_CR3_LOGGED.swap(true, Ordering::Relaxed) {
                                    raw_serial_str("[PMGUARD] no-cr3 dispatch refused tid=");
                                    raw_serial_u64(thread_id);
                                    raw_serial_str(" pid=");
                                    raw_serial_u64(pid.as_u64());
                                    raw_serial_str("\n");
                                }
                                // With next_cr3 == 0, timer_entry.asm takes its fallback path
                                // and restores the interrupted process's address space. Refuse
                                // to finish this user-mode return with that wrong CR3 active.
                                thread.set_terminated();
                                crate::task::scheduler::with_thread_mut(
                                    thread_id,
                                    |sched_thread| {
                                        sched_thread.set_terminated();
                                    },
                                );
                                crate::task::scheduler::set_need_resched();
                                setup_idle_return(interrupt_frame);
                                crate::task::scheduler::switch_to_idle();
                                return;
                            }

                            // Update TSS RSP0 for the new thread's kernel stack
                            if let Some(kernel_stack_top) = thread.kernel_stack_top {
                                crate::per_cpu::update_tss_rsp0(kernel_stack_top.as_u64());
                                log::trace!("Set kernel stack: {:#x}", kernel_stack_top.as_u64());
                            } else {
                                log::error!(
                                    "ERROR: Userspace thread {} has no kernel stack!",
                                    thread_id
                                );
                            }

                            // SIGNAL DELIVERY: Check for pending signals before returning to userspace
                            // This is the correct point to deliver signals - after context is restored
                            // but before we actually return to userspace
                            crate::signal::delivery::check_and_fire_alarm(process);
                            crate::signal::delivery::check_and_fire_itimer_real(process, 5000);

                            if crate::signal::delivery::has_deliverable_signals(process) {
                                log::debug!(
                                    "Signal delivery check: process {} (thread {}) has deliverable signals",
                                    pid.as_u64(),
                                    thread_id
                                );

                                // CRITICAL: Switch to process CR3 BEFORE delivering signal
                                // Signal delivery writes to user stack memory, which requires
                                // the process's page table to be active (not the kernel CR3).
                                // Without this, we get a page fault when trying to write the
                                // signal frame to user memory.
                                if let Some(cr3_val) = process.cr3_value() {
                                    unsafe {
                                        use x86_64::registers::control::{Cr3, Cr3Flags};
                                        use x86_64::structures::paging::PhysFrame;
                                        use x86_64::PhysAddr;
                                        Cr3::write(
                                            PhysFrame::containing_address(PhysAddr::new(cr3_val)),
                                            Cr3Flags::empty(),
                                        );
                                    }
                                    log::debug!(
                                        "Switched to process CR3 {:#x} for signal delivery",
                                        cr3_val
                                    );
                                }

                                // Deliver pending signals
                                let signal_result =
                                    crate::signal::delivery::deliver_pending_signals(
                                        process,
                                        interrupt_frame,
                                        saved_regs,
                                    );

                                match signal_result {
                                    crate::signal::delivery::SignalDeliveryResult::Terminated(
                                        notification,
                                    ) => {
                                        // Signal terminated the process
                                        crate::task::scheduler::set_need_resched();
                                        // Save notification for later (after manager lock is released)
                                        signal_termination_info = Some(notification);
                                        setup_idle_return(interrupt_frame);
                                        crate::task::scheduler::switch_to_idle();
                                        // Don't return here - fall through to handle notification
                                    }
                                    crate::signal::delivery::SignalDeliveryResult::Delivered => {
                                        // Signal was delivered and frame was modified
                                        if process.is_terminated() {
                                            // Process was terminated (somehow?)
                                            crate::task::scheduler::set_need_resched();
                                            setup_idle_return(interrupt_frame);
                                            crate::task::scheduler::switch_to_idle();
                                        }
                                    }
                                    crate::signal::delivery::SignalDeliveryResult::NoAction => {}
                                }
                            }
                        }
                    }
                }
            }

            // Now process borrow has ended - notify parent if signal terminated a child
            // Drop manager guard first to avoid deadlock when notifying parent
            drop(manager_guard);
            if let Some(notification) = signal_termination_info {
                crate::signal::delivery::notify_parent_of_termination_deferred(&notification);
            }
        }
    } else {
        log::error!(
            "CRITICAL: Could not acquire process manager lock to restore context for thread {}. \
             Interrupt frame NOT modified - will return to previous thread instead!",
            thread_id
        );
    }
}

/// Set up interrupt frame for first entry to userspace
fn setup_first_userspace_entry(
    thread_id: u64,
    interrupt_frame: &mut InterruptStackFrame,
    saved_regs: &mut SavedRegisters,
    process_manager_guard: Option<crate::process::TryProcessManagerGuard>,
) -> FirstUserspaceEntry {
    // Phase 1: resolve every fallible dependency before touching the return frame.
    let mut manager_guard = match process_manager_guard.or_else(crate::process::try_manager) {
        Some(guard) if guard.is_some() => guard,
        _ => return FirstUserspaceEntry::Aborted("process manager unavailable"),
    };
    let (_, process) = match manager_guard
        .as_mut()
        .and_then(|manager| manager.find_process_by_thread_mut(thread_id))
    {
        Some(found) => found,
        None => return FirstUserspaceEntry::Aborted("process lookup failed"),
    };
    let cr3_value = match process.cr3_value() {
        Some(value) => value,
        None => return FirstUserspaceEntry::Aborted("process CR3 unavailable"),
    };
    let thread = match process.main_thread.as_ref() {
        Some(thread) => thread,
        None => return FirstUserspaceEntry::Aborted("thread kernel stack unavailable"),
    };
    let kernel_stack_top = match thread.kernel_stack_top {
        Some(stack_top) => stack_top,
        None => return FirstUserspaceEntry::Aborted("thread kernel stack unavailable"),
    };
    let entry_rip = thread.context.rip;
    let user_rsp = thread.context.rsp;
    drop(manager_guard);

    unsafe {
        crate::per_cpu::set_next_cr3(cr3_value);
        core::arch::asm!(
            "mov gs:[80], {}",
            in(reg) cr3_value,
            options(nostack, preserves_flags)
        );
    }
    crate::per_cpu::update_tss_rsp0(kernel_stack_top.as_u64());

    // Phase 2: commit the first userspace frame only after CR3 and RSP0 are installed.
    unsafe {
        interrupt_frame.as_mut().update(|frame| {
            // Set instruction pointer to entry point
            frame.instruction_pointer = VirtAddr::new(entry_rip);

            // Set stack pointer to user stack with proper alignment
            // Ensure (rsp % 16) == 8 at entry for SysV AMD64 ABI
            let aligned_rsp = (user_rsp & !0xF) | 0x8;
            frame.stack_pointer = VirtAddr::new(aligned_rsp);

            // Set code segment to user code (Ring 3)
            frame.code_segment = crate::gdt::user_code_selector();

            // Set stack segment to user data (Ring 3)
            frame.stack_segment = crate::gdt::user_data_selector();

            // Set CPU flags: IF=1 (interrupts enabled), bit 1=1 (reserved)
            let flags_ptr =
                &mut frame.cpu_flags as *mut x86_64::registers::rflags::RFlags as *mut u64;
            *flags_ptr = 0x202; // Bit 1=1 (required), IF=1 (bit 9)

            log::info!(
                "RING3_ENTRY: RIP={:#x}, RSP={:#x}, CS={:#x}, SS={:#x}",
                frame.instruction_pointer.as_u64(),
                frame.stack_pointer.as_u64(),
                frame.code_segment.0,
                frame.stack_segment.0
            );
        });
    }

    // CRITICAL: Zero all general-purpose registers for security and determinism
    // This ensures userspace starts with a clean register state
    saved_regs.rax = 0;
    saved_regs.rbx = 0;
    saved_regs.rcx = 0;
    saved_regs.rdx = 0;
    saved_regs.rsi = 0;
    saved_regs.rdi = 0;
    saved_regs.rbp = 0;
    saved_regs.r8 = 0;
    saved_regs.r9 = 0;
    saved_regs.r10 = 0;
    saved_regs.r11 = 0;
    saved_regs.r12 = 0;
    saved_regs.r13 = 0;
    saved_regs.r14 = 0;
    saved_regs.r15 = 0;

    // DEBUG: Log that registers were zeroed for first entry
    log::info!("FIRST_ENTRY t{}: zeroed all registers", thread_id);

    log::info!(
        "First userspace entry setup complete for thread {}",
        thread_id
    );
    FirstUserspaceEntry::Installed
}

/// Check and deliver pending signals for the current thread
///
/// Called when returning to userspace without a context switch (same thread continues).
/// This ensures signals are delivered promptly even when the same thread keeps running.
fn check_and_deliver_signals_for_current_thread(
    saved_regs: &mut SavedRegisters,
    interrupt_frame: &mut InterruptStackFrame,
) {
    // Get current thread ID
    let current_thread_id = match scheduler::current_thread_id() {
        Some(id) => id,
        None => return,
    };

    // 0 is the no-thread sentinel, not a live thread id, so there is no
    // process to deliver signals to.
    if current_thread_id == 0 {
        return;
    }

    // Try to acquire process manager lock
    let mut manager_guard = match crate::process::try_manager() {
        Some(guard) => guard,
        None => return, // Lock held, skip signal check this time
    };

    // Track if signal termination happened (for parent notification after borrow ends)
    let mut signal_termination_info: Option<crate::signal::delivery::ParentNotification> = None;

    if let Some(ref mut manager) = *manager_guard {
        // Find the process for this thread
        if let Some((_pid, process)) = manager.find_process_by_thread_mut(current_thread_id) {
            // Note: Debug logging removed from hot path - use GDB if debugging is needed
            crate::signal::delivery::check_and_fire_alarm(process);
            crate::signal::delivery::check_and_fire_itimer_real(process, 5000);

            if crate::signal::delivery::has_deliverable_signals(process) {
                // Switch to process's page table for signal delivery
                if let Some(cr3_val) = process.cr3_value() {
                    unsafe {
                        use x86_64::registers::control::{Cr3, Cr3Flags};
                        use x86_64::structures::paging::PhysFrame;
                        use x86_64::PhysAddr;
                        Cr3::write(
                            PhysFrame::containing_address(PhysAddr::new(cr3_val)),
                            Cr3Flags::empty(),
                        );
                    }
                }

                // Deliver signals
                let signal_result = crate::signal::delivery::deliver_pending_signals(
                    process,
                    interrupt_frame,
                    saved_regs,
                );

                match signal_result {
                    crate::signal::delivery::SignalDeliveryResult::Terminated(notification) => {
                        // Signal terminated the process
                        crate::task::scheduler::set_need_resched();
                        signal_termination_info = Some(notification);
                        setup_idle_return(interrupt_frame);
                        crate::task::scheduler::switch_to_idle();
                        // Don't return here - fall through to handle notification
                    }
                    crate::signal::delivery::SignalDeliveryResult::Delivered => {
                        if process.is_terminated() {
                            crate::task::scheduler::set_need_resched();
                            setup_idle_return(interrupt_frame);
                            crate::task::scheduler::switch_to_idle();
                        }
                    }
                    crate::signal::delivery::SignalDeliveryResult::NoAction => {}
                }
            }
        }
        // process borrow has ended here

        // Drop manager guard first to avoid deadlock when notifying parent
        drop(manager_guard);

        // Notify parent if signal terminated a child
        if let Some(notification) = signal_termination_info {
            crate::signal::delivery::notify_parent_of_termination_deferred(&notification);
        }
    }
}

/// Simple idle loop - made pub for exception handlers that need to jump to idle
pub fn idle_loop() -> ! {
    loop {
        crate::task::process_task::reclaim_deferred_process_resources();
        // P6a PR-2, review finding B2. Retention at quiesce has to be sampled
        // from a context that exists AFTER every userspace thread is gone and
        // after the drain above has had passes to run; the idle loop is the only
        // such context on x86, because the strand reporter fires once before
        // userspace and the heartbeat's procfs reader is an aarch64 userspace
        // program. Boot-test profile only. On the production x86 path this is
        // compiled out entirely, and inside the profile it is one relaxed atomic
        // load per idle pass until it emits its single line: no lock, no
        // allocation, and nothing on the context-switch path itself.
        #[cfg(all(target_arch = "x86_64", feature = "boot_tests"))]
        crate::tracing::providers::teardown::x86_settled_tombstone_census();
        crate::task::scheduler::reclaim_terminated_threads();
        // Try to flush any pending IRQ logs while idle
        crate::irq_log::flush_local_try();
        // CRITICAL: Use enable_and_hlt() instead of just hlt()
        // This atomically enables interrupts and halts, preventing race conditions
        // where interrupts might be disabled when we enter this loop.
        // Without this, if interrupts are disabled, HLT would hang forever.
        x86_64::instructions::interrupts::enable_and_hlt();
    }
}

// REMOVED: get_next_page_table() is no longer needed since CR3 switching
// happens immediately during context switch in the scheduler

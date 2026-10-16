//! Context switching logic for interrupt return path
//!
//! This module handles the actual context switching when returning from
//! interrupts. It's called from assembly code after the interrupt handler
//! has completed its minimal work.

#![cfg(target_arch = "x86_64")]

use crate::task::process_context::{
    is_kernel_code_selector, restore_userspace_context, RestoreError,
    SavedRegisters,
};
use crate::task::dispatch_strand_census::{note_fact, DispatchLogFact};
use crate::task::scheduler;
use crate::task::thread::ThreadPrivilege;
use crate::tracing::providers::counters::{
    DISPATCH_GATE_PREEMPT_ACTIVE, DISPATCH_KERNEL_RESTORE_TOTAL, DISPATCH_NO_PROGRESS,
};
use crate::tracing::providers::sched::{
    trace_ctx_switch, trace_dispatch_abandon, trace_dispatch_save, DispatchAbandonSite,
    DispatchSaveReason,
};
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

/// Serial string output that allocates nothing and waits on SERIAL1 for a
/// bounded time only (`serial::write_str_bounded`), then goes to the kernel
/// log, whole, as a lock-free notice. Every caller prints once per boot. Under
/// SERIAL1, with several CPUs printing, a line is not interleaved byte by byte
/// with their output and lost from the boot stages.
#[inline(always)]
fn raw_serial_str(s: &str) {
    crate::serial::write_str_bounded(s);
}

/// One diagnostic line, built on the stack and written in a single
/// `raw_serial_str` call, so another CPU's serial output cannot land inside it.
/// No allocation; a line longer than the buffer is cut short.
struct DiagLine {
    bytes: [u8; 128],
    len: usize,
}

impl DiagLine {
    #[inline(always)]
    fn new() -> Self {
        Self {
            bytes: [0; 128],
            len: 0,
        }
    }

    #[inline(always)]
    fn text(&mut self, s: &str) -> &mut Self {
        for &byte in s.as_bytes() {
            if self.len < self.bytes.len() {
                self.bytes[self.len] = byte;
                self.len += 1;
            }
        }
        self
    }

    #[inline(always)]
    fn number(&mut self, mut value: u64) -> &mut Self {
        let mut digits = [0_u8; 20];
        let mut index = digits.len();
        loop {
            index -= 1;
            digits[index] = b'0' + (value % 10) as u8;
            value /= 10;
            if value == 0 {
                break;
            }
        }
        for &digit in &digits[index..] {
            if self.len < self.bytes.len() {
                self.bytes[self.len] = digit;
                self.len += 1;
            }
        }
        self
    }

    #[inline(always)]
    fn write(&self) {
        let bytes = &self.bytes[..self.len];
        match core::str::from_utf8(bytes) {
            Ok(line) => raw_serial_str(line),
            // Cut inside a multi-byte character: write the whole characters.
            Err(error) => {
                if let Ok(line) = core::str::from_utf8(&bytes[..error.valid_up_to()]) {
                    raw_serial_str(line);
                }
            }
        }
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
        DiagLine::new()
            .text("[PMGUARD] creating dispatch refused tid=")
            .number(thread_id)
            .text(" pid=")
            .number(pid)
            .text("\n")
            .write();
    }
    true
}

#[cfg(feature = "boot_tests")]
pub fn userspace_dispatch_creating_refused() -> u64 {
    USERSPACE_DISPATCH_CREATING_REFUSED.load(Ordering::Relaxed)
}

// REMOVED: NEXT_PAGE_TABLE is no longer needed since CR3 switching happens
// immediately during context switch, not deferred to interrupt return

/// Resumes user CPU time when `check_need_resched_and_switch` returns, for the
/// thread its frame then returns to, if that is Ring 3.
struct ReturnUserTime(*const InterruptStackFrame);

impl Drop for ReturnUserTime {
    fn drop(&mut self) {
        // SAFETY: the frame is the caller's and outlives the function this
        // guard lives in, which rewrites it in place.
        if (unsafe { &*self.0 }.code_segment.0 & 3) == 3 {
            crate::task::thread::resume_user_time();
        }
    }
}

/// Check if rescheduling is needed and perform context switch if necessary
///
/// This is called from the assembly interrupt return path and is the
/// CORRECT place to handle context switching (not in the interrupt handler).
///
/// An interrupt taken from Ring 3 is system time from here, and a return to
/// Ring 3 is user time from the end of this function, the last work before the
/// IRETQ, for whichever thread the frame now returns to: the interrupted one,
/// one this switch dispatched, or a pause or sigsuspend that `switch_to_thread`
/// resumes straight into user mode.
#[no_mangle]
pub extern "C" fn check_need_resched_and_switch(
    saved_regs: &mut SavedRegisters,
    interrupt_frame: &mut InterruptStackFrame,
) {
    if (interrupt_frame.code_segment.0 & 3) == 3 {
        crate::task::thread::enter_kernel_time();
    }
    let _user_time = ReturnUserTime(interrupt_frame as *const InterruptStackFrame);
    crate::task::scheduler::note_scheduling_epoch(crate::per_cpu::cpu_id());
    // This entry is a later interrupt than the one that last switched this
    // CPU, so the CPU is off that switch's outgoing stack; it also takes any
    // reschedule another CPU asked of it.
    crate::task::scheduler::note_x86_interrupt_return(crate::per_cpu::cpu_id());
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
        //
        // #772 diagnostics: `kernel/src/syscall/entry.asm` makes its
        // reschedule call before it sets PREEMPT_ACTIVE and masks interrupts
        // until the bit is clear again, so no call should return here.
        // Counting the exits makes that measurable rather than only readable
        // off the assembly.
        crate::trace_count!(DISPATCH_GATE_PREEMPT_ACTIVE);
        return;
    }

    // Check if current thread is blocked or terminated - we MUST switch away in that case.
    // Uses ThreadState::is_blocked() (#673 review, m4) -- this site recognizes
    // the FULL five-variant family (including BlockedOnIO), unlike
    // per_cpu::can_schedule()'s deliberately narrower four-variant set
    // (per_cpu.rs's own comment on that narrowing names the reason).
    //
    // Recognizing BlockedOnIO here is safe for a reason specific to THIS
    // site, not a blanket claim about the state itself (#673 review, MA2):
    // this check runs strictly AFTER the `preempt_count_value > 0 &&
    // !from_userspace` early return above, so by the time we reach it,
    // either preempt_count is already zero or we entered from userspace --
    // never mid-brake. Every x86 producer of BlockedOnIO (the
    // WaitQueueHead-based `prepare_to_wait(BlockedOnIO)` sites in
    // drivers/virtio/{block,block_mmio,sound,sound_mmio,gpu_pci}.rs and
    // syscall/graphics.rs, and the scheduler-internal
    // `block_current_for_io_with_timeout()` the Completion-based device
    // waits use, task/scheduler.rs) sets the state only immediately before
    // handing off to the scheduler (`schedule_current_wait()` or an inline
    // `schedule()` call), so a thread is never observably BlockedOnIO while
    // still holding a brake it needs this check to respect. That includes
    // #673's own production-init brake:
    // `Completion::wait_timeout_uninterruptible()` (task/completion.rs, used
    // by the ext2 read's `wait_for_completion()`, drivers/virtio/block.rs)
    // explicitly calls `preempt_enable()` before setting BlockedOnIO and
    // restores the brake afterward, so this site seeing BlockedOnIO here
    // never coincides with the brake being up. This half of the widening
    // ships in every x86 profile, `boot_tests`/`testing` included, not only
    // the shipped production one.
    //
    // per_cpu::can_schedule() (M5) narrows differently: it is called from
    // the timer interrupt with no equivalent preceding preempt-count gate of
    // its own, so it cannot rely on the same "never mid-brake" argument --
    // see its own comment for why BlockedOnIO stays out there (#666, #508,
    // the still-open gap between preempt_count and scheduling admission
    // during a boot-thread disk-completion wait). The two sites are not
    // inconsistent: each recognizes the family that is provably safe for
    // where it sits.
    let mut current_thread_blocked_or_terminated = scheduler::with_scheduler(|sched| {
        sched.current_thread_mut().is_some_and(|current| {
            current.state.is_blocked()
                || current.state == crate::task::thread::ThreadState::Terminated
        })
    })
    .unwrap_or(false);

    // Check if reschedule is needed
    // CRITICAL: If current thread is blocked/terminated, we MUST schedule regardless of need_resched.
    // A blocked thread cannot continue running - we must switch to another thread.
    let need_resched = scheduler::check_and_clear_need_resched();

    // A stopped process's thread is held off Ring 3 here, before the switch
    // decision, so the mandatory switch below saves its user context for
    // SIGCONT to resume. Signal delivery arms need_resched for it, so the
    // ordinary tick, with no reschedule pending, does not pay for the check.
    // A process manager busy on another CPU is retried by self-IPI, taken at
    // the first instruction boundary in Ring 3, rather than left to the
    // dispatch below: when that found the lock free a moment later it declined
    // the stop and returned the thread to user mode until the next tick, and a
    // child that sent itself SIGSTOP ran on and exited first. Only a stop in
    // force, or a stop signal pending, can hold the thread, and the thread's
    // copies of its row's signal state say whether there is one, so any
    // other switch does not wait for that lock.
    if from_userspace
        && need_resched
        && !current_thread_blocked_or_terminated
        && crate::per_cpu::current_thread().is_some_and(|thread| thread.signals.mirror.stop_possible())
    {
        if let Some(current_tid) = scheduler::current_thread_id() {
            match crate::signal::delivery::hold_stopped_thread_on_interrupt_return_or_busy(
                current_tid,
            ) {
                Some(held) => current_thread_blocked_or_terminated = held,
                None => {
                    scheduler::set_need_resched();
                    scheduler::retry_after_interrupts_x86();
                    return;
                }
            }
        }
    }
    if !need_resched && !current_thread_blocked_or_terminated {
        // No reschedule needed, but check for pending signals before returning to userspace
        if from_userspace {
            check_and_deliver_signals_for_current_thread(saved_regs, interrupt_frame);
        }
        return;
    }

    // #772 census: count a preemption entered on a frame
    // byte-identical -- RIP AND RSP -- to the one the last completed dispatch
    // installed for this same thread.
    //
    // This used to be a refusal: candidate A spent a one-shot per dispatch,
    // re-armed `need_resched` and returned to the thread. R115 removed that arm.
    // Ruling R113 (2026-09-03) retired the proxy, because
    // docs/planning/green-program/sockets/772-DIAG-2026-09-03.md symbolised the
    // two addresses this population sits on as the wait loop's own park points
    // -- so an identical frame records a thread that went once around its wait
    // loop and re-parked, not a dispatch that retired no instruction. What is
    // left is a REVISIT census: the counter still measures something real and
    // reproducible, it just does not measure the defect #772 is filed for, and
    // nothing here changes control flow. The replacement oracle is the
    // park-count split at the save site (`note_dispatch_save` below), not
    // anything in this block.
    // claim-lint:ok: 8 of 8 `return;` statements in this function match main
    // 5f81d92b, counted in this slot; the only additions are counter reads.
    //
    // The increment rule is unchanged from the build the 40-boot battery
    // measured, so its committed `DISPATCH_NO_PROGRESS` readouts stay
    // comparable with any future one.
    //
    // Cost on the interrupt-return path: 0 added instructions for an interrupt
    // that returns at gate 4 -- the common tick -- because this block sits
    // after that return. On the arm that does reach it: one lock-free
    // current-thread read plus `classify_dispatch_progress`'s four GS-relative
    // loads and one `Acquire` load, then at most one per-CPU relaxed atomic
    // add. No lock, no allocation, no formatting, no page-table work.
    //
    // Short-circuited on the blocked/terminated arm deliberately: that switch
    // is mandatory, it must not pay for the predicate -- not even the lock-free
    // tid read -- and the per-reason save census
    // (`DISPATCH_NOPROGRESS_SAVE_KERNEL_BLOCKED_MANDATORY`) already records
    // that arm's identical frames at the save site.
    if !current_thread_blocked_or_terminated {
        if let Some(current_tid) = crate::per_cpu::current_thread_id_lock_free() {
            // `matches!` rather than `==`: `NoProgress` now carries the
            // witness the save-site split consumes, and this census does not
            // want the split -- reading the kind here would put
            // `classify_no_progress_kind`'s five reads on the
            // interrupt-return path for a value this census does not record.
            if matches!(
                crate::per_cpu::classify_dispatch_progress(
                    current_tid,
                    interrupt_frame.instruction_pointer.as_u64(),
                    interrupt_frame.stack_pointer.as_u64(),
                ),
                crate::per_cpu::DispatchProgress::NoProgress(_)
            ) {
                crate::trace_count!(DISPATCH_NO_PROGRESS);
            }
        }
    }

    // Count reschedule attempts (for diagnostics if needed)
    static RESCHED_LOG_COUNTER: core::sync::atomic::AtomicU64 =
        core::sync::atomic::AtomicU64::new(0);
    let _count = RESCHED_LOG_COUNTER.fetch_add(1, core::sync::atomic::Ordering::Relaxed);

    // An idle CPU runs a full pass only when something is queued; otherwise its
    // scheduling entry (expired timers included) is all it needs.
    let idle_cpu = !from_userspace
        && !current_thread_blocked_or_terminated
        && crate::per_cpu::running_idle_thread();
    if idle_cpu && !scheduler::x86_idle_pass() {
        return;
    }

    // The switch takes no PROCESS_MANAGER. The outgoing context is saved in,
    // and the incoming one restored from, the threads' scheduler rows, and a
    // user thread's page-table root is cached there (`dispatch_root`), so a
    // CPU that stops while it holds PROCESS_MANAGER does not stop dispatch on
    // the others (#1635). That lock is asked for, without waiting on it, only
    // to fill a thread's empty root cache, once per thread, which rolls the
    // switch back when the lock is busy, and to deliver a signal.

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
            // Same thread continues running, but check for pending signals.
            if from_userspace {
                check_and_deliver_signals_for_current_thread(saved_regs, interrupt_frame);
            }
            return;
        }

        // NOTE: No logging here - log statements in the context switch path
        // cause deadlocks when the logger tries to acquire locks during a switch
        // to a newly created kthread. Use the tracing framework for debugging.

        // Note, once per boot, the first switch away from a Ring 3 frame. This
        // proves only that a CPU was running user code when it was preempted;
        // the syscall path's own marker is the syscall handler's.
        // CRITICAL: Use raw serial output without locks to prevent deadlock in IRQ context
        if from_userspace {
            static EMITTED_RING3_MARKER: core::sync::atomic::AtomicBool =
                core::sync::atomic::AtomicBool::new(false);
            if !EMITTED_RING3_MARKER.swap(true, core::sync::atomic::Ordering::Relaxed) {
                raw_serial_str("RING3_ENTER: CS=0x33\n");
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
        let (blocked_in_syscall, old_thread_is_user, old_thread_terminated) =
            scheduler::with_thread_mut(old_thread_id, |thread| {
                (
                    thread.blocked_in_syscall,
                    thread.privilege == ThreadPrivilege::User,
                    thread.state == crate::task::thread::ThreadState::Terminated,
                )
            })
            .unwrap_or((false, false, false));

        // #772 diagnostics: which gate admitted this switch. The blocked or
        // terminated arm is the mandatory switch the refusal is conjoined out
        // of; anything else got here on `need_resched`. This is the same
        // boolean the refusal itself tests, so the split is exactly the
        // refusal's visibility boundary.
        let save_mandatory = current_thread_blocked_or_terminated;

        if from_userspace {
            // A terminated thread is never dispatched again, so its context
            // needs no save. Its row may already be gone, and a save that
            // cannot find it must not hold the CPU on the dead thread.
            if !save_current_thread_context(
                old_thread_id,
                saved_regs,
                interrupt_frame,
                if save_mandatory {
                    DispatchSaveReason::UserMandatory
                } else {
                    DispatchSaveReason::UserPreempt
                },
            ) && !old_thread_terminated
            {
                // Roll back the committed switch and re-arm rescheduling.
                scheduler::abort_dispatch_and_resume(new_thread_id, old_thread_id);
                trace_dispatch_abandon(DispatchAbandonSite::RollbackSaveFailed);
                scheduler::set_need_resched();
                return;
            }
        } else if !from_userspace && (blocked_in_syscall || old_thread_is_user) {
            // A user thread was interrupted in kernel mode, including a thread blocked
            // in a syscall's HLT loop. Save the KERNEL context in its scheduler row so
            // the user-thread restore path can resume it at the correct kernel location.
            // NOTE: No logging here - this is a hot path during blocking I/O
            save_kernel_context(
                old_thread_id,
                saved_regs,
                interrupt_frame,
                if save_mandatory {
                    DispatchSaveReason::KernelBlockedMandatory
                } else {
                    DispatchSaveReason::KernelBlockedPreempt
                },
            );
        } else if !from_userspace {
            // Pure kernel thread (like kthread) being preempted - save its context
            // This is NOT a userspace thread and NOT blocked in syscall - it's a
            // kernel thread running its own code (e.g., kthread_entry -> user function)
            save_kthread_context(
                old_thread_id,
                saved_regs,
                interrupt_frame,
                if save_mandatory {
                    DispatchSaveReason::KthreadMandatory
                } else {
                    DispatchSaveReason::KthreadPreempt
                },
            );
        }

        // Switch to the new thread
        switch_to_thread(new_thread_id, old_thread_id, saved_regs, interrupt_frame);

        // #772: record the resume frame this dispatch installed.
        //
        // Taken HERE rather than beside `set_current_thread` inside
        // `switch_to_thread`: at that point the frame still carries the
        // OUTGOING thread's RIP/RSP, because the resume frame is not written
        // until one of `switch_to_thread`'s arms rewrites it further down. By
        // the time control returns here the frame is final on the arms that
        // install a resume frame and on the arms that instead redirect to idle
        // or roll the dispatch back,
        // which is why the tid comes from the per-CPU current-thread pointer
        // (whoever the CPU is actually about to run) and not from
        // `new_thread_id`. Writing it unconditionally on a completed switch is
        // also what invalidates the previous thread's mark.
        match crate::per_cpu::current_wait_loop_iters() {
            // The tid and the park count come out of one deref of the per-CPU
            // current-thread pointer, so the mark cannot pair one thread's id
            // with another thread's count.
            Some((dispatched_tid, wait_iters)) => {
                crate::per_cpu::set_dispatch_mark(
                    dispatched_tid,
                    interrupt_frame.instruction_pointer.as_u64(),
                    interrupt_frame.stack_pointer.as_u64(),
                    wait_iters,
                );
                // Counted only once the switch completed: an aborted or
                // redirected dispatch leaves another thread current.
                if dispatched_tid == new_thread_id
                    && crate::per_cpu::current_thread()
                        .is_some_and(|thread| thread.privilege != ThreadPrivilege::Kernel)
                {
                    crate::task::user_dispatch::note(crate::per_cpu::cpu_id());
                }
            }
            // No nameable current thread: invalidate rather than record a
            // mark no later check could honestly match.
            None => crate::per_cpu::clear_dispatch_mark(),
        }

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

        // Reset the timer quantum for the new thread
        super::timer::reset_quantum();
    }
}

/// #772 diagnostics: record one context save, classified against the dispatch
/// mark this CPU carries.
///
/// `thread_id` is the thread whose context is being saved, which is also the
/// thread the last completed dispatch installed the mark for, so the two ends
/// of the comparison name the same identity. `Advanced` means the thread moved
/// off the RIP/RSP it was dispatched to (or no mark applies); `NoProgress`
/// means the frame is byte-identical.
///
/// A byte-identical frame is a wait-loop revisit census, not a defect census
/// (docs/planning/green-program/sockets/772-DIAG-2026-09-03.md, and ruling
/// R113 (2026-09-03), which retired the proxy). This function no longer leaves
/// the two cases indistinguishable, which is what that ruling asked for: the
/// `NoProgress` arm mints a `DispatchNoProgressKind` from the witness
/// `classify_dispatch_progress` hands it, splitting a thread that reached a
/// counted park point again (`Revisit`) from one that reached none in
/// between (`ZeroIter`) -- "re-parked on the same halt" and "retired no
/// instructions" hold only where the mark's RIP is that wait loop's own
/// post-halt resume point, which these counters do not record -- with
/// `Unknown` for an observation neither answer fits.
// claim-lint:ok: docs/planning/green-program/sockets/772-DIAG-2026-09-03.md
///
/// Cost: four GS-relative loads and one `PER_CPU_INITIALIZED` Acquire load
/// inside `classify_dispatch_progress`; on a byte-identical frame, five more
/// reads inside `classify_no_progress_kind` (that Acquire load again, the
/// lock-free current-thread deref, the thread's id and its park count, and
/// the GS-relative stamp the count is compared against), then four per-CPU
/// atomic adds (the reason total, the identical-frame subset of that reason,
/// the kind aggregate, and the reason x kind cell); on any other frame, one
/// add. That is the accounting
/// `crate::tracing::providers::sched::trace_dispatch_save` states, and this
/// comment is kept equal to it. No lock, no allocation, no formatting. It is
/// called from the 3 arms that save a context and is absent from the gate-4
/// early return the common tick takes.
#[inline(always)]
fn note_dispatch_save(
    reason: DispatchSaveReason,
    thread_id: u64,
    interrupt_frame: &InterruptStackFrame,
) {
    let rip = interrupt_frame.instruction_pointer.as_u64();
    let no_progress = match crate::per_cpu::classify_dispatch_progress(
        thread_id,
        rip,
        interrupt_frame.stack_pointer.as_u64(),
    ) {
        // The witness the match binds is the only way to reach the split, so
        // the kind cannot be minted from a frame the mark did not match, nor
        // for a thread other than the one the mark named.
        crate::per_cpu::DispatchProgress::NoProgress(frame) => {
            Some(crate::per_cpu::classify_no_progress_kind(frame))
        }
        crate::per_cpu::DispatchProgress::Advanced => None,
    };
    trace_dispatch_save(reason, no_progress, rip);
}

/// Save a user thread's Ring 3 context into its scheduler row, which
/// `restore_userspace_thread_context` resumes it from. Returns false, having
/// saved nothing, when the scheduler has no row for the thread.
fn save_current_thread_context(
    thread_id: u64,
    saved_regs: &SavedRegisters,
    interrupt_frame: &InterruptStackFrame,
    save_reason: DispatchSaveReason,
) -> bool {
    if !save_thread_frame(thread_id, saved_regs, interrupt_frame) {
        note_fact(DispatchLogFact::SaveNoThread);
        return false;
    }
    // #772: recorded where the save actually happened, so the counter cannot
    // outrun the saves.
    note_dispatch_save(save_reason, thread_id, interrupt_frame);
    true
}

/// Save the kernel context of a user thread interrupted inside the kernel
/// (in a blocked syscall's HLT loop, or preempted in a syscall): the kernel
/// RIP and RSP, CS 0x08, so the thread resumes at that kernel location.
fn save_kernel_context(
    thread_id: u64,
    saved_regs: &SavedRegisters,
    interrupt_frame: &InterruptStackFrame,
    save_reason: DispatchSaveReason,
) {
    if save_thread_frame(thread_id, saved_regs, interrupt_frame) {
        // #772/#775: the reason counter and the fixed atomic strand ledger
        // cover this same successful kernel-context save.
        note_dispatch_save(save_reason, thread_id, interrupt_frame);
        crate::task::dispatch_strand_census::note_save(thread_id);
    }
}

/// Save kernel thread (kthread) context before switching away: a pure kernel
/// thread, which is not associated with a process and exists only in the
/// scheduler.
fn save_kthread_context(
    thread_id: u64,
    saved_regs: &SavedRegisters,
    interrupt_frame: &InterruptStackFrame,
    save_reason: DispatchSaveReason,
) {
    // #772: only a save that found its thread is counted.
    if save_thread_frame(thread_id, saved_regs, interrupt_frame) {
        note_dispatch_save(save_reason, thread_id, interrupt_frame);
    }
}

/// Copy the interrupted registers and return frame into the context of
/// `thread_id`'s scheduler row, whichever ring the frame returns to. Every
/// thread's dispatch context lives there, a user thread's included, so a
/// switch reads and writes it under the scheduler lock alone. Returns whether
/// the row was found.
fn save_thread_frame(
    thread_id: u64,
    saved_regs: &SavedRegisters,
    interrupt_frame: &InterruptStackFrame,
) -> bool {
    let saved = scheduler::with_thread_mut(thread_id, |thread| {
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

        thread.context.rip = interrupt_frame.instruction_pointer.as_u64();
        thread.context.rsp = interrupt_frame.stack_pointer.as_u64();
        thread.context.rflags = interrupt_frame.cpu_flags.bits();
        thread.context.cs = interrupt_frame.code_segment.0 as u64;
        thread.context.ss = interrupt_frame.stack_segment.0 as u64;
    })
    .is_some();

    // Hardware memory fence to ensure all context saves are visible before
    // we switch to a different thread. This is critical for TCG mode.
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
    saved
}

/// The saved CS is authoritative, and this predicate deliberately mirrors the
/// enforcement check in `restore_userspace_context`. A never-run user thread
/// carries `cs = 0x33`, while `is_kernel_code_selector` rejects `cs == 0`, so
/// this does not widen kernel-frame routing. It reads the scheduler row the
/// caller already holds, and takes no lock of its own.
fn saved_context_is_kernel_frame(thread: &crate::task::thread::Thread) -> bool {
    thread.privilege == ThreadPrivilege::User && is_kernel_code_selector(thread.context.cs)
}

/// What a switch needs to know about the thread it dispatches, read in the
/// scheduler hold that makes it this CPU's current thread.
struct Incoming {
    is_idle: bool,
    is_kernel_thread: bool,
    /// Blocked in a syscall, or saved inside the kernel: it resumes there.
    resumes_in_kernel: bool,
    /// A user thread's cached page-table root, 0 until its first dispatch.
    cached_root: u64,
    has_started: bool,
}

/// Switch to a different thread
fn switch_to_thread(
    thread_id: u64,
    resume_thread_id: u64,
    saved_regs: &mut SavedRegisters,
    interrupt_frame: &mut InterruptStackFrame,
) {
    // The x87/SSE registers follow the thread being dispatched, and the
    // per-CPU current thread and TSS.RSP0 are updated, in one scheduler hold
    // that also reads how the thread resumes.
    let incoming = scheduler::with_scheduler(|sched| {
        crate::arch_impl::x86_64::fpu::hand_over(sched, Some(resume_thread_id), thread_id);
        let is_idle = thread_id == sched.idle_thread();
        let thread = sched.get_thread_mut(thread_id);
        let incoming = thread.as_ref().map(|thread| Incoming {
            is_idle,
            is_kernel_thread: thread.privilege == ThreadPrivilege::Kernel,
            // Check if thread was blocked inside a syscall (pause/waitpid)
            // If so, we must NOT restore userspace context - the thread needs to
            // continue executing the syscall code and return through the normal path.
            // The saved CS is the authoritative record of the ring where the
            // context was captured, even if a remote waker cleared
            // blocked_in_syscall, so a saved kernel frame resumes in the kernel too.
            resumes_in_kernel: thread.blocked_in_syscall || saved_context_is_kernel_frame(thread),
            cached_root: thread.cached_ttbr0,
            has_started: thread.has_started,
        });
        if let Some(thread) = thread {
            // Update per-CPU current thread pointer
            let thread_ptr = thread as *const _ as *mut crate::task::thread::Thread;
            crate::per_cpu::set_current_thread(thread_ptr);

            // Update TSS.RSP0 with new thread's kernel stack top
            // This is critical for interrupt/exception handling
            if let Some(kernel_stack_top) = thread.kernel_stack_top {
                crate::per_cpu::update_tss_rsp0(kernel_stack_top.as_u64());
            }
        }
        incoming.unwrap_or(Incoming {
            is_idle,
            is_kernel_thread: false,
            resumes_in_kernel: false,
            cached_root: 0,
            has_started: false,
        })
    })
    .unwrap_or(Incoming {
        is_idle: false,
        is_kernel_thread: false,
        resumes_in_kernel: false,
        cached_root: 0,
        has_started: false,
    });

    // Switch TLS if needed (kernel threads don't have TLS)
    if !incoming.is_kernel_thread {
        if crate::tls::switch_tls(thread_id).is_err() {
            scheduler::abort_dispatch_and_resume(thread_id, resume_thread_id);
            trace_dispatch_abandon(DispatchAbandonSite::RollbackTls);
            scheduler::set_need_resched();
            return;
        }
    }

    if incoming.is_idle {
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
            log::trace!("Restoring idle thread's saved context");
            setup_kernel_thread_return(thread_id, saved_regs, interrupt_frame);
        } else {
            // No saved context or was in idle_loop - go to idle loop
            setup_idle_return(interrupt_frame);
        }
    } else if incoming.is_kernel_thread {
        // Set up to return to kernel thread
        setup_kernel_thread_return(thread_id, saved_regs, interrupt_frame);
    } else if incoming.resumes_in_kernel {
        resume_kernel_context(
            thread_id,
            resume_thread_id,
            incoming.cached_root,
            saved_regs,
            interrupt_frame,
        );
    } else {
        restore_userspace_thread_context(
            thread_id,
            resume_thread_id,
            incoming.cached_root,
            incoming.has_started,
            saved_regs,
            interrupt_frame,
        );
    }
}

/// Why a user thread's page-table root could not be had for its dispatch.
enum RootRefusal {
    /// Its root is not cached yet, and PROCESS_MANAGER, which the first
    /// lookup needs, is busy.
    ManagerBusy,
    /// Its process row is still being published.
    Unpublished,
    /// It has no process row, or the row has no page table.
    NoRoot,
}

/// The page-table root to dispatch user thread `thread_id` on, given `cached`,
/// the root its scheduler row caches (0 for none).
///
/// It is cached in the thread's scheduler row, so a dispatch reads it under
/// the scheduler lock alone, in the hold that makes the thread current. The cache is filled here from the process row,
/// with PROCESS_MANAGER, the first time the thread is dispatched once that row
/// is published, and exec replaces it (`ExecSchedCommit::apply`). Nothing else
/// changes a live thread's root: a CLONE_VM thread shares its creator's for
/// good, and exec is refused while another thread shares the old one.
/// Retirement treats a cached root as live until its thread has terminated
/// (`cached_root_is_live` in `process_task.rs`).
fn dispatch_root(thread_id: u64, cached: u64) -> Result<u64, RootRefusal> {
    if cached != 0 {
        return Ok(cached);
    }
    let mut manager_guard = crate::process::poll_manager().ok_or(RootRefusal::ManagerBusy)?;
    let manager = manager_guard.as_mut().ok_or(RootRefusal::ManagerBusy)?;
    let (pid, process) = manager
        .find_process_by_thread_mut(thread_id)
        .ok_or(RootRefusal::NoRoot)?;
    if refuse_unpublished_dispatch(process, thread_id, pid.as_u64()) {
        return Err(RootRefusal::Unpublished);
    }
    let Some(root) = process.cr3_value() else {
        USERSPACE_DISPATCH_NO_CR3_REFUSED.fetch_add(1, Ordering::Relaxed);
        if !USERSPACE_DISPATCH_NO_CR3_LOGGED.swap(true, Ordering::Relaxed) {
            DiagLine::new()
                .text("[PMGUARD] no-cr3 dispatch refused tid=")
                .number(thread_id)
                .text(" pid=")
                .number(pid.as_u64())
                .text("\n")
                .write();
        }
        if let Some(ref mut thread) = process.main_thread {
            thread.set_terminated();
        }
        return Err(RootRefusal::NoRoot);
    };
    scheduler::with_thread_mut(thread_id, |thread| thread.set_dispatch_root(root));
    Ok(root)
}

/// Finish a user-thread dispatch that `dispatch_root` refused. A busy lock
/// rolls the switch back and retries it as soon as interrupts are enabled; a
/// row still being published sends the CPU to idle and requeues the thread;
/// a thread with no address space is terminated and the CPU goes to idle.
/// `in_kernel` is the arm that asked: a thread resuming inside the kernel, or
/// one returning to Ring 3.
fn refuse_user_dispatch(
    thread_id: u64,
    resume_thread_id: u64,
    refusal: RootRefusal,
    in_kernel: bool,
    interrupt_frame: &mut InterruptStackFrame,
) {
    match refusal {
        RootRefusal::ManagerBusy => {
            scheduler::abort_dispatch_and_resume(thread_id, resume_thread_id);
            trace_dispatch_abandon(DispatchAbandonSite::RollbackRootLock);
            scheduler::set_need_resched();
            scheduler::retry_after_interrupts_x86();
        }
        RootRefusal::Unpublished => {
            scheduler::set_need_resched();
            setup_idle_return(interrupt_frame);
            scheduler::switch_to_idle();
            trace_dispatch_abandon(if in_kernel {
                DispatchAbandonSite::IdleUnpublishedBlocked
            } else {
                DispatchAbandonSite::IdleUnpublishedUser
            });
            scheduler::requeue_refused_dispatch(thread_id);
            unsafe {
                crate::memory::process_memory::switch_to_kernel_page_table();
            }
        }
        RootRefusal::NoRoot => {
            scheduler::with_thread_mut(thread_id, |thread| thread.set_terminated());
            scheduler::set_need_resched();
            setup_idle_return(interrupt_frame);
            scheduler::switch_to_idle();
            trace_dispatch_abandon(if in_kernel {
                DispatchAbandonSite::IdleNoCr3Blocked
            } else {
                DispatchAbandonSite::IdleNoCr3User
            });
            unsafe {
                crate::memory::process_memory::switch_to_kernel_page_table();
            }
        }
    }
}

/// Resume a user thread that was switched out inside the kernel -- blocked in
/// a syscall (pause, waitpid, a sleep) or preempted in one -- at that kernel
/// location, on its process's page tables. A signal that came while it waited
/// is not delivered here: the wait's own loop sees it, and the syscall's
/// return delivers it.
fn resume_kernel_context(
    thread_id: u64,
    resume_thread_id: u64,
    cached_root: u64,
    saved_regs: &mut SavedRegisters,
    interrupt_frame: &mut InterruptStackFrame,
) {
    let root = match dispatch_root(thread_id, cached_root) {
        Ok(root) => root,
        Err(refusal) => {
            refuse_user_dispatch(thread_id, resume_thread_id, refusal, true, interrupt_frame);
            return;
        }
    };
    let Some((context, kernel_stack_top)) = scheduler::with_thread_mut(thread_id, |thread| {
        (thread.context.clone(), thread.kernel_stack_top)
    }) else {
        refuse_user_dispatch(thread_id, resume_thread_id, RootRefusal::NoRoot, true, interrupt_frame);
        return;
    };

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

    // Restore interrupt frame with KERNEL context
    unsafe {
        interrupt_frame.as_mut().update(|frame| {
            frame.instruction_pointer = x86_64::VirtAddr::new(context.rip);
            frame.stack_pointer = x86_64::VirtAddr::new(context.rsp);
            frame.cpu_flags = x86_64::registers::rflags::RFlags::from_bits_truncate(context.rflags);
            // CRITICAL: Use kernel code segment (CS=0x08)
            frame.code_segment = crate::gdt::kernel_code_selector();
            frame.stack_segment = crate::gdt::kernel_data_selector();
        });
    }

    // #772/#775: the aggregate counter and fixed atomic strand ledger cover
    // this completed restore.
    crate::trace_count!(DISPATCH_KERNEL_RESTORE_TOTAL);
    crate::task::dispatch_strand_census::note_restore(thread_id);

    // Update TSS RSP0 for the thread's kernel stack
    if let Some(kernel_stack_top) = kernel_stack_top {
        crate::per_cpu::update_tss_rsp0(kernel_stack_top.as_u64());
    }

    // CRITICAL: Switch CR3 IMMEDIATELY when returning to kernel mode!
    // The kernel code (e.g., waitpid HLT loop) will access userspace memory
    // (like the wstatus pointer). Without switching CR3 here, we'd be using
    // the previous process's page tables and get a page fault.
    unsafe {
        use x86_64::registers::control::{Cr3, Cr3Flags};
        use x86_64::structures::paging::PhysFrame;
        use x86_64::PhysAddr;
        crate::memory::tlb::note_root_load(root);
        Cr3::write(
            PhysFrame::containing_address(PhysAddr::new(root)),
            Cr3Flags::empty(),
        );
    }

    // Set up CR3 for the process's page table
    unsafe {
        // Tell timer_entry.asm to switch CR3 before IRETQ
        crate::per_cpu::set_next_cr3(root);

        // Update saved_process_cr3 for future timer interrupts
        core::arch::asm!(
            "mov gs:[80], {}",
            in(reg) root,
            options(nostack, preserves_flags)
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
        note_fact(DispatchLogFact::IdleStackMissing);
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
        // kernel code (idle loop) with it. The idle loop leaves it before it
        // halts.

        // Idle never returns to user mode, so this CPU's user-return CR3 now
        // names nothing it will load. Left set, it would keep a dead process's
        // root looking live to the retirement proof until this CPU next ran
        // user code.
        crate::per_cpu::set_saved_process_cr3(0);

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
    // Get thread info - restore the 15 saved general-purpose registers plus the
    // frame, not just a few.
    //
    // The name is deliberately NOT read here. Cloning it allocated a `String`
    // on each kernel-thread dispatch -- a heap-allocator lock taken from
    // interrupt context, which ordinary thread context holds with interrupts
    // enabled -- for a value this function then dropped unused. `Context` is
    // plain registers, so cloning it performs no allocation.
    //
    // This function is NOT lock-free, and an earlier revision of the #791 record
    // wrongly said the dispatch path had "no lock left to spin on". The call
    // below takes SCHEDULER.lock() -- a spin::Mutex -- through
    // scheduler::with_thread_mut. What keeps that acquisition out of the #791
    // failure class is the interrupt state of its holders, not their absence:
    // with_thread_mut takes the lock inside without_interrupts, so no holder can
    // be preempted while holding it and the byte spun on here always has a
    // runnable owner. The hazard #791 hit was the opposite shape -- a mutex this
    // path reads with IF=0 that ordinary thread context holds with interrupts
    // ENABLED. Any future lock added to this path must have the masked-holder
    // property or it re-arms #791.
    // claim-lint:ok: the deadlock this hazard class produced is issue #791 and
    // docs/planning/green-program/sockets/787-REGRESSION-RCA-2026-09-04.md.
    let thread_info = scheduler::with_thread_mut(thread_id, |thread| thread.context.clone());

    if let Some(context) = thread_info {
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
        // A kernel thread never returns to user mode: retire the user-return
        // CR3 for the same reason `setup_idle_return` does.
        crate::per_cpu::set_saved_process_cr3(0);

        // Hardware memory fence to ensure all writes to interrupt frame and saved_regs
        // are visible before IRETQ reads them. This is critical for TCG mode
        // where software emulation may have different memory ordering semantics.
        // Using a full fence (mfence) rather than just compiler fence to force
        // actual CPU store completion.
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
    } else {
        note_fact(DispatchLogFact::KernelThreadInfoMissing);
    }
}

/// Restore userspace thread context
fn restore_userspace_thread_context(
    thread_id: u64,
    resume_thread_id: u64,
    cached_root: u64,
    has_started: bool,
    saved_regs: &mut SavedRegisters,
    interrupt_frame: &mut InterruptStackFrame,
) {
    let root = match dispatch_root(thread_id, cached_root) {
        Ok(root) => root,
        Err(refusal) => {
            refuse_user_dispatch(thread_id, resume_thread_id, refusal, false, interrupt_frame);
            return;
        }
    };

    // Whether this thread has ever run before
    if !has_started {
        // CRITICAL: This is a brand new thread that has never run
        // We need to set up for its first entry to userspace
        // For first run, we need to set up the interrupt frame to jump to userspace
        match setup_first_userspace_entry(thread_id, root, interrupt_frame, saved_regs) {
            FirstUserspaceEntry::Installed => {
                scheduler::with_thread_mut(thread_id, |thread| {
                    thread.has_started = true;
                });
            }
            FirstUserspaceEntry::Aborted(reason) => {
                FIRST_USERSPACE_ENTRY_ABORT_COUNT.fetch_add(1, Ordering::Relaxed);
                if !FIRST_USERSPACE_ENTRY_ABORT_LOGGED.swap(true, Ordering::Relaxed) {
                    DiagLine::new()
                        .text("[PMGUARD] first-entry aborted tid=")
                        .number(thread_id)
                        .text(" reason=")
                        .text(reason)
                        .text("\n")
                        .write();
                }
                scheduler::abort_dispatch_and_resume(thread_id, resume_thread_id);
                trace_dispatch_abandon(DispatchAbandonSite::RollbackFirstEntry);
                scheduler::set_need_resched();
            }
        }
        return;
    }

    // Thread has run before - do normal context restore, from its scheduler
    // row, with the scheduler lock alone.
    let restored = scheduler::with_thread_mut(thread_id, |thread| {
        restore_userspace_context(thread, interrupt_frame, saved_regs)
            .map(|()| (thread.kernel_stack_top, thread.needs_user_return_check()))
    });
    match restored {
        Some(Ok((kernel_stack_top, return_work))) => {
            // CRITICAL: Defer CR3 switch to timer_entry.asm before IRETQ
            // We do NOT switch CR3 here because:
            // 1. Kernel can run on process page tables (they have kernel mappings)
            // 2. timer_entry.asm will perform the actual switch before IRETQ (line 324)
            // 3. Switching here would cause DOUBLE CR3 write (flush TLB twice)
            //
            // Instead, we set next_cr3 and saved_process_cr3 to communicate
            // the target CR3 to the assembly code.
            unsafe {
                // Tell timer_entry.asm to switch CR3 before IRETQ
                crate::per_cpu::set_next_cr3(root);

                // Update saved_process_cr3 so future timer interrupts
                // without context switch restore the correct CR3
                core::arch::asm!(
                    "mov gs:[80], {}",
                    in(reg) root,
                    options(nostack, preserves_flags)
                );
            }

            // Update TSS RSP0 for the new thread's kernel stack
            if let Some(kernel_stack_top) = kernel_stack_top {
                crate::per_cpu::update_tss_rsp0(kernel_stack_top.as_u64());
            } else {
                note_fact(DispatchLogFact::UserKernelStackMissing);
            }

            // SIGNAL DELIVERY: Check for pending signals before returning to userspace
            // This is the correct point to deliver signals - after context is restored
            // but before we actually return to userspace. A thread whose
            // return-work hint is clear has nothing to deliver, and its
            // dispatch does not wait for PROCESS_MANAGER.
            if return_work {
                deliver_signals_on_user_return(
                    thread_id,
                    saved_regs,
                    interrupt_frame,
                    UserReturn::Dispatch,
                );
            }
        }
        Some(Err(error)) => {
            match error {
                RestoreError::NonCanonicalRip | RestoreError::NonCanonicalRsp => {
                    raw_serial_str("<BADADDR>")
                }
                RestoreError::KernelFrame => raw_serial_str("<KFRAME>"),
            }
            // The thread cannot return to this context: it dies of
            // SIGSEGV, through the deferred exit the user #PF and #GP
            // take, which tells its parent and retires the row. A
            // syscall that returned to a non-canonical RIP lands
            // here when its return switched threads, instead of at
            // the IRETQ #GP.
            let _ = crate::task::process_task::defer_fault_sigsegv_exit(thread_id);
            crate::task::scheduler::terminate_thread(thread_id);
            crate::task::scheduler::set_need_resched();
            setup_idle_return(interrupt_frame);
            crate::task::scheduler::switch_to_idle();
            trace_dispatch_abandon(DispatchAbandonSite::IdleRestoreError);
        }
        None => refuse_user_dispatch(
            thread_id,
            resume_thread_id,
            RootRefusal::NoRoot,
            false,
            interrupt_frame,
        ),
    }
}

/// Set up interrupt frame for first entry to userspace, on page-table root
/// `cr3_value`.
fn setup_first_userspace_entry(
    thread_id: u64,
    cr3_value: u64,
    interrupt_frame: &mut InterruptStackFrame,
    saved_regs: &mut SavedRegisters,
) -> FirstUserspaceEntry {
    // Phase 1: resolve every fallible dependency before touching the return frame.
    let Some((entry_rip, user_rsp, kernel_stack_top)) =
        scheduler::with_thread_mut(thread_id, |thread| {
            thread
                .kernel_stack_top
                .map(|stack_top| (thread.context.rip, thread.context.rsp, stack_top))
        })
        .flatten()
    else {
        return FirstUserspaceEntry::Aborted("thread kernel stack unavailable");
    };

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

            // Set stack pointer to the user stack EXACTLY as process creation
            // computed it -- do NOT realign. Breenix's userspace entry point
            // (_start, libs/libbreenix/src/runtime.rs) is reached via IRETQ
            // directly, not a `call`: there is no implicit return-address
            // push for a "(rsp % 16) == 8 at entry" C-ABI-call convention to
            // compensate for. _start's naked asm reads argc directly from
            // [rsp] (`mov rdi, rsp`) before it does anything else, and its
            // own `and rsp, -16` re-aligns ONLY for its own internal `call`
            // to runtime_entry, entirely after that read. The kernel must
            // hand user_rsp over unmodified: 16-byte aligned, pointing at
            // argc, exactly as ProcessManager::setup_argv_on_stack computed
            // it (or, for a process created with no argv, exactly as
            // create_main_thread's `stack_top - 16` computed it, which is
            // also already 16-byte aligned).
            //
            // A previous version of this line forcibly misaligned RSP by 8
            // bytes here ("Ensure (rsp % 16) == 8"), silently corrupting the
            // exact byte offset _start reads argc from for every
            // argv-carrying process. It was never visible before #713
            // because every earlier x86 process-creation path left the
            // stack region above RSP as fresh, zeroed memory with no real
            // argc/argv written there at all -- reading "argc" 8 bytes off
            // from a run of zero bytes still read as zero either way.
            // #713's spawn() is the first x86 caller to write real,
            // non-zero argc/argv data there, which is what made the
            // corruption visible: bsshd panicked on first entry with
            // "capacity overflow" in std::sys::args::unix::args(), reading
            // a garbage argc that was actually a misaligned read spanning
            // part of argc and part of argv[0]'s pointer bytes.
            frame.stack_pointer = VirtAddr::new(user_rsp);

            // Set code segment to user code (Ring 3)
            frame.code_segment = crate::gdt::user_code_selector();

            // Set stack segment to user data (Ring 3)
            frame.stack_segment = crate::gdt::user_data_selector();

            // Set CPU flags: IF=1 (interrupts enabled), bit 1=1 (reserved)
            let flags_ptr =
                &mut frame.cpu_flags as *mut x86_64::registers::rflags::RFlags as *mut u64;
            *flags_ptr = 0x202; // Bit 1=1 (required), IF=1 (bit 9)
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

    // No logging here: this runs on the interrupt-return dispatch path with
    // interrupts masked, and every log byte is a synchronous UART write. The
    // INFO lines that used to be here held each new process's first dispatch
    // for the length of about 370 bytes of COM2 output (#1048).
    FirstUserspaceEntry::Installed
}

/// Where a delivery on the way back to Ring 3 is made, which names the
/// dispatch-abandon site of a delivery that ends the thread.
#[derive(Clone, Copy, PartialEq, Eq)]
enum UserReturn {
    /// `restore_userspace_thread_context`, for the thread a switch dispatched.
    Dispatch,
    /// `check_and_deliver_signals_for_current_thread`, for a return with no
    /// switch.
    NoSwitch,
}

/// Check and deliver pending signals for the current thread
///
/// Called when returning to userspace without a context switch (same thread continues).
/// This ensures signals are delivered promptly even when the same thread keeps running.
/// A thread whose return-work hint is clear has nothing to deliver, as on the
/// syscall return, and its return does not wait for PROCESS_MANAGER.
fn check_and_deliver_signals_for_current_thread(
    saved_regs: &mut SavedRegisters,
    interrupt_frame: &mut InterruptStackFrame,
) {
    if crate::per_cpu::current_thread().is_some_and(|thread| !thread.needs_user_return_check()) {
        return;
    }

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

    deliver_signals_on_user_return(
        current_thread_id,
        saved_regs,
        interrupt_frame,
        UserReturn::NoSwitch,
    );
}

/// Deliver what is pending for user thread `thread_id`, the current thread,
/// before its interrupt return to Ring 3, rewriting the return frame for a
/// handler. A delivery that ends the thread sends this CPU to idle.
fn deliver_signals_on_user_return(
    thread_id: u64,
    saved_regs: &mut SavedRegisters,
    interrupt_frame: &mut InterruptStackFrame,
    at: UserReturn,
) {
    // A timer's signal for a user handler needs no process manager.
    if deliver_timer_signal_lockless(thread_id, saved_regs, interrupt_frame) {
        return;
    }

    // This returns to Ring 3, so a busy process manager is held on another
    // CPU. Waiting for it with interrupts masked could deadlock against a
    // holder waiting for an interrupt routed here, so it is polled for a
    // bounded time and the delivery is otherwise retried as soon as interrupts
    // are enabled again, instead of at the next tick.
    let mut manager_guard = match crate::process::poll_manager() {
        Some(guard) => guard,
        None => {
            scheduler::retry_after_interrupts_x86();
            return;
        }
    };

    // Track if signal termination happened (for parent notification after borrow ends)
    let mut signal_termination_info: Option<crate::signal::delivery::ParentNotification> = None;

    if let Some(ref mut manager) = *manager_guard {
        // Find the process for this thread
        if let Some((_pid, process, shared_table)) =
            manager.find_process_and_shared_table_by_thread_mut(thread_id)
        {
            // Note: Debug logging removed from hot path - use GDB if debugging is needed
            crate::signal::delivery::check_and_fire_alarm(process);
            crate::signal::delivery::collect_itimer_signals(process);

            // A stop in force or pending is deliverable work too: delivery
            // declines it and arms the next scheduling point, which holds the
            // thread.
            if crate::signal::delivery::needs_action_on_return_to_user(process) {
                if at == UserReturn::Dispatch {
                    note_fact(DispatchLogFact::SignalDeliverableUser);
                }

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
                        crate::memory::tlb::note_root_load(cr3_val);
                        Cr3::write(
                            PhysFrame::containing_address(PhysAddr::new(cr3_val)),
                            Cr3Flags::empty(),
                        );
                    }
                }

                // Deliver signals
                let signal_result = crate::signal::delivery::deliver_pending_signals(
                    process,
                    shared_table,
                    interrupt_frame,
                    saved_regs,
                );

                let (terminated_site, process_terminated_site) = match at {
                    UserReturn::Dispatch => (
                        DispatchAbandonSite::IdleSignalTerminatedUser,
                        DispatchAbandonSite::IdleProcessTerminatedUser,
                    ),
                    UserReturn::NoSwitch => (
                        DispatchAbandonSite::IdleSignalTerminatedOnReturn,
                        DispatchAbandonSite::IdleProcessTerminatedOnReturn,
                    ),
                };
                match signal_result {
                    crate::signal::delivery::SignalDeliveryResult::Terminated(notification) => {
                        // Signal terminated the process
                        crate::task::scheduler::set_need_resched();
                        signal_termination_info = Some(notification);
                        setup_idle_return(interrupt_frame);
                        crate::task::scheduler::switch_to_idle();
                        trace_dispatch_abandon(terminated_site);
                        // Don't return here - fall through to handle notification
                    }
                    crate::signal::delivery::SignalDeliveryResult::FrameFault
                    | crate::signal::delivery::SignalDeliveryResult::DeferredExit => {
                        // The thread is no longer runnable; the exit worker drains its exit.
                        crate::task::scheduler::set_need_resched();
                        setup_idle_return(interrupt_frame);
                        crate::task::scheduler::switch_to_idle();
                        trace_dispatch_abandon(terminated_site);
                    }
                    crate::signal::delivery::SignalDeliveryResult::Delivered => {
                        if process.is_terminated() {
                            crate::task::scheduler::set_need_resched();
                            setup_idle_return(interrupt_frame);
                            crate::task::scheduler::switch_to_idle();
                            trace_dispatch_abandon(process_terminated_site);
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

/// Deliver a due timer signal to its user handler without PROCESS_MANAGER
/// (`signal::delivery::deliver_timer_signal_lockless`), for current thread
/// `thread_id`, writing the frame through its own page tables. Returns
/// whether it did; otherwise the caller takes the process-manager path.
fn deliver_timer_signal_lockless(
    thread_id: u64,
    saved_regs: &mut SavedRegisters,
    interrupt_frame: &mut InterruptStackFrame,
) -> bool {
    let Some(thread) = crate::per_cpu::current_thread() else {
        return false;
    };
    // A CPU-limit signal or a deferred SIGKILL is the process-manager path's.
    if thread.id != thread_id
        || thread.cached_ttbr0 == 0
        || thread.kill_pending()
        || thread
            .resource_limits
            .as_ref()
            .is_some_and(|limits| limits.has_pending_cpu_signals())
    {
        return false;
    }
    let root = thread.cached_ttbr0;
    unsafe {
        use x86_64::registers::control::{Cr3, Cr3Flags};
        use x86_64::structures::paging::PhysFrame;
        use x86_64::PhysAddr;
        if Cr3::read().0.start_address().as_u64() != root & !0xfff {
            crate::memory::tlb::note_root_load(root);
            Cr3::write(
                PhysFrame::containing_address(PhysAddr::new(root)),
                Cr3Flags::empty(),
            );
        }
    }
    crate::signal::delivery::deliver_timer_signal_lockless(
        thread_id,
        &thread.signals,
        thread.signal_timers.as_deref(),
        interrupt_frame,
        saved_regs,
    )
}

/// Simple idle loop - made pub for exception handlers that need to jump to idle
pub fn idle_loop() -> ! {
    loop {
        // This loop prints no line of its own. Placement treats a CPU
        // running its idle thread as free and queues a woken thread there,
        // while a serial line is written with interrupts masked for as long as
        // the port takes to accept it: about 60 ms for the ~300-byte strand
        // census line under the x86 gate's emulation. A thread queued on an
        // idle CPU writing that line waited for it, and `kstrandd`, a kernel
        // thread a busy CPU is seen running, emits the census instead. It
        // does write out what is left in the kernel log ring (below), one
        // FIFO at a time with interrupts masked only for the port writes.
        //
        // The housekeeping below holds preemption off for as long as it runs,
        // which was up to 35 ms under the x86 gate, so placement takes this
        // CPU for a busy one meanwhile, and what was queued here regardless is
        // dispatched as soon as it ends.
        crate::task::scheduler::release_outgoing_stack_from_idle();
        crate::task::scheduler::set_idle_housekeeping(true);
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
        crate::task::scheduler::set_idle_housekeeping(false);
        // Lines logged by callers that could not wait on the UART, holding a
        // lock or with interrupts masked, are written out here, until this
        // CPU has something to run.
        crate::serial::drain_log_while(|| !crate::task::scheduler::is_need_resched());
        // Reclamation can wake a waiter. Check with interrupts masked so a
        // wakeup cannot slip between the check and the atomic enable/halt.
        // A reschedule raised while the housekeeping above could not switch
        // (it holds preemption off) is taken through the interrupt-return path
        // now, by a software interrupt to the reschedule vector. Going round
        // the housekeeping again instead kept the CPU from halting, and a tick
        // landing in the housekeeping again left the reschedule pending. A
        // self-IPI, which this used to send, took 200-500 us to arrive under
        // the x86 gate's nested virtualization, with the woken thread queued
        // here meanwhile.
        x86_64::instructions::interrupts::disable();
        if crate::task::scheduler::is_need_resched()
            || crate::task::scheduler::has_queued_work_here()
        {
            crate::task::scheduler::set_need_resched();
            // SAFETY: the reschedule vector's gate is the timer entry, which
            // saves and restores every register; it returns here with
            // interrupts masked, as they are now.
            unsafe {
                core::arch::asm!("int {vector}", vector = const super::RESCHEDULE_VECTOR);
            }
        }
        // Leave the user root this CPU last ran on before halting. A halted
        // CPU that still has it loaded is a target of every flush and release
        // of that address space, and a halted vCPU answers only once its host
        // runs it again: initiators waited up to 265 ms on the x86 gate. The
        // next dispatch of a user thread loads its own root.
        crate::memory::tlb::load_kernel_root(crate::per_cpu::cpu_id());
        // CRITICAL: Use enable_and_hlt() instead of just hlt()
        // This atomically enables interrupts and halts, preventing race conditions
        // where interrupts might be disabled when we enter this loop.
        // Without this, if interrupts are disabled, HLT would hang forever.
        x86_64::instructions::interrupts::enable_and_hlt();
    }
}

// REMOVED: get_next_page_table() is no longer needed since CR3 switching
// happens immediately during context switch in the scheduler

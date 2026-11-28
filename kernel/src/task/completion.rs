//! Completion primitive for synchronising a waiter thread with an ISR.
//!
//! One thread calls `wait_timeout()` and sleeps until an ISR or another
//! context calls `complete(token)`.
//!
//! # Race prevention
//!
//! `complete(token)` stores `done`, issues a SeqCst fence, and reads `waiter`
//! without taking a lock. A registered waiter receives the wake through its
//! preallocated node in the owner CPU's scheduler inbox. The waiter stores
//! its TID and fences before checking `done`, so the producer sees the waiter
//! or the waiter sees the receipt before it sleeps. On x86_64 a syscall
//! waiter halts in place, and the same pairing on its CPU (`halted_cpu`)
//! lets `complete()` end the halt from another CPU.
//!
//! The waiter's done-check and `block_current_for_io()` execute under a single
//! `with_current()` call. The waiter checks `done` again after that call and
//! after each wake. A waiter that sees `done` clears `waiter`, so `complete()` may
//! read 0 and publish no wake; every exit therefore goes through
//! `finish_wait_current()`, which takes back a published `BlockedOnIO` under
//! the scheduler lock. A pending wake applied after that finds the thread
//! running and does nothing.
//!
//! # Caller contract
//!
//! `wait_timeout()` MUST be called with NO locks held (no AHCI lock,
//! no DMA lock, no scheduler lock).  It releases the caller's park baseline
//! (`task::park::release_baseline`) so the scheduler can safely
//! context-switch the thread while it waits.  Re-enabling preemption with a
//! lock held would allow another thread to acquire the same lock and then
//! have the timer switch us out, causing priority inversion or deadlock.
//!
//! Who may wait is the block I/O caller rule (E2.4,
//! docs/plans/e2-sleepable-contexts.md §6): a context that may park sleeps,
//! early boot polls while it holds the early-boot I/O token, and any other
//! context gets a receipt and `Err(NOT_SLEEPABLE)`, never a poll.
//!
//! Steps 1-3 of the AHCI split command protocol guarantee this:
//!   PHASE 1 (setup)  → held briefly, released before calling wait_timeout
//!   PHASE 2 (wait)   → no lock held; wait_timeout called here
//!   PHASE 3 (finish) → re-acquired briefly for cache invalidate + copy
//!
//! # Usage
//!
//! ```rust
//! static MY_COMP: Completion = Completion::new();
//!
//! // Issuing side (kernel thread):
//! const TOKEN: u32 = 1;
//! MY_COMP.reset();
//! issue_hardware_command();
//! match MY_COMP.wait_timeout(TOKEN, 5_000_000_000) {
//!     Ok(true)  => { /* completed */ }
//!     Ok(false) => { /* timed out */ }
//!     Err(_)    => { /* EINTR — signal arrived */ }
//! }
//!
//! // ISR side (interrupt handler):
//! MY_COMP.complete(TOKEN);
//! ```

use core::sync::atomic::{fence, AtomicU32, AtomicU64, Ordering};

/// POSIX EINTR errno value.
const EINTR: i32 = 4;

#[cfg(target_arch = "aarch64")]
#[inline(always)]
fn trace_wait_timeout_stage(stage: u16) {
    if stage != 0 {
        return;
    }

    use crate::arch_impl::aarch64::percpu::Aarch64PerCpu;
    use crate::arch_impl::traits::PerCpuOps;

    let sp: u64;
    let x30: u64;
    let slot20: u64;
    unsafe {
        core::arch::asm!(
            "mov {}, sp",
            "mov {}, x30",
            out(reg) sp,
            out(reg) x30,
            options(nomem, nostack, preserves_flags)
        );
        slot20 = core::ptr::read_volatile((sp + 0x20) as *const u64);
    }
    let tid = unsafe {
        let thread_ptr = Aarch64PerCpu::current_thread_ptr() as *const crate::task::thread::Thread;
        if thread_ptr.is_null() {
            0
        } else {
            (*thread_ptr).id() as u16
        }
    };

    crate::tracing::record_event_2(
        crate::tracing::TraceEventType::WAIT_TIMEOUT_STAGE,
        stage,
        tid,
    );
    crate::tracing::record_event(
        crate::tracing::TraceEventType::WAIT_TIMEOUT_SP,
        0,
        sp as u32,
    );
    crate::tracing::record_event(
        crate::tracing::TraceEventType::WAIT_TIMEOUT_X30,
        0,
        x30 as u32,
    );
    crate::tracing::record_event(
        crate::tracing::TraceEventType::WAIT_TIMEOUT_SLOT20,
        0,
        slot20 as u32,
    );
}

/// End the current thread's wait: clear `blocked_in_syscall` and take back the
/// `BlockedOnIO` state the wait published, as Linux's `finish_wait` sets
/// `TASK_RUNNING`.
///
/// A waiter that sees `done` after publishing that state returns still running
/// with it published, and no wake need be on its way to clear it: `complete()`
/// reads `waiter` after storing `done`, and finds 0 once the waiter, having seen
/// `done`, has cleared it. Left published, the state switched the thread out at
/// its next scheduling point and kept it off every CPU until the wait's timer
/// entry expired, a whole one-second block-device slice later (#1244). A wake
/// that does arrive later finds the thread running and does nothing.
#[inline]
fn finish_wait_current() {
    crate::task::scheduler::with_current(|sched| {
        if let Some(thread) = sched.current_thread_mut() {
            thread.blocked_in_syscall = false;
            thread.uninterruptible_io = false;
            if thread.state == crate::task::thread::ThreadState::BlockedOnIO {
                thread.set_running();
                thread.wake_time_ns = None;
            }
        }
    });
}

/// The error a wait from a context that may neither park nor poll returns,
/// after its receipt (design §6, "Atomic callers"): EIO, never a silent
/// success.
pub const NOT_SLEEPABLE: i32 = 5;

#[derive(Clone, Copy, PartialEq, Eq)]
enum WaitMode {
    Interruptible,
    Uninterruptible,
    Killable,
}

impl WaitMode {
    fn signal_pending(self) -> bool {
        match self {
            WaitMode::Interruptible => crate::syscall::check_signals_for_eintr().is_some(),
            WaitMode::Uninterruptible => false,
            WaitMode::Killable => crate::syscall::fatal_signal_pending(),
        }
    }
}

/// Observational boundaries for a device's failure-only command history.
/// Park means BlockedOnIO was published; wake includes timer/spurious returns.
#[derive(Clone, Copy)]
pub(crate) enum WaitStage {
    Park,
    Wake,
    Recheck,
}

/// Completion primitive — pairs one waiter thread with one ISR.
pub struct Completion {
    /// 0 = not done, otherwise the completion token published by `complete()`.
    /// Exposed `pub(crate)` so AHCI's early-boot polling path can read it.
    pub(crate) done: AtomicU32,
    /// TID of the sleeping waiter thread. 0 means no waiter.
    waiter: AtomicU64,
    /// x86_64: one more than the CPU a syscall waiter is about to halt on in
    /// place, or 0. Its buffered wake is applied only when a CPU next
    /// schedules, and a device interrupt taken on another CPU returns without
    /// scheduling, so `complete()` interrupts the waiter's halt itself.
    #[cfg(target_arch = "x86_64")]
    halted_cpu: core::sync::atomic::AtomicUsize,
}

impl Completion {
    /// Create a new, not-yet-completed `Completion`.
    pub const fn new() -> Self {
        Self {
            done: AtomicU32::new(0),
            waiter: AtomicU64::new(0),
            #[cfg(target_arch = "x86_64")]
            halted_cpu: core::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// Reset the completion for reuse before issuing a new command.
    ///
    /// Must be called before `wait_timeout()` when reusing a `Completion`
    /// across multiple commands (e.g., the per-port static).
    pub fn reset(&self) {
        self.done.store(0, Ordering::Release);
        self.waiter.store(0, Ordering::Release);
    }

    /// Failure-only receipt sampled by the ARM block-read diagnostic.
    #[cfg(target_arch = "aarch64")]
    pub fn completed_token(&self) -> u32 {
        self.done.load(Ordering::Acquire)
    }

    pub fn waiter_tid(&self) -> u64 {
        self.waiter.load(Ordering::Acquire)
    }

    /// Wait for completion with a wall-clock timeout.
    ///
    /// Sleeps the current thread until:
    /// - `complete(expected_token)` is called  → returns `Ok(true)`
    /// - `timeout_ns` elapses   → returns `Ok(false)`
    /// - A signal arrives        → returns `Err(EINTR)`
    ///
    /// `timeout_ns` is a duration in nanoseconds (not an absolute deadline).
    /// Internally converted to a CNTPCT_EL0 deadline on AArch64.
    ///
    /// Early boot, before the scheduler dispatches, polls instead; a context
    /// that may not park returns `Err(NOT_SLEEPABLE)` after its receipt.
    pub fn wait_timeout(&self, expected_token: u32, timeout_ns: u64) -> Result<bool, i32> {
        self.wait_timeout_inner(expected_token, timeout_ns, WaitMode::Interruptible)
    }

    /// Wait for completion without returning early for pending signals.
    ///
    /// Hardware commands that have already been issued must use this form when
    /// abandoning the wait would leave shared device slots or DMA buffers live.
    pub fn wait_timeout_uninterruptible(
        &self,
        expected_token: u32,
        timeout_ns: u64,
    ) -> Result<bool, i32> {
        self.wait_timeout_inner(expected_token, timeout_ns, WaitMode::Uninterruptible)
    }

    /// The AHCI recorder observes wait boundaries without supplying a decision.
    pub(crate) fn wait_timeout_uninterruptible_observed(
        &self,
        expected_token: u32,
        timeout_ns: u64,
        observe: impl Fn(WaitStage),
    ) -> Result<bool, i32> {
        self.wait_timeout_inner_observed(expected_token, timeout_ns, WaitMode::Uninterruptible, observe)
    }

    /// Wait for completion, returning early only for a pending SIGKILL.
    pub fn wait_timeout_killable(
        &self,
        expected_token: u32,
        timeout_ns: u64,
    ) -> Result<bool, i32> {
        self.wait_timeout_inner(expected_token, timeout_ns, WaitMode::Killable)
    }

    fn wait_timeout_inner(
        &self,
        expected_token: u32,
        timeout_ns: u64,
        wait_mode: WaitMode,
    ) -> Result<bool, i32> {
        self.wait_timeout_inner_observed(expected_token, timeout_ns, wait_mode, |_| {})
    }

    fn wait_timeout_inner_observed(
        &self,
        expected_token: u32,
        timeout_ns: u64,
        wait_mode: WaitMode,
        observe: impl Fn(WaitStage),
    ) -> Result<bool, i32> {
        use crate::task::park::{IoContext, ParkContext};
        observe(WaitStage::Recheck);
        // Fast path: already done (e.g., very fast device, or spurious call).
        // Nothing waits, so no context is refused for it.
        if self.done.load(Ordering::Acquire) == expected_token {
            if crate::task::park::context() == ParkContext::Sleepable {
                finish_wait_current();
            }
            return Ok(true);
        }

        // The caller rule decides the wait (design §6): a context that may
        // park sleeps, early boot polls while it holds the token, and any
        // other context is refused, with a receipt, rather than polled.
        match crate::task::park::io_context() {
            Ok(IoContext::Sleepable) => self.sleep(expected_token, timeout_ns, wait_mode, observe),
            Ok(IoContext::EarlyBoot) => Ok(self.poll_early_boot(expected_token, timeout_ns)),
            Err(_) => Err(NOT_SLEEPABLE),
        }
    }

    /// The sleep, for a context that may park: publish this thread as the
    /// waiter, block it for I/O with the deadline under the scheduler lock,
    /// release its baseline and switch, until `complete()` wakes it, the
    /// deadline passes or a signal the mode admits arrives.
    ///
    /// PRECONDITION: no locks are held (the AHCI split protocol in
    /// ahci/mod.rs releases all locks before calling wait_timeout).
    ///
    /// Race safety: complete(token) publishes done without the scheduler lock
    /// and publishes a wake only if it reads a nonzero waiter. If done lands
    /// after the locked done-check, the unlocked recheck sees it; the waiter
    /// is then cleared, so the ISR may publish no wake, and
    /// finish_wait_current() returns the still-published BlockedOnIO to
    /// Running before the wait returns. Otherwise the ISR read our tid and its
    /// pending wake ends the halt or the switch.
    fn sleep(
        &self,
        expected_token: u32,
        timeout_ns: u64,
        wait_mode: WaitMode,
        observe: impl Fn(WaitStage),
    ) -> Result<bool, i32> {
        let Some(tid) = crate::task::scheduler::current_thread_id() else {
            return Err(NOT_SLEEPABLE);
        };
        // Store TID so complete() can wake us.
        self.waiter.store(tid, Ordering::Release);
        // SeqCst fence: ensure the waiter store is visible to any
        // concurrent complete() before we re-check done.
        fence(Ordering::SeqCst);
        let deadline_ns = {
            let (secs, nanos) = crate::time::get_monotonic_time_ns();
            (secs as u64 * 1_000_000_000 + nanos as u64).saturating_add(timeout_ns)
        };

        // Fast check after waiter store — ISR may have fired already.
        if self.done.load(Ordering::Acquire) == expected_token {
            self.waiter.store(0, Ordering::Release);
            finish_wait_current();
            return Ok(true);
        }

        if wait_mode.signal_pending() {
            self.waiter.store(0, Ordering::Release);
            finish_wait_current();
            return Err(EINTR);
        }

        // Release the baseline so the scheduler can switch us out: the
        // syscall brake, a `park::Braked` level, or none for a kernel thread
        // at baseline 0. The caller holds no locks at this point.
        let released = crate::task::park::release_baseline();

        loop {
            if wait_mode.signal_pending() {
                self.waiter.store(0, Ordering::Release);
                finish_wait_current();
                crate::task::park::retake_baseline(released);
                return Err(EINTR);
            }

            // Atomic check-and-block: under the scheduler lock, either the
            // ISR already set done=expected_token (don't block) or we call
            // block_current_for_io() (sets state=BlockedOnIO).
            let already_done = crate::task::scheduler::with_current(|sched| {
                if self.done.load(Ordering::Acquire) == expected_token {
                    true
                } else {
                    if let Some(thread) = sched.current_thread_mut() {
                        thread.uninterruptible_io = wait_mode == WaitMode::Uninterruptible;
                    }
                    sched.block_current_for_io_with_timeout(Some(deadline_ns));
                    observe(WaitStage::Park);
                    false
                }
            })
            .unwrap_or(false); // None = scheduler gone (shouldn't happen)

            observe(WaitStage::Recheck);
            if already_done || self.done.load(Ordering::Acquire) == expected_token {
                self.waiter.store(0, Ordering::Release);
                finish_wait_current();
                crate::task::park::retake_baseline(released);
                return Ok(true);
            }

            // Switch away until the ISR's complete() -> unblock_for_io()
            // wakes us or the timed BlockedOnIO wait expires.
            #[cfg(target_arch = "aarch64")]
            trace_wait_timeout_stage(0);

            #[cfg(target_arch = "aarch64")]
            {
                crate::arch_impl::aarch64::context_switch::run_deferred_reclamation();
                crate::arch_impl::aarch64::context_switch::schedule_from_kernel();
            }
            // Published before the done check that precedes the halt, so a
            // completion is either seen by that check or finds this CPU to
            // interrupt.
            #[cfg(not(target_arch = "aarch64"))]
            {
                self.halted_cpu
                    .store(crate::per_cpu::cpu_id() + 1, Ordering::Release);
                fence(Ordering::SeqCst);
                if self.done.load(Ordering::Acquire) != expected_token {
                    crate::task::waitqueue::halt_blocked_current();
                }
                self.halted_cpu.store(0, Ordering::Release);
            }

            #[cfg(target_arch = "aarch64")]
            trace_wait_timeout_stage(1);

            observe(WaitStage::Wake);
            // Clear BlockedOnIO state after wake (could be timer or ISR).
            finish_wait_current();

            #[cfg(target_arch = "aarch64")]
            trace_wait_timeout_stage(2);

            // Check done after wake.
            observe(WaitStage::Recheck);
            if self.done.load(Ordering::Acquire) == expected_token {
                self.waiter.store(0, Ordering::Release);
                crate::task::park::retake_baseline(released);
                return Ok(true);
            }

            if wait_mode.signal_pending() {
                self.waiter.store(0, Ordering::Release);
                crate::task::park::retake_baseline(released);
                return Err(EINTR);
            }

            #[cfg(target_arch = "aarch64")]
            trace_wait_timeout_stage(3);

            // Wall-clock timeout.
            #[cfg(target_arch = "aarch64")]
            trace_wait_timeout_stage(4);
            let (secs, nanos) = crate::time::get_monotonic_time_ns();
            #[cfg(target_arch = "aarch64")]
            trace_wait_timeout_stage(5);
            if secs as u64 * 1_000_000_000 + nanos as u64 >= deadline_ns {
                self.waiter.store(0, Ordering::Release);
                crate::task::park::retake_baseline(released);
                return Ok(false);
            }
        }
    }

    /// Early boot, with the I/O token held: the boot thread is the only
    /// thread and the scheduler does not dispatch, so it polls the token
    /// against a deadline. Bounded fallback for the pre-dispatch boot phase,
    /// not event polling: allowlisted per docs/polling-allowlist.md.
    fn poll_early_boot(&self, expected_token: u32, timeout_ns: u64) -> bool {
        #[cfg(target_arch = "aarch64")]
        {
            let cnt: u64;
            let freq: u64;
            unsafe {
                core::arch::asm!("mrs {}, cntpct_el0", out(reg) cnt, options(nomem, nostack));
                core::arch::asm!("mrs {}, cntfrq_el0", out(reg) freq, options(nomem, nostack));
            }
            let ticks = (timeout_ns as u128 * freq as u128 / 1_000_000_000) as u64;
            let deadline = cnt.wrapping_add(ticks);
            loop {
                if self.done.load(Ordering::Acquire) == expected_token {
                    return true;
                }
                let now: u64;
                unsafe {
                    core::arch::asm!("mrs {}, cntpct_el0", out(reg) now, options(nomem, nostack));
                }
                if now >= deadline {
                    return false;
                }
                // `yield` not `wfe` — Parallels does not reliably wake
                // WFE when a wired SPI becomes pending without a timer.
                unsafe {
                    core::arch::asm!("yield", options(nomem, nostack));
                }
            }
        }
        #[cfg(not(target_arch = "aarch64"))]
        {
            // x86_64 early boot: plain spin with a rough cycle budget.
            let limit = timeout_ns / 1000;
            for _ in 0..limit {
                if self.done.load(Ordering::Acquire) == expected_token {
                    return true;
                }
                core::hint::spin_loop();
            }
            self.done.load(Ordering::Acquire) == expected_token
        }
    }

    /// Signal completion from ISR or any other context.
    ///
    /// Sets `done = token`, then wakes the waiter thread (if any) via the
    /// waiter's task-owned wake node. The waiter can observe the receipt
    /// before it blocks, or its CPU will apply the published node under the
    /// same scheduler lock used by the check-and-block path.
    ///
    /// Idempotent: calling `complete()` multiple times is safe (the second
    /// call stores the same token again and a waiter of 0, which is a no-op).
    pub fn complete(&self, token: u32) {
        // Store done before we try to wake the waiter.
        self.done.store(token, Ordering::Release);
        // Fence: ensure done is visible on all CPUs before we read waiter.
        fence(Ordering::SeqCst);

        // SEV: set the ARM64 global event register so that any thread spinning
        // with WFE (e.g., the boot-thread spin path) wakes immediately even if
        // the interrupt fires before WFE is executed (race-free wakeup).
        // This is harmless on x86_64 (compiled away).
        #[cfg(target_arch = "aarch64")]
        unsafe {
            core::arch::asm!("sev", options(nomem, nostack));
        }

        let tid = self.waiter.load(Ordering::Acquire);
        if tid != 0 {
            crate::task::scheduler::isr_unblock_for_io(tid);
        }

        #[cfg(target_arch = "x86_64")]
        if let Some(cpu) = self.halted_cpu.load(Ordering::Acquire).checked_sub(1) {
            crate::task::scheduler::kick_cpu_x86(cpu);
        }
    }
}

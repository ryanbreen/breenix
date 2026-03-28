//! Completion primitive for synchronising a waiter thread with an ISR.
//!
//! `Completion` is the kernel equivalent of Linux's `struct completion`.
//! A single thread calls `wait_timeout()`, which sleeps until an ISR
//! (or any other context) calls `complete(token)`.
//!
//! # Race prevention
//!
//! The done-check and `block_current_for_io()` execute under a single
//! `with_scheduler()` call, matching Linux's `raw_spin_lock_irq` around
//! `__prepare_to_swait`.  The ISR calls `complete(token)` which itself acquires
//! the scheduler lock via `with_scheduler()`.  Because `with_scheduler()`
//! disables interrupts before locking, and the ISR runs with interrupts
//! already masked by hardware, there is no deadlock risk.
//!
//! # Caller contract
//!
//! `wait_timeout()` MUST be called with NO locks held (no AHCI lock,
//! no DMA lock, no scheduler lock).  It calls `preempt_enable()` when
//! in syscall context so the scheduler can safely context-switch the thread
//! while it waits.  Re-enabling preemption with a lock held would allow
//! another thread to acquire the same lock and then have the timer switch
//! us out, causing priority inversion or deadlock.
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

#[cfg(not(target_arch = "aarch64"))]
use crate::arch_impl::traits::CpuOps;
#[cfg(not(target_arch = "aarch64"))]
type Cpu = crate::arch_impl::x86_64::cpu::X86Cpu;

/// POSIX EINTR errno value.
const EINTR: i32 = 4;

#[inline]
fn clear_blocked_in_syscall_current() {
    crate::task::scheduler::with_scheduler(|sched| {
        if let Some(thread) = sched.current_thread_mut() {
            thread.blocked_in_syscall = false;
        }
    });
}

#[inline]
fn restore_syscall_preempt_state() {
    #[cfg(target_arch = "aarch64")]
    crate::per_cpu_aarch64::preempt_disable();
    #[cfg(not(target_arch = "aarch64"))]
    crate::per_cpu::preempt_disable();
}

/// Completion primitive — pairs one waiter thread with one ISR.
pub struct Completion {
    /// 0 = not done, otherwise the completion token published by `complete()`.
    /// Exposed `pub(crate)` so AHCI's intermediate polling path (IRQ registered
    /// but scheduler not yet running) can poll it directly without sleeping.
    pub(crate) done: AtomicU32,
    /// TID of the sleeping waiter thread. 0 means no waiter.
    waiter: AtomicU64,
}

impl Completion {
    /// Create a new, not-yet-completed `Completion`.
    pub const fn new() -> Self {
        Self {
            done: AtomicU32::new(0),
            waiter: AtomicU64::new(0),
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
    /// Falls back to spin-polling when the scheduler is not yet running
    /// (detected by `with_scheduler` returning None).
    pub fn wait_timeout(&self, expected_token: u32, timeout_ns: u64) -> Result<bool, i32> {
        // Fast path: already done (e.g., very fast device, or spurious call).
        if self.done.load(Ordering::Acquire) == expected_token {
            let in_syscall = {
                #[cfg(target_arch = "aarch64")]
                {
                    crate::per_cpu_aarch64::preempt_count() > 0
                }
                #[cfg(not(target_arch = "aarch64"))]
                {
                    crate::per_cpu::preempt_count() > 0
                }
            };
            if in_syscall {
                clear_blocked_in_syscall_current();
            }
            return Ok(true);
        }

        // Obtain current thread ID. If no scheduler exists yet (early boot),
        // fall through to the polling path below.
        let tid = crate::task::scheduler::current_thread_id();

        if let Some(tid) = tid {
            // Store TID so complete() can wake us.
            self.waiter.store(tid, Ordering::Release);
            // SeqCst fence: ensure the waiter store is visible to any
            // concurrent complete() before we re-check done.
            fence(Ordering::SeqCst);

            // Compute deadline using the ARM64 free-running counter.
            let deadline_cntpct = {
                #[cfg(target_arch = "aarch64")]
                {
                    let cnt: u64;
                    let freq: u64;
                    unsafe {
                        core::arch::asm!("mrs {}, cntpct_el0", out(reg) cnt, options(nomem, nostack));
                        core::arch::asm!("mrs {}, cntfrq_el0", out(reg) freq, options(nomem, nostack));
                    }
                    // Convert ns timeout to counter ticks.
                    // freq is in Hz; timeout_ns / 1e9 * freq = timeout_ns * freq / 1e9.
                    let ticks = (timeout_ns as u128 * freq as u128 / 1_000_000_000) as u64;
                    cnt.wrapping_add(ticks)
                }
                #[cfg(not(target_arch = "aarch64"))]
                {
                    // On x86_64 AHCI is not the primary driver; use a large
                    // sentinel so the loop below is effectively unbounded.
                    u64::MAX
                }
            };
            let deadline_ns = {
                #[cfg(target_arch = "aarch64")]
                {
                    let (secs, nanos) = crate::time::get_monotonic_time_ns();
                    let now_ns = secs as u64 * 1_000_000_000 + nanos as u64;
                    now_ns.saturating_add(timeout_ns)
                }
                #[cfg(not(target_arch = "aarch64"))]
                {
                    u64::MAX
                }
            };

            // Detect whether we are in syscall context (preempt_count > 0).
            //
            // Syscall handlers call preempt_disable() on entry, so
            // preempt_count ≥ 1 when called from userspace-initiated I/O.
            // The boot thread runs with preempt_count = 0 — it must NOT
            // underflow the counter and must NOT use the WFI/sleep path
            // (there is no timer to rescue a stuck WFI before timer init).
            let in_syscall = {
                #[cfg(target_arch = "aarch64")]
                {
                    crate::per_cpu_aarch64::preempt_count() > 0
                }
                #[cfg(not(target_arch = "aarch64"))]
                {
                    crate::per_cpu::preempt_count() > 0
                }
            };

            if in_syscall {
                // ============================================================
                // SYSCALL SLEEP PATH — true block_current_for_io
                //
                // PRECONDITION: no locks are held (the AHCI split protocol in
                // ahci/mod.rs releases all locks before calling wait_timeout).
                //
                // 1. preempt_enable() — allows the scheduler to switch us out.
                // 2. Atomic check-and-block under the scheduler lock: if done
                //    already, skip the block; otherwise set BlockedOnIO.
                // 3. Inline schedule() — switch fully off CPU until either the
                //    ISR unblocks us or the timed BlockedOnIO wait expires.
                // 4. Clear blocked state, check done, check timeout.
                //
                // Race safety: the scheduler lock serialises our done-check and
                // block_current_for_io() against complete(token)/unblock_for_io().
                // If the ISR fires between our done-check and block_current_for_io,
                // with_scheduler sees the ISR cleared the state already and the
                // next iteration detects done=expected_token.
                // ============================================================

                // Fast check after waiter store — ISR may have fired already.
                if self.done.load(Ordering::Acquire) == expected_token {
                    self.waiter.store(0, Ordering::Release);
                    clear_blocked_in_syscall_current();
                    return Ok(true);
                }

                if crate::syscall::check_signals_for_eintr().is_some() {
                    self.waiter.store(0, Ordering::Release);
                    clear_blocked_in_syscall_current();
                    return Err(EINTR);
                }

                // Enable preemption so the scheduler can switch us out.
                // SAFETY: caller guarantees no locks are held at this point.
                #[cfg(target_arch = "aarch64")]
                crate::per_cpu_aarch64::preempt_enable();
                #[cfg(not(target_arch = "aarch64"))]
                crate::per_cpu::preempt_enable();

                loop {
                    if crate::syscall::check_signals_for_eintr().is_some() {
                        self.waiter.store(0, Ordering::Release);
                        clear_blocked_in_syscall_current();
                        restore_syscall_preempt_state();
                        return Err(EINTR);
                    }

                    // Atomic check-and-block: under the scheduler lock, either
                    // the ISR already set done=expected_token (don't block)
                    // or we call
                    // block_current_for_io() (sets state=BlockedOnIO).
                    let already_done = crate::task::scheduler::with_scheduler(|sched| {
                        if self.done.load(Ordering::Acquire) == expected_token {
                            true
                        } else {
                            sched.block_current_for_io_with_timeout(Some(deadline_ns));
                            false
                        }
                    })
                    .unwrap_or(false); // None = scheduler gone (shouldn't happen)

                    if already_done || self.done.load(Ordering::Acquire) == expected_token {
                        self.waiter.store(0, Ordering::Release);
                        clear_blocked_in_syscall_current();
                        // Restore preempt_count to the value expected by the
                        // syscall exit path (preempt_disable was called on entry).
                        restore_syscall_preempt_state();
                        return Ok(true);
                    }

                    // Yield CPU via schedule (Linux-equivalent wait_for_completion).
                    // schedule_from_kernel() blocks this thread (BlockedOnIO) and
                    // switches to another. The AHCI ISR calls complete() →
                    // unblock_for_io() to wake us. After re-dispatch, ensure IRQs
                    // are enabled and give the CPU a window to take any pending
                    // interrupt before re-entering the scheduler.
                    #[cfg(target_arch = "aarch64")]
                    crate::arch_impl::aarch64::context_switch::schedule_from_kernel();
                    #[cfg(not(target_arch = "aarch64"))]
                    Cpu::halt_with_interrupts();

                    // Clear BlockedOnIO state after wake (could be timer or ISR).
                    clear_blocked_in_syscall_current();

                    // Check done after wake.
                    if self.done.load(Ordering::Acquire) == expected_token {
                        self.waiter.store(0, Ordering::Release);
                        restore_syscall_preempt_state();
                        return Ok(true);
                    }

                    if crate::syscall::check_signals_for_eintr().is_some() {
                        self.waiter.store(0, Ordering::Release);
                        restore_syscall_preempt_state();
                        return Err(EINTR);
                    }

                    // Wall-clock timeout.
                    #[cfg(target_arch = "aarch64")]
                    {
                        let (secs, nanos) = crate::time::get_monotonic_time_ns();
                        let now_ns = secs as u64 * 1_000_000_000 + nanos as u64;
                        if now_ns >= deadline_ns {
                            self.waiter.store(0, Ordering::Release);
                            restore_syscall_preempt_state();
                            return Ok(false);
                        }
                    }
                    #[cfg(not(target_arch = "aarch64"))]
                    {
                        let _ = deadline_cntpct;
                    }
                }
            } else {
                // ============================================================
                // BOOT-THREAD SPIN PATH
                //
                // Scheduler is running but we are NOT in syscall context
                // (preempt_count = 0), meaning this is the raw boot thread
                // between scheduler init and timer init.  WFI is unsafe here
                // because the timer has not been started yet and may never
                // fire to rescue a stuck WFI.  Instead, spin with yield:
                // complete() emits SEV which wakes WFE race-free.
                // ============================================================
                loop {
                    if self.done.load(Ordering::Acquire) == expected_token {
                        self.waiter.store(0, Ordering::Release);
                        return Ok(true);
                    }

                    #[cfg(target_arch = "aarch64")]
                    {
                        let now: u64;
                        unsafe {
                            core::arch::asm!("mrs {}, cntpct_el0", out(reg) now, options(nomem, nostack));
                        }
                        if now >= deadline_cntpct {
                            self.waiter.store(0, Ordering::Release);
                            return Ok(false);
                        }
                        // `yield` hints the hypervisor to schedule other vCPUs
                        // without halting the CPU.  We do NOT use WFE/WFI here
                        // because Parallels does not reliably wake a halted vCPU
                        // when a wired SPI becomes pending without a timer.
                        unsafe {
                            core::arch::asm!("yield", options(nomem, nostack));
                        }
                    }
                    #[cfg(not(target_arch = "aarch64"))]
                    {
                        let _ = deadline_cntpct;
                        core::hint::spin_loop();
                    }
                }
            }
        } else {
            // Early boot polling path — no scheduler, spin on done flag.
            // Uses the same CNTPCT deadline as the interrupt path.
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
                        return Ok(true);
                    }
                    let now: u64;
                    unsafe {
                        core::arch::asm!("mrs {}, cntpct_el0", out(reg) now, options(nomem, nostack));
                    }
                    if now >= deadline {
                        return Ok(false);
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
                // x86_64 early boot: plain spin (AHCI not primary driver here).
                let limit = timeout_ns / 1000; // rough cycle budget
                for _ in 0..limit {
                    if self.done.load(Ordering::Acquire) == expected_token {
                        return Ok(true);
                    }
                    core::hint::spin_loop();
                }
                Ok(self.done.load(Ordering::Acquire) == expected_token)
            }
        }
    }

    /// Signal completion from ISR or any other context.
    ///
    /// Sets `done = token`, then wakes the waiter thread (if any) by calling
    /// `unblock_for_io()` under the scheduler lock.  This is safe from ISR
    /// context because `with_scheduler()` disables interrupts before locking,
    /// and the ISR already runs with IRQs masked.
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
            crate::task::scheduler::with_scheduler(|sched| {
                sched.unblock_for_io(tid);
            });
        }
    }
}

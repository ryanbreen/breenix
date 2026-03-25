//! Completion primitive for synchronising a waiter thread with an ISR.
//!
//! `Completion` is the kernel equivalent of Linux's `struct completion`.
//! A single thread calls `wait_timeout()`, which sleeps until an ISR
//! (or any other context) calls `complete()`.
//!
//! # Race prevention
//!
//! The done-check and `block_current_for_io()` execute under a single
//! `with_scheduler()` call, matching Linux's `raw_spin_lock_irq` around
//! `__prepare_to_swait`.  The ISR calls `complete()` which itself acquires
//! the scheduler lock via `with_scheduler()`.  Because `with_scheduler()`
//! disables interrupts before locking, and the ISR runs with interrupts
//! already masked by hardware, there is no deadlock risk.
//!
//! # Usage
//!
//! ```rust
//! static MY_COMP: Completion = Completion::new();
//!
//! // Issuing side (kernel thread):
//! MY_COMP.reset();
//! issue_hardware_command();
//! match MY_COMP.wait_timeout(5_000_000_000) {
//!     Ok(true)  => { /* completed */ }
//!     Ok(false) => { /* timed out */ }
//!     Err(_)    => { /* EINTR — signal arrived */ }
//! }
//!
//! // ISR side (interrupt handler):
//! MY_COMP.complete();
//! ```

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering, fence};

#[cfg(target_arch = "aarch64")]
use crate::arch_impl::traits::CpuOps;
#[cfg(target_arch = "aarch64")]
type Cpu = crate::arch_impl::aarch64::Aarch64Cpu;

/// POSIX EINTR errno value.
const EINTR: i32 = 4;

/// Completion primitive — pairs one waiter thread with one ISR.
pub struct Completion {
    /// Set to `true` by `complete()`, read by `wait_timeout()`.
    /// Exposed `pub(crate)` so AHCI's intermediate polling path (IRQ registered
    /// but scheduler not yet running) can poll it directly without sleeping.
    pub(crate) done: AtomicBool,
    /// TID of the sleeping waiter thread. 0 means no waiter.
    waiter: AtomicU64,
}

impl Completion {
    /// Create a new, not-yet-completed `Completion`.
    pub const fn new() -> Self {
        Self {
            done: AtomicBool::new(false),
            waiter: AtomicU64::new(0),
        }
    }

    /// Reset the completion for reuse before issuing a new command.
    ///
    /// Must be called before `wait_timeout()` when reusing a `Completion`
    /// across multiple commands (e.g., the per-port static).
    pub fn reset(&self) {
        self.done.store(false, Ordering::Release);
        self.waiter.store(0, Ordering::Release);
    }

    /// Wait for completion with a wall-clock timeout.
    ///
    /// Sleeps the current thread until:
    /// - `complete()` is called  → returns `Ok(true)`
    /// - `timeout_ns` elapses   → returns `Ok(false)`
    /// - A signal arrives        → returns `Err(EINTR)`
    ///
    /// `timeout_ns` is a duration in nanoseconds (not an absolute deadline).
    /// Internally converted to a CNTPCT_EL0 deadline on AArch64.
    ///
    /// Falls back to spin-polling when the scheduler is not yet running
    /// (detected by `with_scheduler` returning None).
    pub fn wait_timeout(&self, timeout_ns: u64) -> Result<bool, i32> {
        // Fast path: already done (e.g., very fast device, or spurious call).
        if self.done.load(Ordering::Acquire) {
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

            // Detect whether we are in syscall context (preempt_count > 0).
            //
            // Syscall handlers call preempt_disable() on entry, so
            // preempt_count ≥ 1 when called from userspace-initiated I/O.
            // The boot thread runs with preempt_count = 0 — it must NOT
            // underflow the counter and must NOT use the WFI scheduler-sleep
            // path (there is no timer to rescue a stuck WFI before the timer
            // is initialised).
            let in_syscall = {
                #[cfg(target_arch = "aarch64")]
                { crate::per_cpu_aarch64::preempt_count() > 0 }
                #[cfg(not(target_arch = "aarch64"))]
                { crate::per_cpu::preempt_count() > 0 }
            };

            if in_syscall {
                // ============================================================
                // SYSCALL SLEEP PATH
                //
                // Called from userspace-initiated I/O (preempt_count > 0).
                // Lower the preempt count so the scheduler can context-switch
                // us out while we are blocked on I/O.  The timer interrupt
                // drives scheduling; WFI here is safe because the 1 kHz timer
                // will always wake the CPU before the 5-second timeout.
                // ============================================================
                #[cfg(target_arch = "aarch64")]
                crate::per_cpu_aarch64::preempt_enable();
                #[cfg(target_arch = "x86_64")]
                crate::per_cpu::preempt_enable();

                loop {
                    // Atomic check-and-block under scheduler lock.
                    let already_done = crate::task::scheduler::with_scheduler(|sched| {
                        if self.done.load(Ordering::Acquire) {
                            true // completion already fired — skip blocking
                        } else {
                            sched.block_current_for_io();
                            false
                        }
                    });

                    if already_done == Some(true) {
                        self.waiter.store(0, Ordering::Release);
                        #[cfg(target_arch = "aarch64")]
                        crate::per_cpu_aarch64::preempt_disable();
                        #[cfg(target_arch = "x86_64")]
                        crate::per_cpu::preempt_disable();
                        return Ok(true);
                    }

                    // Thread is now BlockedOnIO. WFI halts the CPU until the
                    // AHCI completion interrupt (or timer tick) fires. When the
                    // ISR calls complete() -> unblock_for_io(), the thread's
                    // state changes to Ready. On IRQ return, since we're still
                    // current, execution resumes here immediately — no context
                    // switch needed, no yield_current() latency.
                    #[cfg(target_arch = "aarch64")]
                    Cpu::halt_with_interrupts();

                    // After resuming, clear blocked_in_syscall.
                    crate::task::scheduler::with_scheduler(|sched| {
                        if let Some(t) = sched.current_thread_mut() {
                            t.blocked_in_syscall = false;
                        }
                    });

                    // Normal wakeup path.
                    if self.done.load(Ordering::Acquire) {
                        self.waiter.store(0, Ordering::Release);
                        #[cfg(target_arch = "aarch64")]
                        crate::per_cpu_aarch64::preempt_disable();
                        #[cfg(target_arch = "x86_64")]
                        crate::per_cpu::preempt_disable();
                        return Ok(true);
                    }

                    // EINTR: signal delivered while blocked.
                    if crate::syscall::check_signals_for_eintr().is_some() {
                        self.waiter.store(0, Ordering::Release);
                        crate::task::scheduler::with_scheduler(|sched| {
                            sched.unblock_for_io(tid);
                        });
                        #[cfg(target_arch = "aarch64")]
                        crate::per_cpu_aarch64::preempt_disable();
                        #[cfg(target_arch = "x86_64")]
                        crate::per_cpu::preempt_disable();
                        return Err(EINTR);
                    }

                    // Wall-clock timeout.
                    #[cfg(target_arch = "aarch64")]
                    {
                        let now: u64;
                        unsafe {
                            core::arch::asm!("mrs {}, cntpct_el0", out(reg) now, options(nomem, nostack));
                        }
                        if now >= deadline_cntpct {
                            self.waiter.store(0, Ordering::Release);
                            crate::per_cpu_aarch64::preempt_disable();
                            return Ok(false);
                        }
                    }
                    #[cfg(not(target_arch = "aarch64"))]
                    { let _ = deadline_cntpct; }
                }
            } else {
                // ============================================================
                // BOOT-THREAD SPIN PATH
                //
                // Scheduler is running but we are NOT in syscall context
                // (preempt_count = 0), meaning this is the raw boot thread
                // between scheduler init and timer init.  WFI is unsafe here
                // because the timer has not been started yet and may never
                // fire to rescue a stuck WFI.  Instead, spin with WFE:
                // complete() emits SEV which wakes WFE race-free.
                // ============================================================
                loop {
                    if self.done.load(Ordering::Acquire) {
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
                        // when a wired SPI becomes pending (confirmed empirically:
                        // wfi in the interrupt-driven AHCI loop on Parallels
                        // results in the vCPU staying parked until the next timer
                        // tick, but there is no timer yet at this boot stage).
                        unsafe { core::arch::asm!("yield", options(nomem, nostack)); }
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
                    if self.done.load(Ordering::Acquire) {
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
                    unsafe { core::arch::asm!("yield", options(nomem, nostack)); }
                }
            }
            #[cfg(not(target_arch = "aarch64"))]
            {
                // x86_64 early boot: plain spin (AHCI not primary driver here).
                let limit = timeout_ns / 1000; // rough cycle budget
                for _ in 0..limit {
                    if self.done.load(Ordering::Acquire) {
                        return Ok(true);
                    }
                    core::hint::spin_loop();
                }
                Ok(self.done.load(Ordering::Acquire))
            }
        }
    }

    /// Signal completion from ISR or any other context.
    ///
    /// Sets `done = true`, then wakes the waiter thread (if any) by calling
    /// `unblock_for_io()` under the scheduler lock.  This is safe from ISR
    /// context because `with_scheduler()` disables interrupts before locking,
    /// and the ISR already runs with IRQs masked.
    ///
    /// Idempotent: calling `complete()` multiple times is safe (the second
    /// call sees done=true and a waiter of 0, which is a no-op).
    pub fn complete(&self) {
        // Store done before we try to wake the waiter.
        self.done.store(true, Ordering::Release);
        // Fence: ensure done is visible on all CPUs before we read waiter.
        fence(Ordering::SeqCst);

        // SEV: set the ARM64 global event register so that any thread spinning
        // with WFE (e.g., the boot-thread spin path) wakes immediately even if
        // the interrupt fires before WFE is executed (race-free wakeup).
        // This is harmless on x86_64 (compiled away).
        #[cfg(target_arch = "aarch64")]
        unsafe { core::arch::asm!("sev", options(nomem, nostack)); }

        let tid = self.waiter.load(Ordering::Acquire);
        if tid != 0 {
            crate::task::scheduler::with_scheduler(|sched| {
                sched.unblock_for_io(tid);
            });
        }
    }
}

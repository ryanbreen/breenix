//! Preemptive scheduler implementation
//!
//! This module implements a round-robin scheduler for kernel threads.
//!
//! # Lock Ordering Discipline
//!
//! The kernel uses a strict lock ordering hierarchy to prevent deadlocks.
//! Locks must ALWAYS be acquired in the order listed below. Never acquire a
//! higher-priority (lower-numbered) lock while holding a lower-priority
//! (higher-numbered) lock.
//!
//! ```text
//! Level 1: SCHEDULER       (kernel/src/task/scheduler.rs)     — highest priority
//! Level 2: PROCESS_MANAGER (kernel/src/process/mod.rs)
//! Level 3: STDIN_BUFFER / BLOCKED_READERS (kernel/src/ipc/stdin.rs)
//! Level 4: SERIAL1         (kernel/src/serial_aarch64.rs)     — lowest priority
//! ```
//!
//! ## Key Rules
//!
//! - **Never acquire SERIAL1 while holding SCHEDULER or PROCESS_MANAGER.**
//!   This means no `serial_println!`, `log_serial_println!`, or `write_byte()`
//!   calls from code that holds the scheduler lock. Use `raw_uart_char()` /
//!   `raw_uart_str()` from `serial_aarch64.rs` or `context_switch.rs` for
//!   lock-free debug output instead.
//!
//! - **Never acquire SCHEDULER while holding SERIAL1.** Timer interrupts that
//!   fire while SERIAL1 is held must not try to acquire SCHEDULER. On ARM64,
//!   `write_byte()` and `_print()` disable interrupts before acquiring SERIAL1
//!   to prevent this.
//!
//! - **IRQ context must use lock-free output.** Interrupt handlers (keyboard,
//!   timer, UART RX) must use `raw_serial_char()` / `raw_serial_str()` or the
//!   lock-free `raw_uart_char()` / `raw_uart_str()` for any diagnostic output.
//!   They must never call `serial_println!` or `crate::serial::write_byte()`.
//!
//! ## Rationale
//!
//! On ARM64 SMP, there is a single PL011 UART shared by all CPUs. If CPU 0
//! holds SERIAL1 (via `serial_println!`) and CPU 1 holds SCHEDULER, then:
//! - CPU 0's timer interrupt tries to acquire SCHEDULER → spins on CPU 1
//! - CPU 1 tries to log via `serial_println!` → spins on SERIAL1 held by CPU 0
//! - Classic ABBA deadlock.
//!
//! On x86_64, kernel logging goes to COM2 (separate from COM1 user I/O), so
//! the SERIAL1 contention is less severe. The `#[cfg(target_arch = "x86_64")]`
//! guards on `log_serial_println!` calls in this file reflect that difference.

use super::thread::{Thread, ThreadState};
use crate::log_serial_println;
use alloc::{boxed::Box, collections::VecDeque};
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

// Architecture-generic HAL wrappers for interrupt control.
use crate::{
    arch_interrupts_enabled as are_enabled,
    arch_without_interrupts as without_interrupts,
};

/// Global scheduler instance
static SCHEDULER: Mutex<Option<Scheduler>> = Mutex::new(None);

/// Global need_resched flag for timer interrupt
static NEED_RESCHED: AtomicBool = AtomicBool::new(false);

/// Counter for unblock() calls - used for testing pipe wake mechanism
/// This is a global atomic because:
/// 1. unblock() is called via with_scheduler() which already holds the scheduler lock
/// 2. Tests need to read this outside the scheduler lock
/// 3. AtomicU64 ensures visibility across threads without additional locking
static UNBLOCK_CALL_COUNT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Get the current unblock() call count (for testing)
///
/// This function is used by the test framework to verify that pipe wake
/// mechanisms actually call scheduler.unblock(). It's only called when
/// the boot_tests feature is enabled.
#[allow(dead_code)] // Used by test_framework when boot_tests feature is enabled
pub fn unblock_call_count() -> u64 {
    UNBLOCK_CALL_COUNT.load(Ordering::SeqCst)
}

/// Maximum CPUs for scheduler state arrays.
#[cfg(target_arch = "aarch64")]
const MAX_CPUS: usize = 8;
#[cfg(not(target_arch = "aarch64"))]
const MAX_CPUS: usize = 1;

/// DIAGNOSTIC: Circular buffer tracking last N cpu_state changes per CPU.
/// Each entry: (setter_id, old_thread, new_thread)
/// Setter IDs:
///   1 = commit_cpu_state_after_save
///   2 = switch_to_idle
///   3 = switch_to_idle_best_effort
///   4 = register_idle_thread
///   5 = init_with_current / Scheduler::new
///   6 = set_current_thread / add_thread_as_current
#[cfg(target_arch = "aarch64")]
const HISTORY_SIZE: usize = 8;
#[cfg(target_arch = "aarch64")]
static CPU_STATE_HISTORY: [[core::sync::atomic::AtomicU64; HISTORY_SIZE * 3]; MAX_CPUS] = {
    const INIT_ENTRY: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
    const INIT_CPU: [core::sync::atomic::AtomicU64; HISTORY_SIZE * 3] = [INIT_ENTRY; HISTORY_SIZE * 3];
    [INIT_CPU; MAX_CPUS]
};
#[cfg(target_arch = "aarch64")]
static CPU_STATE_HISTORY_IDX: [core::sync::atomic::AtomicU64; MAX_CPUS] = {
    const INIT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
    [INIT; MAX_CPUS]
};

/// Record a cpu_state change for diagnostics (circular buffer).
#[cfg(target_arch = "aarch64")]
fn record_cpu_state_change(cpu: usize, setter_id: u64, old_val: u64, new_val: u64) {
    if cpu < MAX_CPUS {
        let idx = CPU_STATE_HISTORY_IDX[cpu].fetch_add(1, core::sync::atomic::Ordering::Relaxed) as usize;
        let slot = idx % HISTORY_SIZE;
        let base = slot * 3;
        CPU_STATE_HISTORY[cpu][base].store(setter_id, core::sync::atomic::Ordering::Relaxed);
        CPU_STATE_HISTORY[cpu][base + 1].store(old_val, core::sync::atomic::Ordering::Relaxed);
        CPU_STATE_HISTORY[cpu][base + 2].store(new_val, core::sync::atomic::Ordering::Relaxed);
    }
}

/// Dump the cpu_state change history for a CPU (debug utility).
#[cfg(target_arch = "aarch64")]
#[allow(dead_code)]
pub fn dump_cpu_state_history(cpu: usize) {
    use crate::arch_impl::aarch64::context_switch::{raw_uart_str, raw_uart_dec};
    if cpu >= MAX_CPUS { return; }
    let total = CPU_STATE_HISTORY_IDX[cpu].load(core::sync::atomic::Ordering::Relaxed) as usize;
    let count = if total < HISTORY_SIZE { total } else { HISTORY_SIZE };
    let start = if total < HISTORY_SIZE { 0 } else { total - HISTORY_SIZE };
    raw_uart_str("  cpu_state_history[");
    raw_uart_dec(cpu as u64);
    raw_uart_str("] (last ");
    raw_uart_dec(count as u64);
    raw_uart_str(" of ");
    raw_uart_dec(total as u64);
    raw_uart_str("):\n");
    for i in 0..count {
        let slot = (start + i) % HISTORY_SIZE;
        let base = slot * 3;
        let setter = CPU_STATE_HISTORY[cpu][base].load(core::sync::atomic::Ordering::Relaxed);
        let old = CPU_STATE_HISTORY[cpu][base + 1].load(core::sync::atomic::Ordering::Relaxed);
        let new = CPU_STATE_HISTORY[cpu][base + 2].load(core::sync::atomic::Ordering::Relaxed);
        raw_uart_str("    [");
        raw_uart_dec((start + i) as u64);
        raw_uart_str("] setter=");
        raw_uart_dec(setter);
        raw_uart_str(" ");
        raw_uart_dec(old);
        raw_uart_str("->");
        raw_uart_dec(new);
        raw_uart_str("\n");
    }
}

/// Per-CPU scheduler state.
struct CpuSchedulerState {
    /// Currently running thread ID on this CPU
    current_thread: Option<u64>,
    /// Idle thread ID for this CPU
    idle_thread: u64,
}

/// The kernel scheduler
pub struct Scheduler {
    /// All threads in the system
    threads: alloc::vec::Vec<Box<Thread>>,

    /// Ready queue (thread IDs)
    ready_queue: VecDeque<u64>,

    /// Per-CPU scheduler state (current_thread + idle_thread per CPU)
    cpu_state: [CpuSchedulerState; MAX_CPUS],
}

impl Scheduler {
    /// Create a new scheduler with an idle thread for CPU 0.
    pub fn new(idle_thread: Box<Thread>) -> Self {
        let idle_id = idle_thread.id();

        // Initialize all CPU states: CPU 0 gets the idle thread, rest are empty
        const EMPTY_STATE: CpuSchedulerState = CpuSchedulerState {
            current_thread: None,
            idle_thread: 0,
        };
        let mut cpu_state = [EMPTY_STATE; MAX_CPUS];
        cpu_state[0] = CpuSchedulerState {
            current_thread: Some(idle_id),
            idle_thread: idle_id,
        };

        let scheduler = Self {
            threads: alloc::vec![idle_thread],
            ready_queue: VecDeque::new(),
            cpu_state,
        };

        scheduler
    }

    // -------------------------------------------------------------------------
    // Per-CPU state accessors (backward-compatible with single-CPU code)
    // -------------------------------------------------------------------------

    /// Get the current CPU ID for scheduler operations.
    #[inline]
    fn current_cpu_id() -> usize {
        #[cfg(target_arch = "aarch64")]
        {
            crate::arch_impl::aarch64::percpu::Aarch64PerCpu::cpu_id() as usize
        }
        #[cfg(not(target_arch = "aarch64"))]
        {
            0
        }
    }

    /// Register an idle thread for a specific CPU.
    /// Called during secondary CPU bringup to set up per-CPU idle tasks.
    #[cfg(target_arch = "aarch64")]
    pub fn register_idle_thread(&mut self, cpu_id: usize, idle_thread: Box<Thread>) {
        if cpu_id >= MAX_CPUS {
            return;
        }
        let idle_id = idle_thread.id();
        self.threads.push(idle_thread);
        self.cpu_state[cpu_id].idle_thread = idle_id;
        self.cpu_state[cpu_id].current_thread = Some(idle_id);
    }

    /// Add a new thread to the scheduler
    pub fn add_thread(&mut self, thread: Box<Thread>) {
        self.add_thread_inner(thread, false);
    }

    /// Add a new thread to the front of the ready queue.
    /// Used for fork children so they run before other waiting threads,
    /// following the Linux convention where children exec quickly and exit.
    pub fn add_thread_front(&mut self, thread: Box<Thread>) {
        self.add_thread_inner(thread, true);
    }

    fn add_thread_inner(&mut self, thread: Box<Thread>, front: bool) {
        let thread_id = thread.id();
        let thread_name = thread.name.clone();
        let is_user = thread.privilege == super::thread::ThreadPrivilege::User;
        self.threads.push(thread);
        if front {
            self.ready_queue.push_front(thread_id);
        } else {
            self.ready_queue.push_back(thread_id);
        }
        // CRITICAL: Only log on x86_64. On ARM64, log_serial_println! uses the same
        // SERIAL1 lock as serial_println!, causing deadlock if timer fires while
        // boot code is printing.
        #[cfg(target_arch = "x86_64")]
        log_serial_println!(
            "Added thread {} '{}' to scheduler (user: {}, ready_queue: {:?})",
            thread_id,
            thread_name,
            is_user,
            self.ready_queue
        );
        #[cfg(not(target_arch = "x86_64"))]
        let _ = (thread_id, thread_name, is_user);
    }

    /// Add a thread as the current running thread without scheduling.
    ///
    /// Used when manually starting the first userspace thread (init process).
    /// The thread is added to the scheduler's thread list and marked as current,
    /// but NOT added to the ready queue. This avoids the scheduler trying to
    /// reschedule when timer interrupts fire.
    #[allow(dead_code)]
    pub fn add_thread_as_current(&mut self, mut thread: Box<Thread>) {
        let thread_id = thread.id();
        let thread_name = thread.name.clone();
        // Mark thread as running
        thread.state = ThreadState::Running;
        thread.has_started = true;
        self.threads.push(thread);
        self.cpu_state[Self::current_cpu_id()].current_thread = Some(thread_id);
        // CRITICAL: Only log on x86_64 to avoid deadlock on ARM64
        #[cfg(target_arch = "x86_64")]
        log_serial_println!(
            "Added thread {} '{}' as current (not in ready_queue)",
            thread_id,
            thread_name,
        );
        #[cfg(not(target_arch = "x86_64"))]
        let _ = (thread_id, thread_name);
    }

    /// Get a mutable thread by ID
    pub fn get_thread_mut(&mut self, id: u64) -> Option<&mut Thread> {
        self.threads
            .iter_mut()
            .find(|t| t.id() == id)
            .map(|t| t.as_mut())
    }

    /// Get the current running thread
    #[allow(dead_code)]
    pub fn current_thread(&self) -> Option<&Thread> {
        self.cpu_state[Self::current_cpu_id()].current_thread.and_then(|id| self.get_thread(id))
    }

    /// Get the current running thread mutably
    pub fn current_thread_mut(&mut self) -> Option<&mut Thread> {
        self.cpu_state[Self::current_cpu_id()].current_thread
            .and_then(move |id| self.get_thread_mut(id))
    }

    /// Get the current thread ID
    #[allow(dead_code)]
    pub fn current_thread_id_inner(&self) -> Option<u64> {
        self.cpu_state[Self::current_cpu_id()].current_thread
    }

    /// Get the idle thread ID
    #[allow(dead_code)]
    pub fn idle_thread_id(&self) -> u64 {
        self.cpu_state[Self::current_cpu_id()].idle_thread
    }

    /// Schedule the next thread to run
    /// Returns (old_thread, new_thread) for context switching
    pub fn schedule(&mut self) -> Option<(&mut Thread, &Thread)> {
        // Count schedule calls - only log very sparingly to avoid timing issues
        // Serial output is ~960 bytes/sec, so each log line can take 50-100ms!
        static SCHEDULE_COUNT: core::sync::atomic::AtomicU64 =
            core::sync::atomic::AtomicU64::new(0);
        let _count = SCHEDULE_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);

        // CRITICAL: Logging disabled on ARM64 - schedule() is called from context switch
        // path which may be holding the serial lock. On ARM64, log_serial_println! uses
        // the same SERIAL1 lock as serial_println!, causing deadlock if timer fires
        // while boot code is printing.
        // On x86_64, log_serial goes to a separate UART (COM2), so it's safe.
        #[cfg(target_arch = "x86_64")]
        let debug_log = _count < 5 || (_count % 500 == 0);
        #[cfg(not(target_arch = "x86_64"))]
        let debug_log = false;

        // If current thread is still runnable, put it back in ready queue
        if let Some(current_id) = self.cpu_state[Self::current_cpu_id()].current_thread {
            if current_id != self.cpu_state[Self::current_cpu_id()].idle_thread {
                // Check the state and determine what to do
                let (is_terminated, is_blocked) =
                    if let Some(current) = self.get_thread_mut(current_id) {
                        // Charge elapsed CPU ticks to the outgoing thread
                        let now = crate::time::get_ticks();
                        current.cpu_ticks_total += now.wrapping_sub(current.run_start_ticks);

                        let was_terminated = current.state == ThreadState::Terminated;
                        // Check for any blocked state
                        let was_blocked = current.state == ThreadState::Blocked
                            || current.state == ThreadState::BlockedOnSignal
                            || current.state == ThreadState::BlockedOnChildExit
                            || current.state == ThreadState::BlockedOnTimer;
                        // Only set to Ready if not terminated AND not blocked
                        if !was_terminated && !was_blocked {
                            current.set_ready();
                        }
                        (was_terminated, was_blocked)
                    } else {
                        (true, false)
                    };

                // Put non-terminated, non-blocked threads back in ready queue
                // CRITICAL: Check for duplicates! If unblock() already added this thread
                // (e.g., packet arrived during blocking recvfrom), don't add it again.
                // Duplicates cause schedule() to spin when same thread keeps getting selected.
                let in_queue = self.ready_queue.contains(&current_id);
                let will_add = !is_terminated && !is_blocked && !in_queue;

                if will_add {
                    self.ready_queue.push_back(current_id);
                }
            }
        }

        // Check for expired timer-blocked threads and wake them
        self.wake_expired_timers();

        // Get next thread from ready queue, skipping terminated threads.
        // Terminated threads can end up in the queue if a process was killed
        // by an exception handler on another CPU between requeue and pop.
        let mut next_thread_id = loop {
            if let Some(n) = self.ready_queue.pop_front() {
                if let Some(thread) = self.get_thread(n) {
                    if thread.state == ThreadState::Terminated {
                        continue;
                    }
                }
                break n;
            } else {
                break self.cpu_state[Self::current_cpu_id()].idle_thread;
            }
        };

        if debug_log {
            log_serial_println!(
                "Next thread from queue: {}, ready_queue after pop: {:?}",
                next_thread_id,
                self.ready_queue
            );
        }

        // Important: Don't skip if it's the same thread when there are other threads waiting
        // This was causing the issue where yielding wouldn't switch to other ready threads
        if Some(next_thread_id) == self.cpu_state[Self::current_cpu_id()].current_thread && !self.ready_queue.is_empty() {
            // Put current thread back and get the next one
            self.ready_queue.push_back(next_thread_id);
            next_thread_id = self.ready_queue.pop_front()?;
        } else if Some(next_thread_id) == self.cpu_state[Self::current_cpu_id()].current_thread {
            // Current thread is the only runnable thread.
            // If it's NOT the idle thread, switch to idle to give it a chance.
            // This is important for kthreads that yield while waiting for the idle
            // thread (which runs tests/main logic) to set a flag.
            if next_thread_id != self.cpu_state[Self::current_cpu_id()].idle_thread {
                // On ARM64, don't switch userspace threads to idle. Idle runs in kernel
                // mode (EL1), and ARM64 only preempts when returning to userspace (from_el0=true).
                // If we switched a userspace thread to idle, idle would never be preempted
                // back to the userspace thread because timer fires with from_el0=false.
                #[cfg(target_arch = "aarch64")]
                {
                    let is_userspace = self
                        .get_thread(next_thread_id)
                        .map(|t| t.privilege == super::thread::ThreadPrivilege::User)
                        .unwrap_or(false);
                    if is_userspace {
                        // Userspace thread is alone - keep running it, don't switch to idle
                        if debug_log {
                            log_serial_println!(
                                "Thread {} is userspace and alone, continuing (no idle switch)",
                                next_thread_id
                            );
                        }
                        return None;
                    }
                }
                self.ready_queue.push_back(next_thread_id);
                next_thread_id = self.cpu_state[Self::current_cpu_id()].idle_thread;
                // CRITICAL: Set NEED_RESCHED so the next timer interrupt will
                // switch back to the deferred thread. Without this, idle would
                // spin in HLT for an entire quantum (50ms) before rescheduling.
                #[cfg(target_arch = "x86_64")]
                crate::per_cpu::set_need_resched(true);
                #[cfg(target_arch = "aarch64")]
                crate::per_cpu_aarch64::set_need_resched(true);
                if debug_log {
                    log_serial_println!(
                        "Thread {} is alone (non-idle), switching to idle {}",
                        self.cpu_state[Self::current_cpu_id()].current_thread.unwrap_or(0),
                        self.cpu_state[Self::current_cpu_id()].idle_thread
                    );
                }
            } else {
                // Idle is the only runnable thread - keep running it.
                // No context switch needed.
                // NOTE: Do NOT push idle to ready_queue here! Idle came from
                // the fallback (line 129), not from pop_front. The ready_queue
                // should remain empty. Pushing idle here would accumulate idle
                // entries in the queue, causing incorrect scheduling when new
                // threads are spawned (the queue would contain both idle AND the
                // new thread, when it should only contain the new thread).
                if debug_log {
                    log_serial_println!(
                        "Idle thread {} is alone, continuing (no switch needed)",
                        next_thread_id
                    );
                }
                return None;
            }
        }

        // If current is idle and we have a real next thread, allow switch even if idle
        let old_thread_id = self.cpu_state[Self::current_cpu_id()].current_thread.unwrap_or(self.cpu_state[Self::current_cpu_id()].idle_thread);
        self.cpu_state[Self::current_cpu_id()].current_thread = Some(next_thread_id);

        if debug_log {
            log_serial_println!(
                "Switching from thread {} to thread {}",
                old_thread_id,
                next_thread_id
            );
        }

        // Mark new thread as running
        if let Some(next) = self.get_thread_mut(next_thread_id) {
            next.set_running();
            next.run_start_ticks = crate::time::get_ticks();
        }

        // Get mutable reference to old thread and immutable to new
        // This is safe because we know they're different threads
        unsafe {
            let threads_ptr = self.threads.as_mut_ptr();
            let old_idx = self.threads.iter().position(|t| t.id() == old_thread_id)?;
            let new_idx = self.threads.iter().position(|t| t.id() == next_thread_id)?;

            let old_thread = &mut *(*threads_ptr.add(old_idx)).as_mut();
            let new_thread = &*(*threads_ptr.add(new_idx)).as_ref();

            Some((old_thread, new_thread))
        }
    }

    /// Schedule the next thread, but do NOT add the old thread to the ready queue.
    ///
    /// This is used on ARM64 SMP to prevent a race condition where another CPU
    /// picks up the old thread from the ready queue before the current CPU has
    /// finished saving its context. The caller must call `requeue_thread_after_save()`
    /// after saving the old thread's context.
    ///
    /// Returns (old_thread_id, new_thread_id, should_requeue_old) where
    /// should_requeue_old indicates whether the old thread should be added to
    /// the ready queue after its context is saved.
    #[cfg(target_arch = "aarch64")]
    pub fn schedule_deferred_requeue(&mut self) -> Option<(u64, u64, bool)> {
        // If current thread is still runnable, mark it as Ready but DON'T add to queue
        let mut should_requeue_old = false;
        if let Some(current_id) = self.cpu_state[Self::current_cpu_id()].current_thread {
            if current_id != self.cpu_state[Self::current_cpu_id()].idle_thread {
                let (is_terminated, is_blocked) =
                    if let Some(current) = self.get_thread_mut(current_id) {
                        // Charge elapsed CPU ticks to the outgoing thread
                        let now = crate::time::get_ticks();
                        current.cpu_ticks_total += now.wrapping_sub(current.run_start_ticks);

                        let was_terminated = current.state == ThreadState::Terminated;
                        let was_blocked = current.state == ThreadState::Blocked
                            || current.state == ThreadState::BlockedOnSignal
                            || current.state == ThreadState::BlockedOnChildExit;
                        if !was_terminated && !was_blocked {
                            current.set_ready();
                        }
                        (was_terminated, was_blocked)
                    } else {
                        (true, false)
                    };

                let in_queue = self.ready_queue.contains(&current_id);
                // Instead of adding to ready_queue, just record whether we SHOULD
                should_requeue_old = !is_terminated && !is_blocked && !in_queue;
                // NOTE: We intentionally do NOT push to ready_queue here.
                // The caller will do so after saving context via requeue_thread_after_save().
            }
        }

        // Get next thread from ready queue, skipping terminated threads.
        // Terminated threads can end up in the queue if a process was killed
        // by an exception handler on another CPU between requeue and pop.
        let mut next_thread_id = loop {
            if let Some(n) = self.ready_queue.pop_front() {
                if let Some(thread) = self.get_thread(n) {
                    if thread.state == ThreadState::Terminated {
                        continue;
                    }
                }
                break n;
            } else {
                break self.cpu_state[Self::current_cpu_id()].idle_thread;
            }
        };

        // Handle same-thread cases
        if Some(next_thread_id) == self.cpu_state[Self::current_cpu_id()].current_thread && !self.ready_queue.is_empty() {
            // Current thread was popped but other threads are waiting.
            // DON'T push current back to queue yet — defer until after context save.
            // Just pop the next different thread.
            should_requeue_old = true;
            next_thread_id = match self.ready_queue.pop_front() {
                Some(id) => id,
                None => return None,
            };
        } else if Some(next_thread_id) == self.cpu_state[Self::current_cpu_id()].current_thread {
            if next_thread_id != self.cpu_state[Self::current_cpu_id()].idle_thread {
                let is_userspace = self
                    .get_thread(next_thread_id)
                    .map(|t| t.privilege == super::thread::ThreadPrivilege::User)
                    .unwrap_or(false);
                if is_userspace {
                    // No switch needed. The current thread continues running on
                    // this CPU. Don't requeue — it's still "current" and will be
                    // handled next time schedule_deferred_requeue is called.
                    return None;
                }
                // For non-userspace same-thread-alone: switch to idle.
                // The old thread (which was popped) must be requeued AFTER
                // context save — same deferred-requeue logic applies. Whether
                // the thread was in the queue from unblock() or from the
                // deferred push, either way we must save context first.
                should_requeue_old = true;
                next_thread_id = self.cpu_state[Self::current_cpu_id()].idle_thread;
                crate::per_cpu_aarch64::set_need_resched(true);
            } else {
                return None;
            }
        }

        let old_thread_id = self.cpu_state[Self::current_cpu_id()].current_thread
            .unwrap_or(self.cpu_state[Self::current_cpu_id()].idle_thread);

        // CRITICAL SMP FIX: Do NOT update cpu_state[cpu].current_thread here!
        //
        // Previously we did:  self.cpu_state[cpu].current_thread = Some(next_thread_id);
        //
        // The problem: updating cpu_state removes the old thread from is_current_on_any_cpu().
        // If the old thread is Blocked (e.g., parked render thread or userspace thread blocked
        // in sys_read), unblock() on another CPU sees it's not "current" anywhere and adds it
        // to the ready queue. A third CPU then dispatches it with STALE context (we haven't
        // saved the context yet!). This causes ERET to address 0x0.
        //
        // The fix: defer the cpu_state update until AFTER context is saved. The caller must
        // call commit_cpu_state_after_save() to finalize the switch. While cpu_state still
        // shows the old thread as "current", unblock() will see is_current_on_any_cpu()=true
        // and skip the ready_queue addition (the CPU running the thread will handle it).

        if let Some(next) = self.get_thread_mut(next_thread_id) {
            next.set_running();
            next.run_start_ticks = crate::time::get_ticks();
        }

        Some((old_thread_id, next_thread_id, should_requeue_old))
    }

    /// Finalize cpu_state after context save.
    ///
    /// This must be called after save_kernel_context_arm64 / save_userspace_context_arm64
    /// and BEFORE requeue_thread_after_save. It updates cpu_state[cpu].current_thread
    /// to the new thread, which allows unblock() on other CPUs to see the old thread
    /// as no longer "current" and add it to the ready queue.
    #[cfg(target_arch = "aarch64")]
    pub fn commit_cpu_state_after_save(&mut self, new_thread_id: u64) {
        let cpu = Self::current_cpu_id();
        let old_val = self.cpu_state[cpu].current_thread.unwrap_or(0xDEAD);
        record_cpu_state_change(cpu, 1, old_val, new_thread_id);
        self.cpu_state[cpu].current_thread = Some(new_thread_id);
    }

    /// Add a thread to the ready queue after its context has been saved.
    ///
    /// This completes the deferred requeue from `schedule_deferred_requeue()`.
    /// Must be called only after the thread's context has been fully saved
    /// to prevent other CPUs from dispatching it with stale state.
    #[cfg(target_arch = "aarch64")]
    pub fn requeue_thread_after_save(&mut self, thread_id: u64) {
        // Don't requeue idle threads (they are never in the ready queue)
        if (0..MAX_CPUS).any(|cpu| self.cpu_state[cpu].idle_thread == thread_id) {
            return;
        }
        // Safety checks: only requeue if the thread is in Ready state and not already queued
        if let Some(thread) = self.get_thread(thread_id) {
            if thread.state != ThreadState::Ready {
                return; // Thread state changed (terminated/blocked) - don't requeue
            }
        } else {
            return;
        }
        if !self.ready_queue.contains(&thread_id) {
            self.ready_queue.push_back(thread_id);
            // Send IPI to wake an idle CPU to pick up the requeued thread
            self.send_resched_ipi();
        }
    }

    /// Block the current thread
    #[allow(dead_code)]
    pub fn block_current(&mut self) {
        if let Some(current) = self.current_thread_mut() {
            current.set_blocked();
        }
    }

    /// Unblock a thread by ID
    pub fn unblock(&mut self, thread_id: u64) {
        // Increment the call counter for testing (tracks that unblock was called)
        UNBLOCK_CALL_COUNT.fetch_add(1, Ordering::SeqCst);

        if let Some(thread) = self.get_thread_mut(thread_id) {
            if thread.state == ThreadState::Blocked || thread.state == ThreadState::BlockedOnSignal || thread.state == ThreadState::BlockedOnTimer {
                thread.set_ready();

                // SMP safety: Don't add to ready_queue if thread is currently
                // running on any CPU. If a thread is blocked in a syscall's WFI
                // loop (e.g., sys_read waiting for keyboard input), it's still
                // the "current thread" on that CPU. Adding it to the ready_queue
                // would allow another CPU to schedule it simultaneously, causing
                // double-scheduling: two CPUs executing the same thread with the
                // same stack, leading to context corruption and crashes (ELR=0x0).
                // The CPU running the thread will detect the state change (Blocked
                // → Ready) when its WFI loop checks the thread state after waking.
                let is_current_on_any_cpu = (0..MAX_CPUS).any(|cpu| {
                    self.cpu_state[cpu].current_thread == Some(thread_id)
                });

                if !is_current_on_any_cpu
                    && thread_id != self.cpu_state[Self::current_cpu_id()].idle_thread
                    && !self.ready_queue.contains(&thread_id)
                {
                    self.ready_queue.push_back(thread_id);
                    // CRITICAL: Only log on x86_64 to avoid deadlock on ARM64
                    #[cfg(target_arch = "x86_64")]
                    log_serial_println!("unblock({}): Added to ready_queue", thread_id);

                    // Send IPI to wake an idle CPU so it can pick up the unblocked thread
                    #[cfg(target_arch = "aarch64")]
                    self.send_resched_ipi();
                }
            }
        }
    }

    /// Send a reschedule IPI (SGI 0) to an idle CPU.
    ///
    /// Called after adding a thread to the ready queue to wake a CPU that's
    /// sitting in WFI so it can pick up the newly-runnable thread.
    /// Only sends to one idle CPU (the first one found) to avoid thundering herd.
    #[cfg(target_arch = "aarch64")]
    fn send_resched_ipi(&self) {
        use crate::arch_impl::aarch64::smp;

        let current_cpu = Self::current_cpu_id();
        let online = smp::cpus_online() as usize;

        for cpu in 0..online {
            if cpu == current_cpu {
                continue;
            }
            // Check if this CPU is running its idle thread
            if cpu < MAX_CPUS {
                if let Some(current) = self.cpu_state[cpu].current_thread {
                    if current == self.cpu_state[cpu].idle_thread {
                        // This CPU is idle - send it a reschedule IPI
                        crate::arch_impl::aarch64::gic::send_sgi(
                            crate::arch_impl::aarch64::constants::SGI_RESCHEDULE as u8,
                            cpu as u8,
                        );
                        return; // Only wake one CPU
                    }
                }
            }
        }
    }

    /// Block current thread until a signal is delivered
    /// Used by the pause() syscall
    ///
    /// NOTE: This does NOT set current_thread to None because the thread
    /// is still physically running the syscall. The schedule() function
    /// will check the thread state and not put it back in ready queue.
    pub fn block_current_for_signal(&mut self) {
        self.block_current_for_signal_with_context(None)
    }

    /// Block current thread until a signal is delivered, saving userspace context
    /// Used by the pause() syscall
    ///
    /// CRITICAL: This version atomically saves the userspace context AND sets
    /// blocked_in_syscall=true under the same scheduler lock. This prevents
    /// a race condition where a signal could arrive after the context is saved
    /// to process.main_thread but before blocked_in_syscall is set.
    ///
    /// The saved_userspace_context on the SCHEDULER's Thread is the single source
    /// of truth for signal delivery - context_switch.rs reads from here, not from
    /// process.main_thread.
    ///
    /// NOTE: This does NOT set current_thread to None because the thread
    /// is still physically running the syscall. The schedule() function
    /// will check the thread state and not put it back in ready queue.
    pub fn block_current_for_signal_with_context(
        &mut self,
        userspace_context: Option<super::thread::CpuContext>,
    ) {
        if let Some(current_id) = self.cpu_state[Self::current_cpu_id()].current_thread {
            if let Some(thread) = self.get_thread_mut(current_id) {
                // CRITICAL: Save userspace context FIRST, THEN set state.
                // This ensures that when unblock_for_signal() is called,
                // the context is already saved and ready for signal delivery.
                if let Some(ctx) = userspace_context {
                    thread.saved_userspace_context = Some(ctx);
                    // CRITICAL: Only log on x86_64 to avoid deadlock on ARM64
                    #[cfg(target_arch = "x86_64")]
                    log_serial_println!(
                        "Thread {} saving userspace context: RIP={:#x}",
                        current_id,
                        thread.saved_userspace_context.as_ref().unwrap().rip
                    );
                    // ARM64: No logging - would cause deadlock
                }
                thread.state = ThreadState::BlockedOnSignal;
                // CRITICAL: Mark that this thread is blocked inside a syscall.
                // When the thread is resumed, we must NOT restore userspace context
                // because that would return to the pre-syscall location instead of
                // letting the syscall complete and return properly.
                thread.blocked_in_syscall = true;
                // CRITICAL: Only log on x86_64 to avoid deadlock on ARM64
                #[cfg(target_arch = "x86_64")]
                log_serial_println!("Thread {} blocked waiting for signal (blocked_in_syscall=true)", current_id);
            }
            // Remove from ready queue (shouldn't be there but make sure)
            self.ready_queue.retain(|&id| id != current_id);
            // NOTE: Do NOT clear current_thread here!
            // The thread is still running (inside the syscall handler).
            // schedule() will detect the Blocked state and not put it back in ready queue.
        }
    }

    /// Unblock a thread that was waiting for a signal
    /// Called when a signal is delivered to a blocked thread
    ///
    /// NOTE: This function sets the need_resched flag when a thread is successfully
    /// unblocked to ensure it gets scheduled promptly. This is critical for pause()
    /// to wake up in a timely manner when a signal arrives.
    pub fn unblock_for_signal(&mut self, thread_id: u64) {
        // CRITICAL: Only log on x86_64 to avoid deadlock on ARM64
        #[cfg(target_arch = "x86_64")]
        log_serial_println!(
            "unblock_for_signal: Checking thread {} (current={:?}, ready_queue={:?})",
            thread_id,
            self.cpu_state[Self::current_cpu_id()].current_thread,
            self.ready_queue
        );
        if let Some(thread) = self.get_thread_mut(thread_id) {
            #[cfg(target_arch = "x86_64")]
            log_serial_println!(
                "unblock_for_signal: Thread {} state is {:?}, blocked_in_syscall={}",
                thread_id,
                thread.state,
                thread.blocked_in_syscall
            );
            if thread.state == ThreadState::BlockedOnSignal {
                thread.set_ready();
                // NOTE: Do NOT clear blocked_in_syscall here!
                // The thread needs to resume inside the syscall and complete it.
                // blocked_in_syscall will be cleared when the syscall actually returns.

                // SMP safety: Don't add to ready_queue if thread is current on any CPU
                // (same rationale as unblock() - prevents double-scheduling)
                let is_current_on_any_cpu = (0..MAX_CPUS).any(|cpu| {
                    self.cpu_state[cpu].current_thread == Some(thread_id)
                });

                if !is_current_on_any_cpu
                    && thread_id != self.cpu_state[Self::current_cpu_id()].idle_thread
                    && !self.ready_queue.contains(&thread_id)
                {
                    self.ready_queue.push_back(thread_id);
                    #[cfg(target_arch = "x86_64")]
                    log_serial_println!(
                        "unblock_for_signal: Thread {} unblocked, added to ready_queue={:?}",
                        thread_id,
                        self.ready_queue
                    );

                    // Send IPI to wake an idle CPU
                    #[cfg(target_arch = "aarch64")]
                    self.send_resched_ipi();
                } else {
                    #[cfg(target_arch = "x86_64")]
                    log_serial_println!(
                        "unblock_for_signal: Thread {} already in queue, is idle, or is current on a CPU",
                        thread_id
                    );
                }
                // CRITICAL: Request reschedule so the unblocked thread can run promptly.
                // Without this, the thread is added to ready queue but the scheduler
                // doesn't know to switch to it, causing pause() to timeout waiting for
                // the next timer tick instead of waking up immediately.
                set_need_resched();
            } else {
                #[cfg(target_arch = "x86_64")]
                log_serial_println!(
                    "unblock_for_signal: Thread {} not BlockedOnSignal, state={:?}",
                    thread_id,
                    thread.state
                );
            }
        } else {
            #[cfg(target_arch = "x86_64")]
            log_serial_println!("unblock_for_signal: Thread {} not found!", thread_id);
        }
    }

    /// Block current thread until a child exits
    /// Used by the waitpid() syscall
    ///
    /// NOTE: This does NOT set current_thread to None because the thread
    /// is still physically running the syscall. The schedule() function
    /// will check the thread state and not put it back in ready queue.
    pub fn block_current_for_child_exit(&mut self) {
        if let Some(current_id) = self.cpu_state[Self::current_cpu_id()].current_thread {
            if let Some(thread) = self.get_thread_mut(current_id) {
                thread.state = ThreadState::BlockedOnChildExit;
                // CRITICAL: Mark that this thread is blocked inside a syscall.
                // When the thread is resumed, we must NOT restore userspace context
                // because that would return to the pre-syscall location instead of
                // letting the syscall complete and return properly.
                thread.blocked_in_syscall = true;
                // CRITICAL: Only log on x86_64 to avoid deadlock on ARM64
                #[cfg(target_arch = "x86_64")]
                log_serial_println!("Thread {} blocked waiting for child exit (blocked_in_syscall=true)", current_id);
            }
            // Remove from ready queue (shouldn't be there but make sure)
            self.ready_queue.retain(|&id| id != current_id);
            // NOTE: Do NOT clear current_thread here!
            // The thread is still running (inside the syscall handler).
            // schedule() will detect the Blocked state and not put it back in ready queue.
        }
    }

    /// Unblock a thread that was waiting for a child to exit
    /// Called when a child process terminates
    ///
    /// NOTE: This function sets the need_resched flag when a thread is successfully
    /// unblocked to ensure it gets scheduled promptly. This is critical for waitpid()
    /// to wake up in a timely manner when a child exits.
    pub fn unblock_for_child_exit(&mut self, thread_id: u64) {
        if let Some(thread) = self.get_thread_mut(thread_id) {
            if thread.state == ThreadState::BlockedOnChildExit {
                thread.set_ready();

                // SMP safety: Don't add to ready_queue if thread is current on any CPU
                // (same rationale as unblock() - prevents double-scheduling)
                let is_current_on_any_cpu = (0..MAX_CPUS).any(|cpu| {
                    self.cpu_state[cpu].current_thread == Some(thread_id)
                });

                if !is_current_on_any_cpu
                    && thread_id != self.cpu_state[Self::current_cpu_id()].idle_thread
                    && !self.ready_queue.contains(&thread_id)
                {
                    self.ready_queue.push_back(thread_id);
                    // CRITICAL: Only log on x86_64 to avoid deadlock on ARM64
                    #[cfg(target_arch = "x86_64")]
                    log_serial_println!("Thread {} unblocked by child exit", thread_id);

                    // Send IPI to wake an idle CPU
                    #[cfg(target_arch = "aarch64")]
                    self.send_resched_ipi();
                }
                // CRITICAL: Request reschedule so the unblocked thread can run promptly.
                // Without this, the thread is added to ready queue but the scheduler
                // doesn't know to switch to it, causing waitpid() to hang.
                set_need_resched();
            }
        }
    }

    #[allow(dead_code)] // Will be used when voluntary preemption from syscall handlers is implemented
    /// Block current thread until a timer expires (nanosleep syscall)
    pub fn block_current_for_timer(&mut self, wake_time_ns: u64) {
        if let Some(current_id) = self.cpu_state[Self::current_cpu_id()].current_thread {
            if let Some(thread) = self.get_thread_mut(current_id) {
                thread.state = ThreadState::BlockedOnTimer;
                thread.wake_time_ns = Some(wake_time_ns);
                thread.blocked_in_syscall = true;
            }
            self.ready_queue.retain(|&id| id != current_id);
        }
    }

    /// Check all threads for expired timer-based sleep and wake them.
    /// Called from schedule() on every reschedule.
    fn wake_expired_timers(&mut self) {
        let (secs, nanos) = crate::time::get_monotonic_time_ns();
        let now_ns = secs as u64 * 1_000_000_000 + nanos as u64;

        let mut to_wake = alloc::vec::Vec::new();
        for thread in self.threads.iter() {
            if thread.state == ThreadState::BlockedOnTimer {
                if let Some(wake_time) = thread.wake_time_ns {
                    if now_ns >= wake_time {
                        to_wake.push(thread.id());
                    }
                }
            }
        }

        for id in to_wake {
            if let Some(thread) = self.get_thread_mut(id) {
                thread.state = ThreadState::Ready;
                thread.wake_time_ns = None;
                if id != self.cpu_state[Self::current_cpu_id()].idle_thread && !self.ready_queue.contains(&id) {
                    self.ready_queue.push_back(id);
                }
            }
        }
    }

    /// Terminate the current thread
    #[allow(dead_code)]
    pub fn terminate_current(&mut self) {
        if let Some(current) = self.current_thread_mut() {
            current.set_terminated();
            // Don't put back in ready queue
        }
        self.cpu_state[Self::current_cpu_id()].current_thread = None;
    }

    /// Check if scheduler has any runnable threads
    pub fn has_runnable_threads(&self) -> bool {
        !self.ready_queue.is_empty()
            || self.cpu_state[Self::current_cpu_id()].current_thread.map_or(false, |id| {
                self.get_thread(id).map_or(false, |t| t.is_runnable())
            })
    }

    /// Check if scheduler has any userspace threads (ready, running, or blocked)
    pub fn has_userspace_threads(&self) -> bool {
        self.threads.iter().any(|t| {
            // Exclude all idle threads (one per CPU)
            !self.cpu_state.iter().any(|cs| cs.idle_thread == t.id())
                && t.privilege == super::thread::ThreadPrivilege::User
                && t.state != super::thread::ThreadState::Terminated
        })
    }

    /// Remove a thread from the ready queue (used when blocking)
    pub fn remove_from_ready_queue(&mut self, thread_id: u64) {
        self.ready_queue.retain(|&id| id != thread_id);
    }

    /// Get a thread by ID (public for timer.rs)
    pub fn get_thread(&self, id: u64) -> Option<&Thread> {
        self.threads
            .iter()
            .find(|t| t.id() == id)
            .map(|t| t.as_ref())
    }

    /// Get the idle thread ID
    pub fn idle_thread(&self) -> u64 {
        self.cpu_state[Self::current_cpu_id()].idle_thread
    }

    /// Get the current thread ID for a specific CPU (for diagnostics).
    /// Used by ARM64 exception handler to dump per-CPU state on crash.
    #[cfg(target_arch = "aarch64")]
    pub fn current_thread_for_cpu(&self, cpu: usize) -> Option<u64> {
        if cpu < MAX_CPUS {
            self.cpu_state[cpu].current_thread
        } else {
            None
        }
    }

    /// Set the current thread (used by spawn mechanism)
    #[allow(dead_code)]
    pub fn set_current_thread(&mut self, thread_id: u64) {
        self.cpu_state[Self::current_cpu_id()].current_thread = Some(thread_id);
    }
}

/// Initialize the global scheduler
#[allow(dead_code)]
pub fn init(idle_thread: Box<Thread>) {
    let mut scheduler_lock = SCHEDULER.lock();
    *scheduler_lock = Some(Scheduler::new(idle_thread));
    // CRITICAL: Only log on x86_64 to avoid deadlock on ARM64
    #[cfg(target_arch = "x86_64")]
    log_serial_println!("Scheduler initialized");
}

/// Initialize scheduler with the current thread as the idle task (Linux-style)
/// This is used during boot where the boot thread becomes the idle task
pub fn init_with_current(current_thread: Box<Thread>) {
    let mut scheduler_lock = SCHEDULER.lock();
    let thread_id = current_thread.id();

    // Create scheduler with current thread as both idle and current
    let mut scheduler = Scheduler::new(current_thread);
    scheduler.cpu_state[0].current_thread = Some(thread_id);

    *scheduler_lock = Some(scheduler);
    // CRITICAL: Only log on x86_64 to avoid deadlock on ARM64
    #[cfg(target_arch = "x86_64")]
    log_serial_println!("Scheduler initialized with current thread {} as idle task", thread_id);
    #[cfg(not(target_arch = "x86_64"))]
    let _ = thread_id;
}

/// Register an idle thread for a secondary CPU.
/// Called during SMP bringup from secondary_cpu_entry_rust.
#[cfg(target_arch = "aarch64")]
pub fn register_cpu_idle_thread(cpu_id: usize, idle_thread: Box<Thread>) {
    without_interrupts(|| {
        let mut scheduler_lock = SCHEDULER.lock();
        if let Some(scheduler) = scheduler_lock.as_mut() {
            scheduler.register_idle_thread(cpu_id, idle_thread);
        }
    });
}

/// Add a thread to the scheduler
pub fn spawn(thread: Box<Thread>) {
    // Disable interrupts to prevent timer interrupt deadlock
    without_interrupts(|| {
        let mut scheduler_lock = SCHEDULER.lock();
        if let Some(scheduler) = scheduler_lock.as_mut() {
            scheduler.add_thread(thread);
            // Ensure a switch happens ASAP (especially in CI smoke runs)
            NEED_RESCHED.store(true, Ordering::Relaxed);
            // Mirror to per-CPU flag so IRQ-exit path sees it
            #[cfg(target_arch = "x86_64")]
            crate::per_cpu::set_need_resched(true);
            #[cfg(target_arch = "aarch64")]
            crate::per_cpu_aarch64::set_need_resched(true);
        } else {
            panic!("Scheduler not initialized");
        }
    });
}

/// Add a thread to the front of the ready queue.
/// Used for fork children so they run before other queued threads.
pub fn spawn_front(thread: Box<Thread>) {
    without_interrupts(|| {
        let mut scheduler_lock = SCHEDULER.lock();
        if let Some(scheduler) = scheduler_lock.as_mut() {
            scheduler.add_thread_front(thread);
            NEED_RESCHED.store(true, Ordering::Relaxed);
            #[cfg(target_arch = "x86_64")]
            crate::per_cpu::set_need_resched(true);
            #[cfg(target_arch = "aarch64")]
            crate::per_cpu_aarch64::set_need_resched(true);
        } else {
            panic!("Scheduler not initialized");
        }
    });
}

/// Add a thread as the current running thread without scheduling.
///
/// Used when manually starting the first userspace thread (init process).
/// The thread is added to the scheduler's thread list and marked as current,
/// but NOT added to the ready queue and need_resched is NOT set.
/// This allows the thread to run without the scheduler trying to preempt it.
#[allow(dead_code)]
pub fn spawn_as_current(thread: Box<Thread>) {
    without_interrupts(|| {
        let mut scheduler_lock = SCHEDULER.lock();
        if let Some(scheduler) = scheduler_lock.as_mut() {
            scheduler.add_thread_as_current(thread);
            // NOTE: Do NOT set need_resched - we want this thread to run
        } else {
            panic!("Scheduler not initialized");
        }
    });
}

/// Perform scheduling and return threads to switch between
pub fn schedule() -> Option<(u64, u64)> {
    // Check if interrupts are already disabled (i.e., we're in interrupt context)
    let interrupts_were_enabled = are_enabled();

    let result = if interrupts_were_enabled {
        // Normal case: disable interrupts to prevent deadlock
        without_interrupts(|| {
            let mut scheduler_lock = SCHEDULER.lock();
            if let Some(scheduler) = scheduler_lock.as_mut() {
                scheduler.schedule().map(|(old, new)| (old.id(), new.id()))
            } else {
                None
            }
        })
    } else {
        // Already in interrupt context - don't try to disable interrupts again
        let mut scheduler_lock = SCHEDULER.lock();
        if let Some(scheduler) = scheduler_lock.as_mut() {
            scheduler.schedule().map(|(old, new)| (old.id(), new.id()))
        } else {
            None
        }
    };

    result
}

/// Schedule with deferred requeue for ARM64 SMP.
///
/// Like `schedule()`, but does NOT add the old thread to the ready queue.
/// Returns `(old_thread_id, new_thread_id, should_requeue_old)`.
///
/// CRITICAL: The caller MUST call `requeue_old_thread()` after saving the old
/// thread's context. Failure to do so will leak the thread (it will never be
/// scheduled again).
///
/// This prevents the SMP race where another CPU picks up the old thread from
/// the ready queue before its context has been saved, causing:
/// - Double-scheduling (same thread on two CPUs with same kernel stack)
/// - Stale context dispatch (thread runs with initial/outdated register values)
/// - ELR=0 crashes (ERET to address 0 from unsaved elr_el1)
#[cfg(target_arch = "aarch64")]
pub fn schedule_deferred() -> Option<(u64, u64, bool)> {
    schedule_deferred_with_fixup(None)
}

/// Schedule with optional cpu_state fixup for stale idle state.
///
/// When `real_thread_id` is Some(tid), and the current cpu_state says idle,
/// fix cpu_state to tid BEFORE making the scheduling decision. This prevents
/// the TOCTOU race where cpu_state is stale (says idle) but the real user
/// thread is running on this CPU.
///
/// This must run under a single lock hold with schedule_deferred_requeue to
/// prevent another CPU from changing state between the fixup and the decision.
#[cfg(target_arch = "aarch64")]
pub fn schedule_deferred_with_fixup(real_thread_id: Option<u64>) -> Option<(u64, u64, bool)> {
    // Already in interrupt context on ARM64 (called from IRQ handler)
    let mut scheduler_lock = SCHEDULER.lock();
    if let Some(scheduler) = scheduler_lock.as_mut() {
        // Fix stale cpu_state if needed (atomically with the scheduling decision)
        if let Some(real_tid) = real_thread_id {
            let cpu = Scheduler::current_cpu_id();
            let current = scheduler.cpu_state[cpu].current_thread;
            let idle = scheduler.cpu_state[cpu].idle_thread;
            if current == Some(idle) && real_tid != idle {
                record_cpu_state_change(cpu, 1, idle, real_tid);
                scheduler.cpu_state[cpu].current_thread = Some(real_tid);
            }
        }
        scheduler.schedule_deferred_requeue()
    } else {
        None
    }
}

/// Finalize the cpu_state update after saving context.
///
/// CRITICAL: Must be called after save_kernel_context_arm64 / save_userspace_context_arm64
/// and BEFORE requeue_old_thread. This updates cpu_state[cpu].current_thread to the new
/// thread, which:
/// 1. Removes the old thread from is_current_on_any_cpu() protection
/// 2. Allows unblock() on other CPUs to add the old thread to the ready queue
///
/// The ordering is: schedule_deferred -> save_context -> commit_schedule -> requeue_old
#[cfg(target_arch = "aarch64")]
pub fn commit_schedule_after_save(new_thread_id: u64) {
    // Already in interrupt context on ARM64
    let mut scheduler_lock = SCHEDULER.lock();
    if let Some(scheduler) = scheduler_lock.as_mut() {
        scheduler.commit_cpu_state_after_save(new_thread_id);
    }
}

/// Complete the deferred requeue after saving context.
///
/// Must be called after `schedule_deferred()` returns `should_requeue_old = true`
/// and the old thread's context has been fully saved.
#[cfg(target_arch = "aarch64")]
pub fn requeue_old_thread(thread_id: u64) {
    // Already in interrupt context on ARM64
    let mut scheduler_lock = SCHEDULER.lock();
    if let Some(scheduler) = scheduler_lock.as_mut() {
        scheduler.requeue_thread_after_save(thread_id);
    }
}

/// Check if a thread is an idle thread on any CPU.
#[cfg(target_arch = "aarch64")]
pub fn is_idle_thread(thread_id: u64) -> bool {
    let scheduler_lock = SCHEDULER.lock();
    if let Some(scheduler) = scheduler_lock.as_ref() {
        (0..MAX_CPUS).any(|cpu| scheduler.cpu_state[cpu].idle_thread == thread_id)
    } else {
        false
    }
}

/// Special scheduling point called from IRQ exit path
/// This is safe to call from IRQ context when returning to user or idle
#[allow(dead_code)]
pub fn preempt_schedule_irq() {
    // IMPORTANT: This function must NOT call schedule()!
    //
    // The schedule() function updates scheduler.current_thread, but the actual
    // context switch only happens on the assembly IRETQ path. Calling schedule()
    // here would desync scheduler state from reality:
    //   1. Thread A is running
    //   2. preempt_schedule_irq calls schedule(), sets current_thread = B
    //   3. We return through softirq_exit -> irq_exit -> timer ISR -> IRETQ
    //   4. IRETQ returns to thread A's context (no switch happened)
    //   5. Scheduler thinks B is running, but A is actually running
    //   6. Next schedule() saves A's regs to B's context -> corruption
    //
    // Instead, we leave need_resched set. The assembly interrupt return path
    // (check_need_resched_and_switch) will:
    //   1. Check need_resched
    //   2. Call schedule() to decide what to switch to
    //   3. Perform the actual context switch before IRETQ
    //
    // See also: yield_current() which similarly just sets need_resched
    // and the ARCHITECTURAL CONSTRAINT comment near schedule().

    // No-op: Let the assembly IRETQ path handle context switching
}

/// Non-blocking scheduling attempt (for interrupt context). Returns None if lock is busy.
/// Note: Currently unused - the assembly interrupt return path handles scheduling.
/// Kept as part of public API for potential future use in SMP context.
#[allow(dead_code)]
pub fn try_schedule() -> Option<(u64, u64)> {
    // Do not disable interrupts; we only attempt a non-blocking lock here
    if let Some(mut scheduler_lock) = SCHEDULER.try_lock() {
        if let Some(scheduler) = scheduler_lock.as_mut() {
            return scheduler.schedule().map(|(old, new)| (old.id(), new.id()));
        }
    }
    None
}

/// Check if the current thread is the idle thread (safe to call from IRQ context)
/// Returns None if the scheduler lock can't be acquired (to avoid deadlock)
#[allow(dead_code)]
pub fn is_current_idle_thread() -> Option<bool> {
    // Try to get the lock without blocking - if we can't, assume not idle
    // to be safe. This prevents deadlock when timer fires during scheduler ops.
    if let Some(scheduler_lock) = SCHEDULER.try_lock() {
        if let Some(scheduler) = scheduler_lock.as_ref() {
            return Some(
                scheduler
                    .current_thread_id_inner()
                    .map(|id| id == scheduler.idle_thread_id())
                    .unwrap_or(false),
            );
        }
    }
    None
}

/// Get access to the scheduler
/// This function disables interrupts to prevent deadlock with timer interrupt
pub fn with_scheduler<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut Scheduler) -> R,
{
    without_interrupts(|| {
        let mut scheduler_lock = SCHEDULER.lock();
        scheduler_lock.as_mut().map(f)
    })
}

/// Get mutable access to a specific thread (for timer interrupt handler)
/// This function disables interrupts to prevent deadlock with timer interrupt
pub fn with_thread_mut<F, R>(thread_id: u64, f: F) -> Option<R>
where
    F: FnOnce(&mut super::thread::Thread) -> R,
{
    without_interrupts(|| {
        let mut scheduler_lock = SCHEDULER.lock();
        scheduler_lock
            .as_mut()
            .and_then(|sched| sched.get_thread_mut(thread_id).map(f))
    })
}

/// Get per-process accumulated CPU ticks from all threads in the scheduler.
///
/// Returns a Vec of (owner_pid, cpu_ticks_total) for each thread that has an
/// owner_pid set. For currently-running threads, includes the in-flight ticks
/// since their last schedule (now - run_start_ticks).
///
/// Used by btop monitor to display CPU% per process.
pub fn get_process_cpu_ticks() -> alloc::vec::Vec<(u64, u64)> {
    without_interrupts(|| {
        if let Some(scheduler_lock) = SCHEDULER.try_lock() {
            if let Some(scheduler) = scheduler_lock.as_ref() {
                let now = crate::time::get_ticks();
                return scheduler
                    .threads
                    .iter()
                    .filter_map(|t| {
                        t.owner_pid.map(|pid| {
                            let mut ticks = t.cpu_ticks_total;
                            // If thread is currently running, add in-flight ticks
                            if t.state == super::thread::ThreadState::Running {
                                ticks += now.wrapping_sub(t.run_start_ticks);
                            }
                            (pid, ticks)
                        })
                    })
                    .collect();
            }
        }
        alloc::vec::Vec::new()
    })
}

/// Get the current thread ID
/// This function disables interrupts to prevent deadlock with timer interrupt
pub fn current_thread_id() -> Option<u64> {
    without_interrupts(|| {
        let scheduler_lock = SCHEDULER.lock();
        scheduler_lock.as_ref().and_then(|s| s.cpu_state[Scheduler::current_cpu_id()].current_thread)
    })
}

/// Set the current thread ID
/// Used during boot to establish the initial userspace thread as current
/// before jumping to userspace.
#[allow(dead_code)]
pub fn set_current_thread(thread_id: u64) {
    without_interrupts(|| {
        let mut scheduler_lock = SCHEDULER.lock();
        if let Some(scheduler) = scheduler_lock.as_mut() {
            scheduler.set_current_thread(thread_id);
        }
    });
}

/// Yield the current thread
pub fn yield_current() {
    // CRITICAL FIX: Do NOT call schedule() here!
    // schedule() updates self.cpu_state[Self::current_cpu_id()].current_thread, but no actual context switch happens.
    // This caused the scheduler to get out of sync with reality:
    //   1. Thread A is running
    //   2. yield_current() calls schedule(), returns (A, B), sets current_thread = B
    //   3. No actual context switch - thread A continues running
    //   4. Timer fires, schedule() returns (B, C), saves thread A's regs to thread B's context
    //   5. Thread B's context is now corrupted with thread A's registers
    //
    // Instead, just set need_resched flag. The actual scheduling decision and context
    // switch will happen at the next interrupt return via check_need_resched_and_switch.
    set_need_resched();
}

// NOTE: get_pending_switch() was removed because it called schedule() which mutates
// self.cpu_state[Self::current_cpu_id()].current_thread. Calling it "just to peek" would corrupt scheduler state.
// If needed in future, implement a true peek function that doesn't mutate state.
//
// ARCHITECTURAL CONSTRAINT: Never add a function that calls schedule() "just to look"
// at what would happen. The schedule() function MUST only be called when an actual
// context switch will follow immediately. Violating this invariant will desync
// scheduler.current_thread from reality, causing register corruption in child processes.
// See commit f59bccd for the full bug investigation.

/// Allocate a new thread ID
#[allow(dead_code)]
pub fn allocate_thread_id() -> Option<u64> {
    Some(super::thread::allocate_thread_id())
}

/// Set the need_resched flag (called from timer interrupt)
pub fn set_need_resched() {
    NEED_RESCHED.store(true, Ordering::Relaxed);
    #[cfg(target_arch = "x86_64")]
    crate::per_cpu::set_need_resched(true);
    #[cfg(target_arch = "aarch64")]
    crate::per_cpu_aarch64::set_need_resched(true);
}

/// Check and clear the need_resched flag (called from interrupt return path)
pub fn check_and_clear_need_resched() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        let need = crate::per_cpu::need_resched();
        if need { crate::per_cpu::set_need_resched(false); }
        let _ = NEED_RESCHED.swap(false, Ordering::Relaxed);
        need
    }
    #[cfg(target_arch = "aarch64")]
    {
        // ARM64: Check per-CPU flag and global atomic
        let need = crate::per_cpu_aarch64::need_resched();
        if need {
            crate::per_cpu_aarch64::set_need_resched(false);
        }
        let _ = NEED_RESCHED.swap(false, Ordering::Relaxed);
        need
    }
}

/// Check if the need_resched flag is set (without clearing it)
/// Used by can_schedule() to determine if kernel threads should be rescheduled
pub fn is_need_resched() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        crate::per_cpu::need_resched() || NEED_RESCHED.load(Ordering::Relaxed)
    }
    #[cfg(target_arch = "aarch64")]
    {
        // ARM64: Check per-CPU flag and global atomic
        crate::per_cpu_aarch64::need_resched() || NEED_RESCHED.load(Ordering::Relaxed)
    }
}

/// Switch to idle thread immediately (for use by exception handlers)
/// This updates scheduler state so subsequent timer interrupts can properly schedule.
/// Call this before modifying exception frame to return to idle_loop.
pub fn switch_to_idle() {
    with_scheduler(|sched| {
        let cpu_id = Scheduler::current_cpu_id();
        let idle_id = sched.cpu_state[cpu_id].idle_thread;
        let old_val = sched.cpu_state[cpu_id].current_thread.unwrap_or(0xDEAD);
        #[cfg(target_arch = "aarch64")]
        record_cpu_state_change(cpu_id, 2, old_val, idle_id);
        let _ = old_val; // suppress unused warning on non-aarch64
        sched.cpu_state[cpu_id].current_thread = Some(idle_id);

        // Also update per-CPU current thread pointer
        #[cfg(target_arch = "x86_64")]
        if let Some(thread) = sched.get_thread_mut(idle_id) {
            let thread_ptr = thread as *const _ as *mut crate::task::thread::Thread;
            crate::per_cpu::set_current_thread(thread_ptr);
            log::info!(
                "Exception handler: Set per_cpu thread to idle {} at {:p}",
                idle_id, thread_ptr
            );
        } else {
            log::error!("Exception handler: Failed to get idle thread {} from scheduler!", idle_id);
        }

        #[cfg(target_arch = "x86_64")]
        log::info!("Exception handler: Switched scheduler to idle thread {}", idle_id);
    });
}

/// Best-effort switch to idle — uses try_lock to avoid deadlock in crash handlers.
///
/// When an INSTRUCTION_ABORT or DATA_ABORT occurs from EL1, the SCHEDULER lock
/// may already be held (e.g., the crash happened during a context switch). Using
/// `switch_to_idle()` would deadlock on the same CPU. This version uses try_lock:
/// if the lock is available, update scheduler state; if not, just return — the
/// next timer interrupt on this CPU will see the idle loop and correct the state.
#[cfg(target_arch = "aarch64")]
pub fn switch_to_idle_best_effort() {
    if let Some(mut scheduler_lock) = SCHEDULER.try_lock() {
        if let Some(sched) = scheduler_lock.as_mut() {
            let cpu_id = Scheduler::current_cpu_id();
            let idle_id = sched.cpu_state[cpu_id].idle_thread;
            let old_val = sched.cpu_state[cpu_id].current_thread.unwrap_or(0xDEAD);
            record_cpu_state_change(cpu_id, 3, old_val, idle_id);
            sched.cpu_state[cpu_id].current_thread = Some(idle_id);
        }
    }
    // If try_lock fails, the scheduler state will be stale, but the CPU
    // will be executing idle_loop_arm64 which only does WFI. The next
    // timer-driven schedule() call will see the idle thread running and
    // correct the state.
}

/// Test module for scheduler state invariants
/// These tests use x86_64-specific types (VirtAddr) and are only compiled for x86_64
#[cfg(all(test, target_arch = "x86_64"))]
pub mod tests {
    use super::*;
    use alloc::boxed::Box;
    use alloc::string::String;
    use crate::task::thread::{Thread, ThreadPrivilege, ThreadState};
    use x86_64::VirtAddr;

    fn dummy_entry() {}

    fn make_thread(id: u64, state: ThreadState) -> Box<Thread> {
        let mut thread = Thread::new_with_id(
            id,
            String::from("scheduler-test-thread"),
            dummy_entry,
            VirtAddr::new(0x2000),
            VirtAddr::new(0x1000),
            VirtAddr::new(0),
            ThreadPrivilege::Kernel,
        );
        thread.state = state;
        Box::new(thread)
    }

    pub fn test_unblock_does_not_duplicate_ready_queue() {
        log::info!("=== TEST: unblock avoids duplicate ready_queue entries ===");

        let idle_thread = make_thread(1, ThreadState::Ready);
        let mut scheduler = Scheduler::new(idle_thread);

        let blocked_thread_id = 2;
        let blocked_thread = make_thread(blocked_thread_id, ThreadState::Blocked);
        scheduler.add_thread(blocked_thread);
        if let Some(thread) = scheduler.get_thread_mut(blocked_thread_id) {
            thread.state = ThreadState::Blocked;
        }
        scheduler.remove_from_ready_queue(blocked_thread_id);

        scheduler.unblock(blocked_thread_id);
        scheduler.unblock(blocked_thread_id);

        let count = scheduler
            .ready_queue
            .iter()
            .filter(|&&id| id == blocked_thread_id)
            .count();
        assert_eq!(count, 1);

        log::info!("=== TEST PASSED: unblock avoids duplicate ready_queue entries ===");
    }

    pub fn test_schedule_does_not_duplicate_ready_queue() {
        log::info!("=== TEST: schedule avoids duplicate ready_queue entries ===");

        let idle_thread = make_thread(1, ThreadState::Ready);
        let mut scheduler = Scheduler::new(idle_thread);

        let current_thread_id = 2;
        let current_thread = make_thread(current_thread_id, ThreadState::Running);
        scheduler.add_thread(current_thread);

        let other_thread_id = 3;
        let other_thread = make_thread(other_thread_id, ThreadState::Ready);
        scheduler.add_thread(other_thread);

        scheduler.cpu_state[0].current_thread = Some(current_thread_id);
        if let Some(thread) = scheduler.get_thread_mut(current_thread_id) {
            thread.state = ThreadState::Running;
        }

        let scheduled = scheduler.schedule();
        assert_eq!(scheduled.is_some(), true);

        let count = scheduler
            .ready_queue
            .iter()
            .filter(|&&id| id == current_thread_id)
            .count();
        assert_eq!(count, 1);

        log::info!("=== TEST PASSED: schedule avoids duplicate ready_queue entries ===");
    }

    /// Test that yield_current() does NOT modify scheduler.current_thread.
    ///
    /// This test validates the fix for the bug where yield_current() called schedule(),
    /// which updated self.cpu_state[Self::current_cpu_id()].current_thread without an actual context switch occurring.
    /// This caused scheduler state to desync from reality, corrupting child process
    /// register state during fork.
    ///
    /// The fix changed yield_current() to only set the need_resched flag, deferring
    /// the actual scheduling decision to the next interrupt return.
    pub fn test_yield_current_does_not_modify_scheduler_state() {
        log::info!("=== TEST: yield_current() scheduler state invariant ===");

        // Capture the current thread ID before yield
        let thread_id_before = current_thread_id();
        log::info!("Thread ID before yield_current(): {:?}", thread_id_before);

        // Call yield_current() - this should ONLY set need_resched flag
        yield_current();

        // Capture the current thread ID after yield
        let thread_id_after = current_thread_id();
        log::info!("Thread ID after yield_current(): {:?}", thread_id_after);

        // CRITICAL ASSERTION: current_thread should NOT have changed
        // If this fails, it means yield_current() is calling schedule() which
        // would cause the register corruption bug to return.
        assert_eq!(
            thread_id_before, thread_id_after,
            "BUG: yield_current() modified scheduler.current_thread! \
             This will cause fork to corrupt child registers. \
             yield_current() must ONLY set need_resched flag, not call schedule()."
        );

        // Verify that need_resched was set
        let need_resched = crate::per_cpu::need_resched();
        assert!(
            need_resched,
            "yield_current() should have set the need_resched flag"
        );

        // Clean up: clear the need_resched flag to avoid affecting other tests
        crate::per_cpu::set_need_resched(false);

        log::info!("=== TEST PASSED: yield_current() correctly preserves scheduler state ===");
    }
}

/// Public wrapper for running scheduler tests (callable from kernel main)
/// This is intentionally available but not automatically called - it can be
/// invoked manually during debugging to verify scheduler invariants.
/// Only available on x86_64 since tests use architecture-specific types.
#[cfg(target_arch = "x86_64")]
#[allow(dead_code)]
pub fn run_scheduler_tests() {
    #[cfg(test)]
    {
        tests::test_yield_current_does_not_modify_scheduler_state();
    }
    #[cfg(not(test))]
    {
        // In non-test builds, run a simplified version that doesn't use assert
        log::info!("=== Scheduler invariant check (non-test mode) ===");

        let thread_id_before = current_thread_id();
        yield_current();
        let thread_id_after = current_thread_id();

        if thread_id_before != thread_id_after {
            log::error!(
                "SCHEDULER BUG: yield_current() changed current_thread from {:?} to {:?}!",
                thread_id_before, thread_id_after
            );
        } else {
            log::info!("Scheduler invariant check passed: yield_current() preserves state");
        }

        // Clean up
        crate::per_cpu::set_need_resched(false);
    }
}

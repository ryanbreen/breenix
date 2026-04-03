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
use alloc::{boxed::Box, collections::BinaryHeap, collections::VecDeque};
use core::cmp::Reverse;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use spin::Mutex;

// Architecture-generic HAL wrappers for interrupt control.
#[cfg(not(target_arch = "aarch64"))]
use crate::arch_interrupts_enabled as are_enabled;
use crate::arch_without_interrupts as without_interrupts;

// ---------------------------------------------------------------------------
// Lock-free ISR wakeup buffer
//
// The AHCI ISR calls `isr_unblock_for_io(tid)` which writes the thread ID
// into a per-CPU slot array using atomic CAS — no lock, no allocation.
// The scheduler drains these buffers under its own lock at the top of every
// `schedule_deferred_requeue()` / `schedule()` call, performing the actual
// state transition + queue push.
//
// This breaks the ISR's dependency on the global SCHEDULER mutex, which was
// the root cause of CPU 0's IRQ death: the AHCI ISR on CPU 0 would spin on
// the lock (held by another CPU) with IRQs masked, starving the timer.
// ---------------------------------------------------------------------------

const ISR_WAKEUP_SLOTS: usize = 32;
const ISR_WAKEUP_EMPTY: u64 = 0;

/// Per-CPU lock-free buffer for ISR wakeups.
/// ISR writes thread IDs here via atomic CAS. Scheduler drains on each schedule.
struct IsrWakeupBuffer {
    slots: [AtomicU64; ISR_WAKEUP_SLOTS],
}

// SAFETY: All access is via atomics.
unsafe impl Sync for IsrWakeupBuffer {}

impl IsrWakeupBuffer {
    const fn new() -> Self {
        Self {
            slots: [const { AtomicU64::new(ISR_WAKEUP_EMPTY) }; ISR_WAKEUP_SLOTS],
        }
    }

    /// Push a thread ID (called from ISR context, no locks).
    /// Returns true if pushed, false if buffer full.
    fn push(&self, tid: u64) -> bool {
        for slot in &self.slots {
            if slot
                .compare_exchange(
                    ISR_WAKEUP_EMPTY,
                    tid,
                    Ordering::AcqRel,
                    Ordering::Relaxed,
                )
                .is_ok()
            {
                return true;
            }
        }
        false // Buffer full — should never happen with 32 slots
    }

    /// Drain all entries (called from scheduler under lock).
    fn drain(&self, out: &mut alloc::vec::Vec<u64>) {
        for slot in &self.slots {
            let tid = slot.swap(ISR_WAKEUP_EMPTY, Ordering::AcqRel);
            if tid != ISR_WAKEUP_EMPTY {
                out.push(tid);
            }
        }
    }
}

static ISR_WAKEUP_BUFFERS: [IsrWakeupBuffer; 8] =
    [const { IsrWakeupBuffer::new() }; 8];

/// Global scheduler instance
static SCHEDULER: Mutex<Option<Scheduler>> = Mutex::new(None);

/// Global need_resched flag for timer interrupt
static NEED_RESCHED: AtomicBool = AtomicBool::new(false);

/// Global context switch counter - incremented on every successful context switch.
/// Used by the soft lockup detector to detect CPU stalls.
static CONTEXT_SWITCH_COUNT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Per-CPU "is idle" flags. Set to true when a CPU is running its idle thread,
/// false when running a real thread. Updated lock-free during scheduling
/// decisions. Used by the timer interrupt handler to always request reschedule
/// on idle CPUs, ensuring threads added to the ready queue are picked up
/// within one timer tick (~5ms) instead of waiting for quantum expiry (~50ms).
///
/// IMPORTANT: Initialized to false (not idle). CPU 0 is the boot CPU and
/// starts running init — it must NOT be marked idle. Secondary CPUs will be
/// marked idle when they enter their idle loops and the first scheduling
/// decision runs. This prevents the timer handler from falsely setting
/// need_resched on every tick for CPUs that are actually running real work.
#[cfg(target_arch = "aarch64")]
static CPU_IS_IDLE: [AtomicBool; MAX_CPUS] = [
    AtomicBool::new(false),
    AtomicBool::new(false),
    AtomicBool::new(false),
    AtomicBool::new(false),
    AtomicBool::new(false),
    AtomicBool::new(false),
    AtomicBool::new(false),
    AtomicBool::new(false),
];

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

/// Get the global context switch count (for soft lockup detection).
/// This is lock-free and safe to call from interrupt context.
pub fn context_switch_count() -> u64 {
    CONTEXT_SWITCH_COUNT.load(core::sync::atomic::Ordering::Relaxed)
}

/// Increment the global context switch count.
/// Called from the ARM64 context switch path (context_switch.rs) where the
/// actual switch happens outside of schedule_deferred_requeue().
/// On x86_64, the count is incremented inside schedule() directly.
#[cfg(target_arch = "aarch64")]
pub fn increment_context_switch_count() {
    CONTEXT_SWITCH_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
}

/// Acquire the scheduler lock for a full context switch operation.
///
/// Returns the raw lock guard, allowing the caller to perform all
/// context switch operations under a single lock hold. This eliminates
/// TOCTOU races from separate lock acquisitions.
///
/// SAFETY: Must be called from interrupt context (interrupts already disabled).
/// The caller must not call any other scheduler public functions that acquire
/// SCHEDULER.lock() while holding this guard (would deadlock).
#[cfg(target_arch = "aarch64")]
pub fn lock_for_context_switch() -> spin::MutexGuard<'static, Option<Scheduler>> {
    SCHEDULER.lock()
}

/// Force-unlock the scheduler mutex after an inline AArch64 context switch.
///
/// The inline scheduler path intentionally leaks the lock guard before hopping
/// to a per-CPU scheduler stack, then releases the lock from that neutral
/// stack after the outgoing thread is fully off CPU.
#[cfg(target_arch = "aarch64")]
pub unsafe fn force_unlock_scheduler() {
    SCHEDULER.force_unlock();
}

/// Check if a specific CPU is running its idle thread (lock-free).
/// Safe to call from interrupt context (timer handler).
#[cfg(target_arch = "aarch64")]
pub fn is_cpu_idle(cpu_id: usize) -> bool {
    cpu_id < MAX_CPUS && CPU_IS_IDLE[cpu_id].load(Ordering::Relaxed)
}

/// Mark a CPU as idle or non-idle (lock-free).
/// Called from the scheduling decision path.
#[cfg(target_arch = "aarch64")]
fn set_cpu_idle(cpu_id: usize, idle: bool) {
    if cpu_id < MAX_CPUS {
        CPU_IS_IDLE[cpu_id].store(idle, Ordering::Relaxed);
    }
}

/// Per-thread diagnostic entry for soft lockup dump.
pub struct ThreadDumpEntry {
    pub id: u64,
    pub state: u8, // 0=Ready,1=Running,2=Blocked,3=BlockedOnSignal,4=BlockedOnChildExit,5=BlockedOnTimer,6=Terminated
    pub blocked_in_syscall: bool,
    pub saved_by_inline_schedule: bool,
    pub inline_schedule_caller_lr: u64,
    pub inline_schedule_saved_sp: u64,
    pub has_wake_time: bool,
    pub privilege: u8, // 0=Kernel, 1=User
    pub owner_pid: u64,
    pub elr_el1: u64,
    pub x30: u64,
    pub sp: u64,
}

/// Diagnostic snapshot of scheduler state for the soft lockup detector.
pub struct SchedulerDumpInfo {
    pub current_thread_id: u64,
    pub ready_queue_len: u64,
    pub total_threads: u64,
    pub blocked_count: u64,
    pub per_cpu_current: [u64; 8],  // current_thread per CPU (0 = none)
    pub per_cpu_previous: [u64; 8], // previous_thread per CPU (0 = none)
    pub threads: alloc::vec::Vec<ThreadDumpEntry>,
    pub ready_queue_ids: alloc::vec::Vec<u64>,
}

/// Try to get a snapshot of scheduler state without blocking.
/// Returns None if the scheduler lock is held (which is itself diagnostic).
/// Safe to call from interrupt context.
pub fn try_dump_state() -> Option<SchedulerDumpInfo> {
    let guard = SCHEDULER.try_lock()?;
    let sched = guard.as_ref()?;

    let current_thread_id = sched.cpu_state[0].current_thread.unwrap_or(0);
    let ready_queue_len = sched.per_cpu_queues.iter().map(|q| q.len()).sum::<usize>() as u64;
    let total_threads = sched.threads.len() as u64;
    let blocked_count = sched
        .threads
        .iter()
        .filter(|t| {
            matches!(
                t.state,
                ThreadState::Blocked
                    | ThreadState::BlockedOnSignal
                    | ThreadState::BlockedOnChildExit
                    | ThreadState::BlockedOnTimer
                    | ThreadState::BlockedOnIO
            )
        })
        .count() as u64;

    let mut per_cpu_current = [0u64; 8];
    let mut per_cpu_previous = [0u64; 8];
    for cpu in 0..MAX_CPUS.min(8) {
        per_cpu_current[cpu] = sched.cpu_state[cpu].current_thread.unwrap_or(0);
        per_cpu_previous[cpu] = sched.cpu_state[cpu].previous_thread.unwrap_or(0);
    }

    let threads: alloc::vec::Vec<ThreadDumpEntry> = sched
        .threads
        .iter()
        .map(|t| {
            #[cfg(target_arch = "aarch64")]
            let (elr_el1, x30, sp) = (t.context.elr_el1, t.context.x30, t.context.sp);

            #[cfg(not(target_arch = "aarch64"))]
            let (elr_el1, x30, sp) = (0, 0, t.context.rsp);

            ThreadDumpEntry {
            id: t.id(),
            state: match t.state {
                ThreadState::Ready => 0,
                ThreadState::Running => 1,
                ThreadState::Blocked => 2,
                ThreadState::BlockedOnSignal => 3,
                ThreadState::BlockedOnChildExit => 4,
                ThreadState::BlockedOnTimer => 5,
                ThreadState::BlockedOnIO => 7,
                ThreadState::Terminated => 6,
            },
            blocked_in_syscall: t.blocked_in_syscall,
            saved_by_inline_schedule: t.saved_by_inline_schedule,
            inline_schedule_caller_lr: t.inline_schedule_caller_lr,
            inline_schedule_saved_sp: t.inline_schedule_saved_sp,
            has_wake_time: t.wake_time_ns.is_some(),
            privilege: if t.privilege == super::thread::ThreadPrivilege::Kernel {
                0
            } else {
                1
            },
            owner_pid: t.owner_pid.unwrap_or(0),
            elr_el1,
            x30,
            sp,
        }
        })
        .collect();

    let ready_queue_ids: alloc::vec::Vec<u64> = sched
        .per_cpu_queues
        .iter()
        .flat_map(|q| q.iter().copied())
        .collect();

    Some(SchedulerDumpInfo {
        current_thread_id,
        ready_queue_len,
        total_threads,
        blocked_count,
        per_cpu_current,
        per_cpu_previous,
        threads,
        ready_queue_ids,
    })
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
    const INIT_CPU: [core::sync::atomic::AtomicU64; HISTORY_SIZE * 3] =
        [INIT_ENTRY; HISTORY_SIZE * 3];
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
        let idx =
            CPU_STATE_HISTORY_IDX[cpu].fetch_add(1, core::sync::atomic::Ordering::Relaxed) as usize;
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
    use crate::arch_impl::aarch64::context_switch::{raw_uart_dec, raw_uart_str};
    if cpu >= MAX_CPUS {
        return;
    }
    let total = CPU_STATE_HISTORY_IDX[cpu].load(core::sync::atomic::Ordering::Relaxed) as usize;
    let count = if total < HISTORY_SIZE {
        total
    } else {
        HISTORY_SIZE
    };
    let start = if total < HISTORY_SIZE {
        0
    } else {
        total - HISTORY_SIZE
    };
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
pub(crate) struct CpuSchedulerState {
    /// Currently running thread ID on this CPU
    pub(crate) current_thread: Option<u64>,
    /// Idle thread ID for this CPU
    pub(crate) idle_thread: u64,
    /// Thread that was just switched out on this CPU.
    ///
    /// After a context switch, the old thread's kernel stack is still in use
    /// by this CPU until ERET completes (post-switch code runs on the old
    /// thread's stack). This field prevents wakeup paths (unblock, wake_expired_timers)
    /// from adding the thread to the ready_queue too early — which would allow
    /// another CPU to dispatch it while this CPU still has stack frames on
    /// the same kernel stack, causing register/stack corruption.
    ///
    /// Set when committing a context switch, cleared when processing the
    /// deferred requeue on the NEXT context switch (by which time ERET has
    /// completed and the stack is free).
    #[cfg_attr(not(target_arch = "aarch64"), allow(dead_code))]
    pub(crate) previous_thread: Option<u64>,
}

/// The kernel scheduler
pub struct Scheduler {
    /// All threads in the system
    threads: alloc::vec::Vec<Box<Thread>>,

    /// Per-CPU ready queues — each CPU pops from its own queue; work-stealing
    /// falls back to other CPUs' queues when the local queue is empty.
    per_cpu_queues: [VecDeque<u64>; MAX_CPUS],

    /// Per-CPU scheduler state (current_thread + idle_thread per CPU)
    pub(crate) cpu_state: [CpuSchedulerState; MAX_CPUS],

    /// Min-heap of (wake_time_ns, thread_id) for timer-blocked threads.
    /// Replaces O(N) scan in wake_expired_timers with O(log N) insert + O(1) peek.
    /// Stale entries (threads already woken by ISR or terminated) are harmless —
    /// wake_expired_timers validates each entry before acting on it.
    timer_heap: BinaryHeap<Reverse<(u64, u64)>>,
}

impl Scheduler {
    /// Create a new scheduler with an idle thread for CPU 0.
    pub fn new(idle_thread: Box<Thread>) -> Self {
        let idle_id = idle_thread.id();

        // Initialize all CPU states: CPU 0 gets the idle thread, rest are empty
        const EMPTY_STATE: CpuSchedulerState = CpuSchedulerState {
            current_thread: None,
            idle_thread: 0,
            previous_thread: None,
        };
        let mut cpu_state = [EMPTY_STATE; MAX_CPUS];
        cpu_state[0] = CpuSchedulerState {
            current_thread: Some(idle_id),
            idle_thread: idle_id,
            previous_thread: None,
        };

        // VecDeque::new() is not const, so we initialise via a helper array.
        // Each element is an independent empty deque; no const generic is needed.
        let per_cpu_queues = {
            // Build an array of MAX_CPUS VecDeques without requiring Copy or const.
            let mut arr: [core::mem::MaybeUninit<VecDeque<u64>>; MAX_CPUS] =
                unsafe { core::mem::MaybeUninit::uninit().assume_init() };
            for slot in arr.iter_mut() {
                slot.write(VecDeque::new());
            }
            unsafe { core::mem::transmute::<_, [VecDeque<u64>; MAX_CPUS]>(arr) }
        };

        let scheduler = Self {
            threads: alloc::vec![idle_thread],
            per_cpu_queues,
            cpu_state,
            timer_heap: BinaryHeap::new(),
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
        // Route to least-loaded CPU queue (or current CPU if tied).
        let target = self.least_loaded_cpu();
        if front {
            self.per_cpu_queues[target].push_front(thread_id);
        } else {
            self.per_cpu_queues[target].push_back(thread_id);
        }
        // CRITICAL: Only log on x86_64. On ARM64, log_serial_println! uses the same
        // SERIAL1 lock as serial_println!, causing deadlock if timer fires while
        // boot code is printing.
        #[cfg(target_arch = "x86_64")]
        log_serial_println!(
            "Added thread {} '{}' to scheduler (user: {}, target_cpu: {})",
            thread_id,
            thread_name,
            is_user,
            target
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
        self.cpu_state[Self::current_cpu_id()]
            .current_thread
            .and_then(|id| self.get_thread(id))
    }

    /// Get the current running thread mutably
    pub fn current_thread_mut(&mut self) -> Option<&mut Thread> {
        self.cpu_state[Self::current_cpu_id()]
            .current_thread
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

        // Drain lock-free ISR wakeup buffers (see schedule_deferred_requeue for rationale).
        {
            let mut wakeups = alloc::vec::Vec::new();
            for buf in ISR_WAKEUP_BUFFERS.iter() {
                buf.drain(&mut wakeups);
            }
            for tid in wakeups {
                self.unblock_for_io(tid);
            }
        }

        // If current thread is still runnable, put it back in ready queue
        if let Some(current_id) = self.cpu_state[Self::current_cpu_id()].current_thread {
            if current_id != self.cpu_state[Self::current_cpu_id()].idle_thread {
                // Check the state and determine what to do
                let (is_terminated, is_blocked) =
                    if let Some(current) = self.get_thread_mut(current_id) {
                        let was_terminated = current.state == ThreadState::Terminated;
                        // Check for any blocked state
                        let was_blocked = current.state == ThreadState::Blocked
                            || current.state == ThreadState::BlockedOnSignal
                            || current.state == ThreadState::BlockedOnChildExit
                            || current.state == ThreadState::BlockedOnTimer
                            || current.state == ThreadState::BlockedOnIO;

                        // Charge elapsed CPU ticks to the outgoing thread, but ONLY
                        // if it was actually running. Blocked threads already had
                        // their ticks charged at block time — charging again here
                        // would count blocked/sleeping time as CPU usage.
                        if !was_blocked && !was_terminated {
                            let now = crate::time::get_ticks();
                            current.cpu_ticks_total += now.wrapping_sub(current.run_start_ticks);
                            current.run_start_ticks = now;
                            current.set_ready();
                        } else {
                            // Reset run_start_ticks so the next dispatch doesn't
                            // charge stale time from the blocked period.
                            current.run_start_ticks = crate::time::get_ticks();
                        }

                        (was_terminated, was_blocked)
                    } else {
                        (true, false)
                    };

                // Put non-terminated, non-blocked threads back in ready queue
                // CRITICAL: Check for duplicates! If unblock() already added this thread
                // (e.g., packet arrived during blocking recvfrom), don't add it again.
                // Duplicates cause schedule() to spin when same thread keeps getting selected.
                let in_queue = self.per_cpu_queues.iter().any(|q| q.contains(&current_id));
                let will_add = !is_terminated && !is_blocked && !in_queue;

                if will_add {
                    let cpu = Self::current_cpu_id();
                    self.per_cpu_queues[cpu].push_back(current_id);
                }
            }
        }

        // Check for expired timer-blocked threads and wake them
        self.wake_expired_timers();

        // Get next thread from ready queue (local first, then steal), skipping terminated.
        let current_cpu = Self::current_cpu_id();
        let mut next_thread_id = 'outer: loop {
            // Try local queue first
            while let Some(n) = self.per_cpu_queues[current_cpu].pop_front() {
                if let Some(thread) = self.get_thread(n) {
                    if thread.state == ThreadState::Terminated {
                        continue;
                    }
                }
                break 'outer n;
            }
            // Local queue empty — work-steal from other CPUs
            for steal_cpu in 0..MAX_CPUS {
                if steal_cpu == current_cpu {
                    continue;
                }
                while let Some(n) = self.per_cpu_queues[steal_cpu].pop_front() {
                    if let Some(thread) = self.get_thread(n) {
                        if thread.state == ThreadState::Terminated {
                            continue;
                        }
                    }
                    break 'outer n;
                }
            }
            break self.cpu_state[current_cpu].idle_thread;
        };

        if debug_log {
            log_serial_println!(
                "Next thread from queue: {}, cpu: {}",
                next_thread_id,
                current_cpu,
            );
        }

        // Important: Don't skip if it's the same thread when there are other threads waiting
        // This was causing the issue where yielding wouldn't switch to other ready threads
        let any_queued = self.per_cpu_queues.iter().any(|q| !q.is_empty());
        if Some(next_thread_id) == self.cpu_state[current_cpu].current_thread && any_queued {
            // Put current thread back in its CPU queue and get the next one
            self.per_cpu_queues[current_cpu].push_back(next_thread_id);
            // Pop from local queue first; fall back to any CPU
            next_thread_id = {
                let mut found = None;
                if let Some(n) = self.per_cpu_queues[current_cpu].pop_front() {
                    found = Some(n);
                } else {
                    for steal_cpu in 0..MAX_CPUS {
                        if steal_cpu == current_cpu {
                            continue;
                        }
                        if let Some(n) = self.per_cpu_queues[steal_cpu].pop_front() {
                            found = Some(n);
                            break;
                        }
                    }
                }
                found?
            };
        } else if Some(next_thread_id) == self.cpu_state[current_cpu].current_thread {
            // Current thread is the only runnable thread.
            // If it's NOT the idle thread, switch to idle to give it a chance.
            // This is important for kthreads that yield while waiting for the idle
            // thread (which runs tests/main logic) to set a flag.
            if next_thread_id != self.cpu_state[current_cpu].idle_thread {
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
                        // Userspace thread is alone - keep running it, don't switch to idle.
                        // Restore Running state (was set to Ready above).
                        if let Some(t) = self.get_thread_mut(next_thread_id) {
                            t.set_running();
                        }
                        // Remove from per-CPU queue (was pushed above).
                        for q in self.per_cpu_queues.iter_mut() {
                            if let Some(pos) = q.iter().position(|&id| id == next_thread_id) {
                                q.remove(pos);
                                break;
                            }
                        }
                        if debug_log {
                            log_serial_println!(
                                "Thread {} is userspace and alone, continuing (no idle switch)",
                                next_thread_id
                            );
                        }
                        return None;
                    }
                }
                self.per_cpu_queues[current_cpu].push_back(next_thread_id);
                next_thread_id = self.cpu_state[current_cpu].idle_thread;
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
                        self.cpu_state[current_cpu].current_thread.unwrap_or(0),
                        self.cpu_state[current_cpu].idle_thread
                    );
                }
            } else {
                // Idle is the only runnable thread - keep running it.
                // No context switch needed.
                // NOTE: Do NOT push idle to per_cpu_queues here! Idle came from
                // the fallback path, not from pop_front. The queues should remain
                // empty. Pushing idle here would accumulate idle entries.
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
        let old_thread_id = self.cpu_state[current_cpu]
            .current_thread
            .unwrap_or(self.cpu_state[current_cpu].idle_thread);
        self.cpu_state[current_cpu].current_thread = Some(next_thread_id);

        // Track context switches for soft lockup detection
        CONTEXT_SWITCH_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);

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
        // Update per-CPU idle flag based on CURRENT state (before scheduling decision).
        // This ensures the flag is always accurate, even when this function returns None.
        // If we return Some(...), the flag is overwritten with the post-switch state later.
        let cpu = Self::current_cpu_id();
        let current_is_idle =
            self.cpu_state[cpu].current_thread == Some(self.cpu_state[cpu].idle_thread);
        set_cpu_idle(cpu, current_is_idle);

        // Drain lock-free ISR wakeup buffers — ISRs (AHCI, etc.) push thread IDs
        // here via isr_unblock_for_io() to avoid spinning on SCHEDULER from ISR
        // context.  We drain ALL CPUs' buffers because the ISR that completed the
        // I/O may have run on any CPU.
        {
            let mut wakeups = alloc::vec::Vec::new();
            for buf in ISR_WAKEUP_BUFFERS.iter() {
                buf.drain(&mut wakeups);
            }
            for tid in wakeups {
                self.unblock_for_io(tid);
            }
        }

        // If current thread is still runnable, mark it as Ready but DON'T add to queue
        let mut should_requeue_old = false;
        if let Some(current_id) = self.cpu_state[Self::current_cpu_id()].current_thread {
            if current_id != self.cpu_state[Self::current_cpu_id()].idle_thread {
                let (is_terminated, is_blocked) =
                    if let Some(current) = self.get_thread_mut(current_id) {
                        let was_terminated = current.state == ThreadState::Terminated;
                        let was_blocked = current.state == ThreadState::Blocked
                            || current.state == ThreadState::BlockedOnSignal
                            || current.state == ThreadState::BlockedOnChildExit
                            || current.state == ThreadState::BlockedOnTimer
                            || current.state == ThreadState::BlockedOnIO;

                        // Only charge CPU ticks if thread was actually running
                        if !was_blocked && !was_terminated {
                            let now = crate::time::get_ticks();
                            current.cpu_ticks_total += now.wrapping_sub(current.run_start_ticks);
                            current.run_start_ticks = now;
                            current.set_ready();
                        } else {
                            current.run_start_ticks = crate::time::get_ticks();
                        }

                        (was_terminated, was_blocked)
                    } else {
                        (true, false)
                    };

                let in_queue = self.per_cpu_queues.iter().any(|q| q.contains(&current_id));
                // Instead of adding to a queue, just record whether we SHOULD
                should_requeue_old = !is_terminated && !is_blocked && !in_queue;
                // NOTE: We intentionally do NOT push to any queue here.
                // The caller will do so after saving context via requeue_thread_after_save().
            }
        }

        // Check for expired timer-blocked threads and wake them
        self.wake_expired_timers();

        // Get next thread: local queue first, then work-steal, then idle.
        let current_cpu = Self::current_cpu_id();
        let mut next_thread_id = 'sched_outer: loop {
            // Try local queue
            while let Some(n) = self.per_cpu_queues[current_cpu].pop_front() {
                if let Some(thread) = self.get_thread(n) {
                    if thread.state == ThreadState::Terminated {
                        continue;
                    }
                }
                break 'sched_outer n;
            }
            // Work-steal from other CPUs
            for steal_cpu in 0..MAX_CPUS {
                if steal_cpu == current_cpu {
                    continue;
                }
                while let Some(n) = self.per_cpu_queues[steal_cpu].pop_front() {
                    if let Some(thread) = self.get_thread(n) {
                        if thread.state == ThreadState::Terminated {
                            continue;
                        }
                    }
                    break 'sched_outer n;
                }
            }
            // All queues empty — emit diagnostic (rate-limited) for threads
            // that are truly stuck: Ready state, not current on any CPU, not in
            // any deferred-requeue slot.
            #[cfg(target_arch = "aarch64")]
            {
                use core::sync::atomic::{AtomicU32, Ordering as AO};
                static STUCK_LOG_COUNT: AtomicU32 = AtomicU32::new(0);
                let first_stuck = self
                    .threads
                    .iter()
                    .find(|t| {
                        if t.state != super::thread::ThreadState::Ready {
                            return false;
                        }
                        let tid = t.id();
                        if (0..MAX_CPUS).any(|c| self.cpu_state[c].idle_thread == tid) {
                            return false;
                        }
                        if (0..MAX_CPUS).any(|c| self.cpu_state[c].current_thread == Some(tid)) {
                            return false;
                        }
                        if self.is_in_deferred_requeue(tid) {
                            return false;
                        }
                        true
                    })
                    .map(|t| t.id());
                if let Some(stuck_tid) = first_stuck {
                    let count = STUCK_LOG_COUNT.fetch_add(1, AO::Relaxed);
                    if count < 5 || count % 1000 == 0 {
                        crate::serial_aarch64::raw_serial_str(b"[SCHED] queue_empty stuck_tid=");
                        {
                            let mut n = stuck_tid;
                            let mut buf = [0u8; 20];
                            let mut i = 20usize;
                            if n == 0 {
                                buf[i - 1] = b'0';
                                i -= 1;
                            } else {
                                while n > 0 {
                                    i -= 1;
                                    buf[i] = b'0' + (n % 10) as u8;
                                    n /= 10;
                                }
                            }
                            crate::serial_aarch64::raw_serial_str(&buf[i..]);
                        }
                        crate::serial_aarch64::raw_serial_str(b" count=");
                        {
                            let mut n = count as u64;
                            let mut buf = [0u8; 20];
                            let mut i = 20usize;
                            if n == 0 {
                                buf[i - 1] = b'0';
                                i -= 1;
                            } else {
                                while n > 0 {
                                    i -= 1;
                                    buf[i] = b'0' + (n % 10) as u8;
                                    n /= 10;
                                }
                            }
                            crate::serial_aarch64::raw_serial_str(&buf[i..]);
                        }
                        crate::serial_aarch64::raw_serial_str(b"\n");
                    }
                }
            }
            break self.cpu_state[current_cpu].idle_thread;
        };

        // Handle same-thread cases
        let any_other_queued = self.per_cpu_queues.iter().any(|q| !q.is_empty());
        if Some(next_thread_id) == self.cpu_state[current_cpu].current_thread && any_other_queued {
            // Current thread was popped but other threads are waiting.
            // DON'T push current back to queue yet — defer until after context save.
            // Just pop the next different thread.
            should_requeue_old = true;
            // Try local queue first, then steal
            next_thread_id = {
                let mut found = None;
                if let Some(n) = self.per_cpu_queues[current_cpu].pop_front() {
                    found = Some(n);
                } else {
                    for steal_cpu in 0..MAX_CPUS {
                        if steal_cpu == current_cpu {
                            continue;
                        }
                        if let Some(n) = self.per_cpu_queues[steal_cpu].pop_front() {
                            found = Some(n);
                            break;
                        }
                    }
                }
                match found {
                    Some(id) => id,
                    None => return None,
                }
            };
        } else if Some(next_thread_id) == self.cpu_state[current_cpu].current_thread {
            if next_thread_id != self.cpu_state[current_cpu].idle_thread {
                let is_userspace = self
                    .get_thread(next_thread_id)
                    .map(|t| t.privilege == super::thread::ThreadPrivilege::User)
                    .unwrap_or(false);
                if is_userspace {
                    // No switch needed. The current thread continues running on
                    // this CPU. Don't requeue — it's still "current" and will be
                    // handled next time schedule_deferred_requeue is called.
                    // Restore Running state (was set to Ready at line 541 above).
                    if let Some(t) = self.get_thread_mut(next_thread_id) {
                        t.set_running();
                    }
                    return None;
                }
                // For non-userspace same-thread-alone: switch to idle.
                // The old thread (which was popped) must be requeued AFTER
                // context save — same deferred-requeue logic applies. Whether
                // the thread was in the queue from unblock() or from the
                // deferred push, either way we must save context first.
                should_requeue_old = true;
                next_thread_id = self.cpu_state[current_cpu].idle_thread;
                crate::per_cpu_aarch64::set_need_resched(true);
            } else {
                return None;
            }
        }

        let old_thread_id = self.cpu_state[current_cpu]
            .current_thread
            .unwrap_or(self.cpu_state[current_cpu].idle_thread);

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

        // Update per-CPU idle flag (lock-free, used by timer handler)
        let is_switching_to_idle = next_thread_id == self.cpu_state[current_cpu].idle_thread;
        set_cpu_idle(current_cpu, is_switching_to_idle);

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
        // CRITICAL: Don't requeue threads that are currently running on any CPU.
        // Race condition: wake_expired_timers (or unblock) can wake a thread and
        // dispatch it on another CPU while this CPU's DEFERRED_REQUEUE still holds
        // the thread's ID. Without this check, the deferred requeue would add the
        // thread to the ready queue AGAIN, causing it to be dispatched on a second
        // CPU simultaneously — sharing the same kernel stack and Thread context,
        // leading to register/stack corruption (DATA_ABORT, INSTRUCTION_ABORT, etc.).
        if (0..MAX_CPUS).any(|cpu| self.cpu_state[cpu].current_thread == Some(thread_id)) {
            return;
        }
        // Don't requeue threads still pending deferred requeue on another CPU.
        // This is a defense-in-depth check — the primary protection is in
        // the wakeup paths (unblock, wake_expired_timers, etc.).
        if self.is_in_deferred_requeue(thread_id) {
            return;
        }
        // Safety checks: only requeue if the thread is in Ready state and not already queued.
        // Also handle the deferred-window race: if unblock_for_io() fired while the thread
        // was in the deferred slot (set Ready but couldn't enqueue), enqueue it now.
        if let Some(thread) = self.get_thread(thread_id) {
            if thread.state != ThreadState::Ready {
                return; // Thread state changed (terminated/blocked) - don't requeue
            }
        } else {
            return;
        }
        let in_any_queue = self.per_cpu_queues.iter().any(|q| q.contains(&thread_id));
        if !in_any_queue {
            let cpu = Self::current_cpu_id();
            self.per_cpu_queues[cpu].push_back(thread_id);
            // Send IPI to wake an idle CPU to pick up the requeued thread
            self.send_resched_ipi();
        }
    }

    /// Block the current thread
    #[allow(dead_code)]
    pub fn block_current(&mut self) {
        if let Some(current) = self.current_thread_mut() {
            // Charge elapsed CPU ticks before blocking
            let now = crate::time::get_ticks();
            current.cpu_ticks_total += now.wrapping_sub(current.run_start_ticks);
            current.run_start_ticks = now;

            current.set_blocked();
        }
    }

    /// Unblock a thread by ID
    pub fn unblock(&mut self, thread_id: u64) {
        // Increment the call counter for testing (tracks that unblock was called)
        UNBLOCK_CALL_COUNT.fetch_add(1, Ordering::SeqCst);

        if let Some(thread) = self.get_thread_mut(thread_id) {
            let was_blocked_on_io = thread.state == ThreadState::BlockedOnIO;
            if thread.state == ThreadState::Blocked
                || thread.state == ThreadState::BlockedOnSignal
                || thread.state == ThreadState::BlockedOnTimer
                || thread.state == ThreadState::BlockedOnIO
            {
                thread.set_ready();
                // For BlockedOnIO, do NOT clear blocked_in_syscall here —
                // the wait_timeout caller manages it after detecting the wakeup.
                // For other states it is safe (and necessary) to clear.
                if !was_blocked_on_io {
                    thread.blocked_in_syscall = false;
                }

                // SMP safety: Don't add to ready_queue if thread is currently
                // running on any CPU. If a thread is blocked in a syscall's WFI
                // loop (e.g., sys_read waiting for keyboard input), it's still
                // the "current thread" on that CPU. Adding it to the ready_queue
                // would allow another CPU to schedule it simultaneously, causing
                // double-scheduling: two CPUs executing the same thread with the
                // same stack, leading to context corruption and crashes (ELR=0x0).
                // The CPU running the thread will detect the state change (Blocked
                // → Ready) when its WFI loop checks the thread state after waking.
                let is_current_on_any_cpu =
                    (0..MAX_CPUS).any(|cpu| self.cpu_state[cpu].current_thread == Some(thread_id));

                // SMP safety: Don't add to ready_queue if thread was just
                // context-switched out and the old CPU's ERET hasn't completed.
                // State was already set to Ready above; the deferred requeue
                // will add it to ready_queue when the kernel stack is free.
                #[cfg(target_arch = "aarch64")]
                let is_in_deferred = self.is_in_deferred_requeue(thread_id);
                #[cfg(not(target_arch = "aarch64"))]
                let is_in_deferred = false;

                if !is_current_on_any_cpu
                    && !is_in_deferred
                    && thread_id != self.cpu_state[Self::current_cpu_id()].idle_thread
                    && !self.per_cpu_queues.iter().any(|q| q.contains(&thread_id))
                {
                    let target = self.find_target_cpu_for_wakeup(thread_id);
                    self.per_cpu_queues[target].push_back(thread_id);
                    // CRITICAL: Only log on x86_64 to avoid deadlock on ARM64
                    #[cfg(target_arch = "x86_64")]
                    log_serial_println!("unblock({}): Added to per_cpu_queues[{}]", thread_id, target);

                    // Send IPI to wake an idle CPU so it can pick up the unblocked thread
                    #[cfg(target_arch = "aarch64")]
                    self.send_resched_ipi();
                }
            }
        }
    }

    /// Send reschedule IPIs (SGI 0) to all idle CPUs.
    ///
    /// Called after adding a thread to the ready queue to wake CPUs that are
    /// sitting in WFI so they can pick up newly-runnable threads.
    ///
    /// Uses cpu_state (authoritative, protected by scheduler lock which is
    /// held when this is called) to identify idle CPUs. We wake ALL idle
    /// CPUs because during burst scheduling (e.g., init forking 4 children),
    /// multiple threads may be added to the queue in quick succession. Since
    /// cpu_state isn't updated until after the deferred commit, waking only
    /// one CPU would repeatedly target the same idle CPU while others sleep.
    /// Waking all ensures prompt thread pickup; idle CPUs that find nothing
    /// in the queue return immediately with negligible overhead.
    #[cfg(target_arch = "aarch64")]
    fn send_resched_ipi(&self) {
        use crate::arch_impl::aarch64::smp;

        let current_cpu = Self::current_cpu_id();
        let online = smp::cpus_online() as usize;

        for cpu in 0..online {
            if cpu == current_cpu {
                continue;
            }
            if cpu < MAX_CPUS {
                if let Some(current) = self.cpu_state[cpu].current_thread {
                    if current == self.cpu_state[cpu].idle_thread {
                        crate::arch_impl::aarch64::gic::send_sgi(
                            crate::arch_impl::aarch64::constants::SGI_RESCHEDULE as u8,
                            cpu as u8,
                        );
                        // Continue to wake ALL idle CPUs
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
                // Charge elapsed CPU ticks before blocking
                let now = crate::time::get_ticks();
                thread.cpu_ticks_total += now.wrapping_sub(thread.run_start_ticks);
                thread.run_start_ticks = now;

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
                log_serial_println!(
                    "Thread {} blocked waiting for signal (blocked_in_syscall=true)",
                    current_id
                );
            }
            // Remove from ready queue (shouldn't be there but make sure)
            for q in self.per_cpu_queues.iter_mut() {
                q.retain(|&id| id != current_id);
            }
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
            "unblock_for_signal: Checking thread {} (current={:?})",
            thread_id,
            self.cpu_state[Self::current_cpu_id()].current_thread,
        );
        if let Some(thread) = self.get_thread_mut(thread_id) {
            #[cfg(target_arch = "x86_64")]
            log_serial_println!(
                "unblock_for_signal: Thread {} state is {:?}, blocked_in_syscall={}",
                thread_id,
                thread.state,
                thread.blocked_in_syscall
            );
            // Also wake threads blocked on I/O — they check signals in their
            // wait loop and will return EINTR when they resume.
            if thread.state == ThreadState::BlockedOnIO {
                self.unblock_for_io(thread_id);
                return;
            }
            if thread.state == ThreadState::BlockedOnSignal {
                thread.set_ready();
                // NOTE: Do NOT clear blocked_in_syscall here!
                // The thread needs to resume inside the syscall and complete it.
                // blocked_in_syscall will be cleared when the syscall actually returns.

                // SMP safety: Don't add to ready_queue if thread is current on any CPU
                // (same rationale as unblock() - prevents double-scheduling)
                let is_current_on_any_cpu =
                    (0..MAX_CPUS).any(|cpu| self.cpu_state[cpu].current_thread == Some(thread_id));

                #[cfg(target_arch = "aarch64")]
                let is_in_deferred = self.is_in_deferred_requeue(thread_id);
                #[cfg(not(target_arch = "aarch64"))]
                let is_in_deferred = false;

                if !is_current_on_any_cpu
                    && !is_in_deferred
                    && thread_id != self.cpu_state[Self::current_cpu_id()].idle_thread
                    && !self.per_cpu_queues.iter().any(|q| q.contains(&thread_id))
                {
                    let target = self.find_target_cpu_for_wakeup(thread_id);
                    self.per_cpu_queues[target].push_back(thread_id);
                    #[cfg(target_arch = "x86_64")]
                    log_serial_println!(
                        "unblock_for_signal: Thread {} unblocked, added to per_cpu_queues[{}]",
                        thread_id,
                        target
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
                // Charge elapsed CPU ticks before blocking
                let now = crate::time::get_ticks();
                thread.cpu_ticks_total += now.wrapping_sub(thread.run_start_ticks);
                thread.run_start_ticks = now;

                thread.state = ThreadState::BlockedOnChildExit;
                // CRITICAL: Mark that this thread is blocked inside a syscall.
                // When the thread is resumed, we must NOT restore userspace context
                // because that would return to the pre-syscall location instead of
                // letting the syscall complete and return properly.
                thread.blocked_in_syscall = true;
                // CRITICAL: Only log on x86_64 to avoid deadlock on ARM64
                #[cfg(target_arch = "x86_64")]
                log_serial_println!(
                    "Thread {} blocked waiting for child exit (blocked_in_syscall=true)",
                    current_id
                );
            }
            // Remove from ready queue (shouldn't be there but make sure)
            for q in self.per_cpu_queues.iter_mut() {
                q.retain(|&id| id != current_id);
            }
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
                let is_current_on_any_cpu =
                    (0..MAX_CPUS).any(|cpu| self.cpu_state[cpu].current_thread == Some(thread_id));

                #[cfg(target_arch = "aarch64")]
                let is_in_deferred = self.is_in_deferred_requeue(thread_id);
                #[cfg(not(target_arch = "aarch64"))]
                let is_in_deferred = false;

                if !is_current_on_any_cpu
                    && !is_in_deferred
                    && thread_id != self.cpu_state[Self::current_cpu_id()].idle_thread
                    && !self.per_cpu_queues.iter().any(|q| q.contains(&thread_id))
                {
                    let target = self.find_target_cpu_for_wakeup(thread_id);
                    self.per_cpu_queues[target].push_back(thread_id);
                    // CRITICAL: Only log on x86_64 to avoid deadlock on ARM64
                    #[cfg(target_arch = "x86_64")]
                    log_serial_println!("Thread {} unblocked by child exit, queued to cpu {}", thread_id, target);

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

    /// Block current thread until a timer expires (nanosleep syscall)
    pub fn block_current_for_timer(&mut self, wake_time_ns: u64) {
        if let Some(current_id) = self.cpu_state[Self::current_cpu_id()].current_thread {
            if let Some(thread) = self.get_thread_mut(current_id) {
                // Charge elapsed CPU ticks before blocking
                let now = crate::time::get_ticks();
                thread.cpu_ticks_total += now.wrapping_sub(thread.run_start_ticks);
                thread.run_start_ticks = now;

                thread.state = ThreadState::BlockedOnTimer;
                thread.wake_time_ns = Some(wake_time_ns);
                thread.blocked_in_syscall = true;
            }
            // Insert into timer heap for O(1) expiry detection
            self.timer_heap.push(Reverse((wake_time_ns, current_id)));
            for q in self.per_cpu_queues.iter_mut() {
                q.retain(|&id| id != current_id);
            }
        }
    }

    /// Block the current thread for device I/O.
    ///
    /// Sets state to BlockedOnIO and blocked_in_syscall. The thread will be
    /// woken by unblock_for_io() when the device ISR signals completion.
    ///
    /// CRITICAL: Must be called under the scheduler lock (via with_scheduler).
    /// The done-check and this call must happen in the same with_scheduler()
    /// invocation to prevent the ISR from racing between the check and the block.
    pub fn block_current_for_io(&mut self) {
        self.block_current_for_io_with_timeout(None);
    }

    /// Block the current thread for device I/O, optionally with a timeout.
    ///
    /// A timed BlockedOnIO wait is used by completions: the ISR wakes it via
    /// unblock_for_io(), while the timer path wakes it by observing
    /// wake_time_ns without clearing blocked_in_syscall prematurely.
    pub fn block_current_for_io_with_timeout(&mut self, wake_time_ns: Option<u64>) {
        if let Some(current_id) = self.cpu_state[Self::current_cpu_id()].current_thread {
            if let Some(thread) = self.get_thread_mut(current_id) {
                // Charge elapsed CPU ticks before blocking
                let now = crate::time::get_ticks();
                thread.cpu_ticks_total += now.wrapping_sub(thread.run_start_ticks);
                thread.run_start_ticks = now;

                thread.state = ThreadState::BlockedOnIO;
                thread.wake_time_ns = wake_time_ns;
                // Mark blocked_in_syscall so the context switch path resumes
                // inside the syscall (wait_timeout loop) rather than restoring
                // stale userspace context.
                thread.blocked_in_syscall = true;
            }
            // Insert into timer heap if a timeout was specified
            if let Some(wt) = wake_time_ns {
                self.timer_heap.push(Reverse((wt, current_id)));
            }
            // Remove from all per-CPU queues (shouldn't be there, but guard against races)
            for q in self.per_cpu_queues.iter_mut() {
                q.retain(|&id| id != current_id);
            }
        }
    }

    /// Unblock a thread that was blocked for device I/O.
    ///
    /// Sets state to Ready and adds to ready queue. Does NOT clear
    /// blocked_in_syscall — the wait_timeout caller clears it after resuming
    /// to prevent context save corruption (clearing it early would allow the
    /// context switch path to restore stale userspace context).
    ///
    /// Safe to call from ISR context via with_scheduler() because
    /// with_scheduler() disables interrupts before acquiring the lock, and
    /// the ISR runs with interrupts already masked by hardware.
    pub fn unblock_for_io(&mut self, tid: u64) {
        if let Some(thread) = self.get_thread_mut(tid) {
            let should_queue = if thread.state == ThreadState::BlockedOnIO {
                thread.set_ready();
                thread.wake_time_ns = None;
                // Do NOT clear blocked_in_syscall here — the wait_timeout
                // caller manages it after detecting the wakeup.
                true
            } else {
                // If the completion wakeup was deferred through the lock-free
                // ISR buffer, the thread may already have been marked Ready by
                // another path before the buffer is drained. As long as it is
                // still blocked in the syscall, it still needs a ready-queue
                // insertion to resume and observe `done=token`.
                thread.state == ThreadState::Ready && thread.blocked_in_syscall
            };

            if should_queue {
                // Do not enqueue a thread that is still current on some CPU.
                // For BlockedOnIO waiters this means the wakeup won the race
                // against the old CPU's context save; that CPU will publish
                // the saved context and requeue the Ready thread after the
                // save point completes.
                let is_current_on_any_cpu =
                    (0..MAX_CPUS).any(|cpu| self.cpu_state[cpu].current_thread == Some(tid));

                #[cfg(target_arch = "aarch64")]
                let is_in_deferred = self.is_in_deferred_requeue(tid);
                #[cfg(not(target_arch = "aarch64"))]
                let is_in_deferred = false;

                if !is_current_on_any_cpu
                    && !is_in_deferred
                    && tid != self.cpu_state[Self::current_cpu_id()].idle_thread
                    && !self.per_cpu_queues.iter().any(|q| q.contains(&tid))
                {
                    let target = self.find_target_cpu_for_wakeup(tid);
                    self.per_cpu_queues[target].push_back(tid);
                    #[cfg(target_arch = "aarch64")]
                    self.send_resched_ipi();
                }
                set_need_resched();
            }
        }
    }

    /// Block current thread for compositor frame pacing (mark_window_dirty syscall).
    ///
    /// Uses BlockedOnTimer with a timeout so the thread wakes either when
    /// the compositor calls unblock() or when the timeout expires (fallback).
    /// This provides Wayland-style back-pressure: the client renders at
    /// exactly the compositor's display rate.
    pub fn block_current_for_compositor(&mut self, timeout_ns: u64) {
        if let Some(current_id) = self.cpu_state[Self::current_cpu_id()].current_thread {
            if let Some(thread) = self.get_thread_mut(current_id) {
                // Charge elapsed CPU ticks NOW, before blocking. Otherwise the
                // next schedule() call charges all time since last dispatch —
                // including blocked/sleeping time — as CPU usage.
                let now = crate::time::get_ticks();
                thread.cpu_ticks_total += now.wrapping_sub(thread.run_start_ticks);
                thread.run_start_ticks = now;

                thread.state = ThreadState::BlockedOnTimer;
                thread.wake_time_ns = Some(timeout_ns);
                thread.blocked_in_syscall = true;
            }
            // Insert into timer heap for O(1) expiry detection
            self.timer_heap.push(Reverse((timeout_ns, current_id)));
            for q in self.per_cpu_queues.iter_mut() {
                q.retain(|&id| id != current_id);
            }
        }
    }

    /// Check the timer heap for expired timer-based sleep and wake them.
    ///
    /// Uses a BinaryHeap (min-heap via Reverse) so only expired entries at the
    /// front are visited — O(1) peek + O(log N) pop per expired timer, vs the
    /// old O(N) scan of ALL threads. Stale entries (threads already woken by
    /// ISR, signal, or terminated) are detected by the validation step and
    /// discarded without any side effects.
    ///
    /// Called from schedule() on every reschedule, and from the nanosleep
    /// HLT loop to immediately detect timer expiry without waiting for
    /// a scheduling decision on another CPU.
    pub fn wake_expired_timers(&mut self) {
        let (secs, nanos) = crate::time::get_monotonic_time_ns();
        let now_ns = secs as u64 * 1_000_000_000 + nanos as u64;

        // Pop all expired entries from the min-heap
        while let Some(&Reverse((wake_time, tid))) = self.timer_heap.peek() {
            if wake_time > now_ns {
                break; // All remaining entries are in the future
            }
            self.timer_heap.pop();

            // Validate: thread might have been woken already (by ISR, signal, etc.)
            // or terminated. Only process if still in a timed-wait state with a
            // wake_time set.
            let is_timed_wait = if let Some(thread) = self.get_thread(tid) {
                (matches!(thread.state, ThreadState::BlockedOnTimer)
                    || (thread.state == ThreadState::BlockedOnIO
                        && thread.wake_time_ns.is_some()))
                    && thread.wake_time_ns.is_some()
            } else {
                false
            };

            if !is_timed_wait {
                continue; // Stale entry — thread already woken or terminated
            }

            // SMP safety: Don't add to ready_queue if thread is currently
            // running on any CPU. Same protection as unblock() — a thread
            // in BlockedOnTimer state might still be executing its WFI poll
            // loop (e.g., sys_poll, sys_nanosleep). Adding it to the
            // ready_queue would allow another CPU to dispatch it, causing
            // double-scheduling: two CPUs executing the same thread with
            // the same kernel stack, leading to context corruption and
            // crashes (ELR=0x0, SPSR corruption).
            //
            // The CPU running the thread will detect the state change
            // (BlockedOnTimer → Ready) when its poll loop checks the thread
            // state after waking from WFI. If the thread is context-switched
            // out before detecting it, DEFERRED_REQUEUE will properly add it
            // to the ready_queue when the kernel stack is free.
            let is_current_on_any_cpu =
                (0..MAX_CPUS).any(|cpu| self.cpu_state[cpu].current_thread == Some(tid));
            if is_current_on_any_cpu {
                // Still update state so the running thread detects the change,
                // but don't clear blocked_in_syscall — the running thread
                // manages this flag itself when it detects the state change.
                if let Some(thread) = self.get_thread_mut(tid) {
                    let was_blocked_on_io = thread.state == ThreadState::BlockedOnIO;
                    thread.state = ThreadState::Ready;
                    thread.wake_time_ns = None;
                    if !was_blocked_on_io {
                        thread.blocked_in_syscall = false;
                    }
                }
                continue;
            }

            // SMP safety: Don't add to ready_queue if thread was just
            // context-switched out and the old CPU's ERET hasn't completed.
            // The deferred requeue will add it when the kernel stack is free.
            #[cfg(target_arch = "aarch64")]
            let in_deferred_requeue = self.is_in_deferred_requeue(tid);
            #[cfg(not(target_arch = "aarch64"))]
            let in_deferred_requeue = false;

            let is_idle = tid == self.cpu_state[Self::current_cpu_id()].idle_thread;
            let already_queued = self.per_cpu_queues.iter().any(|q| q.contains(&tid));

            if let Some(thread) = self.get_thread_mut(tid) {
                let was_blocked_on_io = thread.state == ThreadState::BlockedOnIO;
                thread.state = ThreadState::Ready;
                thread.wake_time_ns = None;

                // Timer-driven I/O wakeups resume back into wait_timeout() so
                // blocked_in_syscall must stay set until the waiter consumes
                // the wake reason. Ordinary BlockedOnTimer sleeps clear it here.
                if !was_blocked_on_io {
                    thread.blocked_in_syscall = false;
                }

                if !in_deferred_requeue && !is_idle && !already_queued {
                    let target = self.find_target_cpu_for_wakeup(tid);
                    self.per_cpu_queues[target].push_back(tid);
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
        self.per_cpu_queues.iter().any(|q| !q.is_empty())
            || self.cpu_state[Self::current_cpu_id()]
                .current_thread
                .map_or(false, |id| {
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

    /// Remove a thread from all per-CPU queues (used when blocking)
    pub fn remove_from_ready_queue(&mut self, thread_id: u64) {
        for q in self.per_cpu_queues.iter_mut() {
            q.retain(|&id| id != thread_id);
        }
    }

    /// Get the total ready queue length across all CPUs (for tracing)
    pub fn ready_queue_length(&self) -> usize {
        self.per_cpu_queues.iter().map(|q| q.len()).sum()
    }

    /// Find which CPU this thread last ran on, or the least-loaded CPU if unknown.
    /// Used by wakeup paths for cache-affinity routing.
    fn find_target_cpu_for_wakeup(&self, tid: u64) -> usize {
        let current_cpu = Self::current_cpu_id();
        // If the thread is still "current" on a CPU, use that CPU (affinity).
        for cpu in 0..MAX_CPUS {
            if self.cpu_state[cpu].current_thread == Some(tid) {
                return cpu;
            }
        }
        // Otherwise pick the least-loaded CPU.
        (0..MAX_CPUS)
            .min_by_key(|&cpu| self.per_cpu_queues[cpu].len())
            .unwrap_or(current_cpu)
    }

    /// Find the CPU with the fewest threads in its queue.
    /// Used when spawning new threads.
    fn least_loaded_cpu(&self) -> usize {
        let current_cpu = Self::current_cpu_id();
        (0..MAX_CPUS)
            .min_by_key(|&cpu| self.per_cpu_queues[cpu].len())
            .unwrap_or(current_cpu)
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

    /// Check if a thread is an idle thread on any CPU (called from within lock hold).
    ///
    /// Unlike the module-level `is_idle_thread()` which acquires the SCHEDULER lock,
    /// this method works directly on `&self` for use inside a single lock hold.
    #[cfg(target_arch = "aarch64")]
    pub fn is_idle_thread_inner(&self, thread_id: u64) -> bool {
        (0..MAX_CPUS).any(|cpu| self.cpu_state[cpu].idle_thread == thread_id)
    }

    /// Check if a thread is in the deferred requeue state on any CPU.
    ///
    /// Returns true if the thread was recently context-switched out on some CPU
    /// and that CPU's ERET hasn't completed yet (the thread's kernel stack is
    /// still in use). Wakeup paths must not add such threads to the ready_queue.
    #[cfg(target_arch = "aarch64")]
    pub fn is_in_deferred_requeue(&self, thread_id: u64) -> bool {
        (0..MAX_CPUS).any(|cpu| self.cpu_state[cpu].previous_thread == Some(thread_id))
    }

    /// Set the need_resched flag (called from within lock hold, no lock needed).
    #[cfg(target_arch = "aarch64")]
    pub fn set_need_resched_inner(&self) {
        NEED_RESCHED.store(true, Ordering::Release);
        crate::per_cpu_aarch64::set_need_resched(true);
    }

    /// Rescue threads that are Ready but stuck outside the ready queue.
    ///
    /// Safety net for the block/unblock race described in completion.rs: a thread
    /// may end up with state=Ready but not in the ready queue, not current on any
    /// CPU, and not in a DEFERRED_REQUEUE slot.  This can happen when:
    ///
    ///   1. `block_current_for_io()` sets state=BlockedOnIO and the thread is
    ///      still "current" (is_current_on_any_cpu=true).
    ///   2. A timer fires and the scheduler switches the thread out — without
    ///      requeueing it (correctly, since it is Blocked).
    ///   3. The AHCI ISR calls `unblock_for_io()`. At this moment the thread is
    ///      no longer "current" and not in DEFERRED_REQUEUE (the deferred slot was
    ///      already drained by a subsequent context switch). `unblock_for_io` sets
    ///      state=Ready and adds the thread to the ready queue — correct.
    ///
    ///   BUT: a second, subtler race exists around `previous_thread`.  Between
    ///   `commit_cpu_state_after_save` (which clears is_current) and
    ///   `DEFERRED_REQUEUE[cpu].swap(old_id, …)` (which publishes the deferred
    ///   slot), there is a window in which the thread is neither "current" nor
    ///   in DEFERRED_REQUEUE.  If `unblock_for_io` fires in that window it
    ///   adds the thread to the ready queue.  Then the deferred-requeue
    ///   processing at the NEXT context switch calls `requeue_thread_after_save`
    ///   which skips the add (thread already in queue) — correct.
    ///
    ///   The actual slow-leak scenario we protect against is any path — now or
    ///   in future — that leaves state=Ready without a corresponding ready-queue
    ///   entry.  Running this rescue every ~1 second bounds the maximum stall.
    ///
    /// Called from the timer interrupt handler on CPU 0 every 1000 ticks (~1 s).
    /// Runs under a try_lock to avoid blocking the timer handler; if the lock is
    /// contended the rescue simply runs on the next second.
    #[cfg(target_arch = "aarch64")]
    pub fn rescue_stuck_ready_threads(&mut self) -> u32 {
        use crate::arch_impl::aarch64::context_switch::{raw_uart_dec, raw_uart_str};

        let mut rescued: u32 = 0;

        // Collect IDs of stuck threads first (immutable pass) to avoid borrow
        // conflicts with the subsequent mutable requeue_thread_after_save calls.
        let stuck_tids: alloc::vec::Vec<u64> = self
            .threads
            .iter()
            .filter_map(|t| {
                if t.state != ThreadState::Ready {
                    return None;
                }
                let tid = t.id();
                // Exclude idle threads — they are never in the ready queue.
                if (0..MAX_CPUS).any(|c| self.cpu_state[c].idle_thread == tid) {
                    return None;
                }
                // Exclude threads already in any per-CPU queue — they are not stuck.
                if self.per_cpu_queues.iter().any(|q| q.contains(&tid)) {
                    return None;
                }
                // Exclude threads currently running on any CPU — they are being
                // handled by that CPU's scheduling path.
                if (0..MAX_CPUS).any(|c| self.cpu_state[c].current_thread == Some(tid)) {
                    return None;
                }
                // Exclude threads pending deferred requeue on any CPU — the deferred
                // mechanism will pick them up at the start of the next context switch.
                if self.is_in_deferred_requeue(tid) {
                    return None;
                }
                // This thread is genuinely stuck — Ready but reachable by nobody.
                Some(tid)
            })
            .collect();

        for tid in stuck_tids {
            // Re-validate under mutable borrow (state could have changed since
            // the immutable scan above, though both run under the scheduler lock).
            if let Some(thread) = self.get_thread(tid) {
                if thread.state != ThreadState::Ready {
                    continue;
                }
            } else {
                continue;
            }

            // Diagnostic: emit once per rescued thread (lock-free UART).
            raw_uart_str("[SCHED_RESCUE] stuck tid=");
            raw_uart_dec(tid);
            raw_uart_str(" bis=");
            raw_uart_dec(
                if self
                    .get_thread(tid)
                    .map(|t| t.blocked_in_syscall)
                    .unwrap_or(false)
                {
                    1
                } else {
                    0
                },
            );
            raw_uart_str("\n");

            let target = self.find_target_cpu_for_wakeup(tid);
            self.per_cpu_queues[target].push_back(tid);
            self.send_resched_ipi();
            rescued += 1;
        }

        rescued
    }

    /// Fix stale cpu_state where it says idle but a real thread is running.
    ///
    /// Called from within the consolidated context switch lock hold, before
    /// the scheduling decision. This prevents TOCTOU races where cpu_state
    /// is stale (says idle) but a real user thread is running on this CPU.
    #[cfg(target_arch = "aarch64")]
    pub fn fix_stale_idle_cpu_state(&mut self, real_tid: u64) {
        let cpu = Self::current_cpu_id();
        let current = self.cpu_state[cpu].current_thread;
        let idle = self.cpu_state[cpu].idle_thread;
        if current == Some(idle) && real_tid != idle {
            record_cpu_state_change(cpu, 1, idle, real_tid);
            self.cpu_state[cpu].current_thread = Some(real_tid);
        }
    }

    /// Repair stale cpu_state after an exception handler redirected to idle.
    ///
    /// The exception path may have committed the per-CPU return state to the
    /// idle loop but failed to update scheduler cpu_state if the global lock was
    /// contended. Before the next save/dispatch, force the CPU's logical owner
    /// back to its idle thread so we do not save an idle-loop frame into the
    /// previously running user thread.
    #[cfg(target_arch = "aarch64")]
    pub fn fix_exception_cleanup_cpu_state(&mut self) {
        let cpu = Self::current_cpu_id();
        let idle = self.cpu_state[cpu].idle_thread;
        let current = self.cpu_state[cpu].current_thread.unwrap_or(0xDEAD);
        if current != idle {
            record_cpu_state_change(cpu, 3, current, idle);
            self.cpu_state[cpu].current_thread = Some(idle);
        }
        self.cpu_state[cpu].previous_thread = None;
        set_cpu_idle(cpu, true);
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
    log_serial_println!(
        "Scheduler initialized with current thread {} as idle task",
        thread_id
    );
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
            {
                crate::per_cpu_aarch64::set_need_resched(true);
                // Wake idle CPUs so they can pick up the new thread immediately
                // rather than waiting up to 1ms for their next timer tick.
                scheduler.send_resched_ipi();
            }
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
            {
                crate::per_cpu_aarch64::set_need_resched(true);
                // Wake idle CPUs so the fork child is picked up immediately
                // rather than waiting up to 1ms for their next timer tick.
                // Without this, all 7 idle CPUs sleep through the spawn and only
                // the spawning CPU's next timer tick dispatches the child.
                scheduler.send_resched_ipi();
            }
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

/// Perform scheduling inline from Rust kernel context (AArch64).
#[cfg(target_arch = "aarch64")]
pub fn schedule() {
    crate::arch_impl::aarch64::context_switch::schedule_from_kernel();
}

/// Perform scheduling and return threads to switch between
#[cfg(not(target_arch = "aarch64"))]
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
    #[cfg(target_arch = "aarch64")]
    {
        use crate::arch_impl::aarch64::timer_interrupt::CPU0_BREADCRUMB_ID;
        use core::sync::atomic::Ordering;
        let cpu_id = crate::arch_impl::aarch64::percpu::Aarch64PerCpu::cpu_id();
        if cpu_id == 0 {
            CPU0_BREADCRUMB_ID.store(20, Ordering::Relaxed); // with_scheduler entry
        }
    }
    without_interrupts(|| {
        let mut scheduler_lock = SCHEDULER.lock();
        #[cfg(target_arch = "aarch64")]
        {
            use crate::arch_impl::aarch64::timer_interrupt::CPU0_BREADCRUMB_ID;
            use core::sync::atomic::Ordering;
            let cpu_id = crate::arch_impl::aarch64::percpu::Aarch64PerCpu::cpu_id();
            if cpu_id == 0 {
                CPU0_BREADCRUMB_ID.store(21, Ordering::Relaxed); // after lock acquisition
            }
        }
        let result = scheduler_lock.as_mut().map(f);
        #[cfg(target_arch = "aarch64")]
        {
            use crate::arch_impl::aarch64::timer_interrupt::CPU0_BREADCRUMB_ID;
            use core::sync::atomic::Ordering;
            let cpu_id = crate::arch_impl::aarch64::percpu::Aarch64PerCpu::cpu_id();
            if cpu_id == 0 {
                CPU0_BREADCRUMB_ID.store(22, Ordering::Relaxed); // after closure
            }
        }
        result
    })
}

/// Attempt a non-blocking rescue of stuck ready threads.
///
/// Called from the timer interrupt handler (CPU 0) every ~1000 ticks (~1 s).
/// Uses `try_lock` so it never blocks the timer handler — if the scheduler lock
/// is contended we simply skip this cycle and try again next second.
///
/// See `Scheduler::rescue_stuck_ready_threads` for the full rationale.
#[cfg(target_arch = "aarch64")]
pub fn rescue_stuck_ready_threads_try() {
    if let Some(mut guard) = SCHEDULER.try_lock() {
        if let Some(sched) = guard.as_mut() {
            sched.rescue_stuck_ready_threads();
        }
    }
}

/// Collect the idle thread ID for each online CPU into a fixed-size buffer.
///
/// Returns the number of idle thread IDs written into `out` (one per online CPU).
/// The caller must pass a buffer large enough for `cpus_online` entries.
/// Safe to call from kernel context; disables interrupts internally.
pub fn collect_idle_thread_ids(out: &mut [u64]) -> usize {
    without_interrupts(|| {
        let scheduler_lock = SCHEDULER.lock();
        if let Some(sched) = scheduler_lock.as_ref() {
            let count = out.len().min(MAX_CPUS);
            for i in 0..count {
                out[i] = sched.cpu_state[i].idle_thread;
            }
            count
        } else {
            0
        }
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
        scheduler_lock
            .as_ref()
            .and_then(|s| s.cpu_state[Scheduler::current_cpu_id()].current_thread)
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
        let per_cpu = crate::per_cpu::need_resched();
        if per_cpu {
            crate::per_cpu::set_need_resched(false);
        }
        let global = NEED_RESCHED.swap(false, Ordering::Relaxed);
        per_cpu || global
    }
    #[cfg(target_arch = "aarch64")]
    {
        // ARM64: Check per-CPU flag AND global atomic.
        // CRITICAL: Both sources must be checked. spawn/spawn_front set the
        // global flag from one CPU but the target CPU may be different.
        // Previously, the global flag was cleared but its value was discarded,
        // meaning cross-CPU need_resched signals were silently lost.
        let per_cpu = crate::per_cpu_aarch64::need_resched();
        if per_cpu {
            crate::per_cpu_aarch64::set_need_resched(false);
        }
        let global = NEED_RESCHED.swap(false, Ordering::Relaxed);
        per_cpu || global
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

/// Lock-free ISR wakeup: push thread ID to per-CPU wakeup buffer.
///
/// Called from the AHCI ISR (via `Completion::complete()`) instead of
/// `with_scheduler(|s| s.unblock_for_io(tid))`.  This avoids acquiring the
/// global SCHEDULER mutex from ISR context, which was the root cause of
/// CPU 0's IRQ death: the ISR would spin on the lock with IRQs masked,
/// starving the timer for milliseconds.
///
/// The scheduler drains the buffer under its own lock at the top of every
/// `schedule_deferred_requeue()` / `schedule()` call.
pub fn isr_unblock_for_io(tid: u64) {
    let cpu = current_cpu_id_raw();
    if cpu < ISR_WAKEUP_BUFFERS.len() {
        ISR_WAKEUP_BUFFERS[cpu].push(tid);
    }
    set_need_resched();
    // Send IPI to idle CPUs so the buffer is drained promptly.
    #[cfg(target_arch = "aarch64")]
    {
        let online = crate::arch_impl::aarch64::smp::cpus_online() as usize;
        for target in 0..online.min(MAX_CPUS) {
            if target != cpu && is_cpu_idle(target) {
                crate::arch_impl::aarch64::gic::send_sgi(
                    crate::arch_impl::aarch64::constants::SGI_RESCHEDULE as u8,
                    target as u8,
                );
            }
        }
    }
}

/// Read the current CPU ID directly from hardware (MPIDR_EL1 on ARM64).
/// Safe to call from ISR context — no per-CPU data, no locks.
#[inline]
fn current_cpu_id_raw() -> usize {
    #[cfg(target_arch = "aarch64")]
    {
        let mpidr: u64;
        unsafe {
            core::arch::asm!("mrs {}, mpidr_el1", out(reg) mpidr, options(nomem, nostack));
        }
        (mpidr & 0xFF) as usize
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        0
    }
}

/// Check if a CPU is idle without acquiring any lock (raw version for ISR use).
/// On non-aarch64, always returns false.
#[allow(dead_code)]
pub fn is_cpu_idle_raw(cpu_id: usize) -> bool {
    #[cfg(target_arch = "aarch64")]
    {
        is_cpu_idle(cpu_id)
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        let _ = cpu_id;
        false
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
                idle_id,
                thread_ptr
            );
        } else {
            log::error!(
                "Exception handler: Failed to get idle thread {} from scheduler!",
                idle_id
            );
        }

        #[cfg(target_arch = "x86_64")]
        log::info!(
            "Exception handler: Switched scheduler to idle thread {}",
            idle_id
        );
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
            // Clear previous_thread to prevent starvation: if a crash occurred during
            // context switch, previous_thread stays set permanently blocking that thread
            // from being requeued on any CPU.
            sched.cpu_state[cpu_id].previous_thread = None;
            unsafe {
                crate::arch_impl::aarch64::percpu::Aarch64PerCpu::set_exception_cleanup_context(
                    false,
                );
            }
        }
    } else {
        unsafe {
            crate::arch_impl::aarch64::percpu::Aarch64PerCpu::set_exception_cleanup_context(true);
        }
    }
    // If try_lock fails, the scheduler state will be stale. This function
    // is only safe for exception handlers where the lock might be held by
    // this CPU. The consolidated context switch path handles dispatch failures
    // directly under the scheduler lock hold.
}

/// Test module for scheduler state invariants
/// These tests use x86_64-specific types (VirtAddr) and are only compiled for x86_64
#[cfg(all(test, target_arch = "x86_64"))]
pub mod tests {
    use super::*;
    use crate::task::thread::{Thread, ThreadPrivilege, ThreadState};
    use alloc::boxed::Box;
    use alloc::string::String;
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
            .per_cpu_queues
            .iter()
            .flat_map(|q| q.iter())
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
            .per_cpu_queues
            .iter()
            .flat_map(|q| q.iter())
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
                thread_id_before,
                thread_id_after
            );
        } else {
            log::info!("Scheduler invariant check passed: yield_current() preserves state");
        }

        // Clean up
        crate::per_cpu::set_need_resched(false);
    }
}

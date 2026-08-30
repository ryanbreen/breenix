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

#[cfg(target_arch = "aarch64")]
use super::thread::{CpuContext, VirtAddr};
use super::thread::{Thread, ThreadState};
#[cfg(feature = "boot_tests")]
use super::thread::ThreadPrivilege;
use crate::log_serial_println;
use alloc::{boxed::Box, collections::BinaryHeap, collections::VecDeque};
use core::cmp::Reverse;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use spin::Mutex;

/// Exit-batch identity carried by teardown-attributed expedite evidence.
/// P2 uses one pid-derived batch per single-victim request; P9 later assigns
/// one shared id to a group request rather than introducing a parallel type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GroupBatchId(u64);

impl GroupBatchId {
    pub const fn for_single_victim(pid: u64) -> Self {
        Self(pid)
    }

    #[cfg(target_arch = "aarch64")]
    const fn as_u64(self) -> u64 {
        self.0
    }
}

#[cfg(target_arch = "aarch64")]
#[inline(always)]
fn observe_exit_kick(thread_pid: u64) -> bool {
    use crate::tracing::providers::teardown::{EXIT_KICK_BUCKETS, EXIT_KICK_SLOTS};

    let slot = &EXIT_KICK_SLOTS[thread_pid as usize % EXIT_KICK_BUCKETS];
    if let Some(observation) = slot.observe(thread_pid) {
        let interval = crate::tracing::trace_timestamp().wrapping_sub(observation.at);
        crate::tracing::providers::teardown::record_exit_kick_observed(thread_pid, interval);
        true
    } else {
        slot.is_observed_for(thread_pid)
    }
}

#[cfg(not(target_arch = "aarch64"))]
#[inline(always)]
fn observe_exit_kick(_thread_pid: u64) -> bool {
    true
}

// Architecture-generic HAL wrappers for interrupt control.
#[cfg(not(target_arch = "aarch64"))]
use crate::arch_interrupts_enabled as are_enabled;
use crate::arch_without_interrupts as without_interrupts;

// ---------------------------------------------------------------------------
// Lock-free interrupt-context wakeup buffer
//
// AHCI completion and network softirq paths write thread IDs into a per-CPU
// slot array using atomic CAS — no lock, no allocation.
// The scheduler drains these buffers under its own lock at the top of every
// `schedule_deferred_requeue()` / `schedule()` call, performing the actual
// state transition + queue push.
//
// This breaks the ISR's dependency on the global SCHEDULER mutex, which was
// the root cause of CPU 0's IRQ death: the AHCI ISR on CPU 0 would spin on
// the lock (held by another CPU) with IRQs masked, starving the timer.
// ---------------------------------------------------------------------------

const ISR_WAKEUP_SLOTS: usize = 64;
const ISR_WAKEUP_EMPTY: u64 = 0;

/// Per-CPU lock-free buffer shared by AHCI completion wakeups
/// (`task/completion.rs`, `task/waitqueue.rs`) and loopback-pump/TCP wakes.
///
/// It is sized for the reviewed worst-case burst of 64 distinct tids between
/// two `schedule()` calls. Duplicate tids consume one slot, but all shared
/// producers count against this capacity, so sizing must cover their combined
/// burst rather than either subsystem in isolation.
struct IsrWakeupBuffer {
    slots: [AtomicU64; ISR_WAKEUP_SLOTS],
}

enum IsrWakePush {
    Inserted,
    AlreadyPending,
    Full,
}

// SAFETY: All access is via atomics.
unsafe impl Sync for IsrWakeupBuffer {}

impl IsrWakeupBuffer {
    const fn new() -> Self {
        Self {
            slots: [const { AtomicU64::new(ISR_WAKEUP_EMPTY) }; ISR_WAKEUP_SLOTS],
        }
    }

    /// Push a thread ID from interrupt context without locks or allocation.
    fn push(&self, tid: u64) -> IsrWakePush {
        for slot in &self.slots {
            let pending = slot.load(Ordering::Acquire);
            if pending == tid {
                return IsrWakePush::AlreadyPending;
            }
            if pending == ISR_WAKEUP_EMPTY {
                match slot.compare_exchange(
                    ISR_WAKEUP_EMPTY,
                    tid,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(_) => return IsrWakePush::Inserted,
                    Err(actual) if actual == tid => return IsrWakePush::AlreadyPending,
                    Err(_) => {}
                }
            }
        }
        IsrWakePush::Full
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

    /// Count pending entries without modifying the buffer.
    fn depth(&self) -> usize {
        self.slots
            .iter()
            .filter(|slot| slot.load(Ordering::Acquire) != ISR_WAKEUP_EMPTY)
            .count()
    }
}

static ISR_WAKEUP_BUFFERS: [IsrWakeupBuffer; 8] = [const { IsrWakeupBuffer::new() }; 8];

const WAKE_ATTRIB_MAX_TIDS: usize = 4096;
const READY_SITE_NONE: u64 = 0;
const READY_SITE_SCHEDULE: u64 = 1;
const READY_SITE_UNBLOCK: u64 = 2;
const READY_SITE_SIGNAL: u64 = 3;
const READY_SITE_CHILD_EXIT: u64 = 4;
const READY_SITE_WAKE_IO_LOCKED: u64 = 5;
const READY_SITE_WAKE_IO_ISR_DRAIN: u64 = 6;
const READY_SITE_TIMER: u64 = 7;

static WAKE_LAST_READY_SITE: [AtomicU64; WAKE_ATTRIB_MAX_TIDS] =
    [const { AtomicU64::new(READY_SITE_NONE) }; WAKE_ATTRIB_MAX_TIDS];

pub static WAKE_SITE_SCHEDULE: AtomicU64 = AtomicU64::new(0);
pub static WAKE_SITE_UNBLOCK: AtomicU64 = AtomicU64::new(0);
pub static WAKE_SITE_ISR_UNBLOCK: AtomicU64 = AtomicU64::new(0);
pub static WAKE_SITE_WAKE_IO_LOCKED: AtomicU64 = AtomicU64::new(0);
pub static WAKE_SITE_SIGNAL: AtomicU64 = AtomicU64::new(0);
pub static WAKE_SITE_CHILD_EXIT: AtomicU64 = AtomicU64::new(0);
pub static WAKE_SITE_TIMER: AtomicU64 = AtomicU64::new(0);

pub static ENQUEUE_SAME_LOCK_OK: AtomicU64 = AtomicU64::new(0);
pub static ENQUEUE_DEFERRED: AtomicU64 = AtomicU64::new(0);
pub static ENQUEUE_ISR_BUFFER: AtomicU64 = AtomicU64::new(0);
pub static ENQUEUE_ISR_BUFFER_DEDUP: AtomicU64 = AtomicU64::new(0);
pub static ENQUEUE_DEFERRED_DRAINED_OK: AtomicU64 = AtomicU64::new(0);
pub static ENQUEUE_ISR_BUFFER_DRAINED_OK: AtomicU64 = AtomicU64::new(0);
pub static ENQUEUE_ALREADY_QUEUED_OK: AtomicU64 = AtomicU64::new(0);
pub static ENQUEUE_ISR_BUFFER_FULL: AtomicU64 = AtomicU64::new(0);
pub static ENQUEUE_OFFLINE_RECLAIMED: AtomicU64 = AtomicU64::new(0);
pub static ENQUEUE_STALLED_RECLAIMED: AtomicU64 = AtomicU64::new(0);

#[inline]
fn wake_tid_index(tid: u64) -> Option<usize> {
    let idx = tid as usize;
    if idx < WAKE_ATTRIB_MAX_TIDS {
        Some(idx)
    } else {
        None
    }
}

#[inline]
fn record_ready_site(tid: u64, site: u64) {
    if let Some(idx) = wake_tid_index(tid) {
        WAKE_LAST_READY_SITE[idx].store(site, Ordering::Relaxed);
    }
}

#[cfg(target_arch = "aarch64")]
const TRACE_SCHED_DIAG_ENTER: u16 = 1;
#[cfg(target_arch = "aarch64")]
const TRACE_SCHED_DIAG_CURRENT_READY: u16 = 2;
#[cfg(target_arch = "aarch64")]
const TRACE_SCHED_DIAG_PICK: u16 = 3;
#[cfg(target_arch = "aarch64")]
const TRACE_SCHED_DIAG_RETURN: u16 = 4;
#[cfg(target_arch = "aarch64")]
const TRACE_SCHED_DIAG_RETURN_NONE: u16 = 5;
const TRACE_SCHED_DIAG_WAKE_TIMER_CURRENT: u16 = 6;
const TRACE_SCHED_DIAG_WAKE_TIMER_ENQUEUE: u16 = 7;
const TRACE_SCHED_DIAG_WAKE_TIMER_DEFERRED: u16 = 8;
const TRACE_SCHED_DIAG_WAKE_TIMER_ALREADY_QUEUED: u16 = 9;

#[cfg(target_arch = "aarch64")]
#[inline(always)]
fn trace_sched_diag(stage: u16, tid: u64, old_id: u64, new_id: u64, flags: u32) {
    crate::tracing::record_event(
        crate::tracing::TraceEventType::SCHED_DIAG_STAGE,
        0,
        ((stage as u32) << 16) | ((tid as u32) & 0xFFFF),
    );
    crate::tracing::record_event(
        crate::tracing::TraceEventType::SCHED_DIAG_TID_PAIR,
        0,
        (((old_id as u32) & 0xFFFF) << 16) | ((new_id as u32) & 0xFFFF),
    );
    crate::tracing::record_event(crate::tracing::TraceEventType::SCHED_DIAG_FLAGS, 0, flags);
}

#[cfg(not(target_arch = "aarch64"))]
#[inline(always)]
fn trace_sched_diag(_stage: u16, _tid: u64, _old_id: u64, _new_id: u64, _flags: u32) {}

pub fn emit_wake_attribution_counters() {
    crate::serial_println!(
        "[wake-attrib] schedule={} unblock={} isr_unblock={} wake_io={} signal={} child={} timer={}",
        WAKE_SITE_SCHEDULE.load(Ordering::Relaxed),
        WAKE_SITE_UNBLOCK.load(Ordering::Relaxed),
        WAKE_SITE_ISR_UNBLOCK.load(Ordering::Relaxed),
        WAKE_SITE_WAKE_IO_LOCKED.load(Ordering::Relaxed),
        WAKE_SITE_SIGNAL.load(Ordering::Relaxed),
        WAKE_SITE_CHILD_EXIT.load(Ordering::Relaxed),
        WAKE_SITE_TIMER.load(Ordering::Relaxed)
    );
    crate::serial_println!(
        "[enqueue-attrib] same_lock={} deferred={} isr_buf={} isr_buf_dedup={} deferred_drained={} isr_buf_drained={} already_queued={} isr_buf_full={}",
        ENQUEUE_SAME_LOCK_OK.load(Ordering::Relaxed),
        ENQUEUE_DEFERRED.load(Ordering::Relaxed),
        ENQUEUE_ISR_BUFFER.load(Ordering::Relaxed),
        ENQUEUE_ISR_BUFFER_DEDUP.load(Ordering::Relaxed),
        ENQUEUE_DEFERRED_DRAINED_OK.load(Ordering::Relaxed),
        ENQUEUE_ISR_BUFFER_DRAINED_OK.load(Ordering::Relaxed),
        ENQUEUE_ALREADY_QUEUED_OK.load(Ordering::Relaxed),
        ENQUEUE_ISR_BUFFER_FULL.load(Ordering::Relaxed)
    );
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnblockOutcome {
    /// The thread changed from a blocked state to Ready.
    Transitioned,
    /// The thread exists and is already Ready or Running.
    AlreadyRunnable,
    /// No state handled by `unblock()` matched.
    ///
    /// This includes missing or terminated threads and threads whose state has
    /// a dedicated wake path, such as `BlockedOnChildExit`.
    NotFound,
}

/// Global scheduler instance
static SCHEDULER: Mutex<Option<Scheduler>> = Mutex::new(None);

#[cfg(target_arch = "aarch64")]
#[inline(always)]
fn arch_can_dispatch_here() -> bool {
    crate::arch_impl::aarch64::context_switch::can_dispatch_here()
}

#[cfg(not(target_arch = "aarch64"))]
#[inline(always)]
fn arch_can_dispatch_here() -> bool {
    true
}

#[cfg(all(target_arch = "aarch64", feature = "boot_tests"))]
static BOOT_TEST_CPU_AFFINITY: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];

#[inline(always)]
fn lock_scheduler() -> spin::MutexGuard<'static, Option<Scheduler>> {
    let guard = SCHEDULER.lock();
    crate::tracing::providers::teardown::note_scheduler_acquire();
    guard
}

#[inline(always)]
fn try_lock_scheduler() -> Option<spin::MutexGuard<'static, Option<Scheduler>>> {
    let guard = SCHEDULER.try_lock()?;
    crate::tracing::providers::teardown::note_scheduler_acquire();
    Some(guard)
}

#[cfg(all(target_arch = "aarch64", feature = "boot_tests"))]
fn retain_cpu_affine_test_thread(
    queue: &mut VecDeque<u64>,
    thread_id: u64,
    current_cpu: usize,
) -> bool {
    // Zero means "no pinned thread" in BOOT_TEST_CPU_AFFINITY, and 0 is also the
    // no-thread sentinel: no live thread carries it, so a zero here can only be
    // an empty affinity slot.
    if thread_id == 0 {
        return false;
    }
    let target_cpu = BOOT_TEST_CPU_AFFINITY
        .iter()
        .position(|slot| slot.load(Ordering::Acquire) == thread_id);
    if target_cpu.is_none() || target_cpu == Some(current_cpu) {
        return false;
    }
    queue.push_back(thread_id);
    true
}

/// Threads work-stealing declined to take because their saved kernel SP stands
/// in another CPU's per-CPU stack slot. Never reset; reported in the fatal
/// postmortem next to the custody refusals.
#[cfg(target_arch = "aarch64")]
pub static PERCPU_STACK_SELECTION_ROUTED: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);

/// Total steals declined for per-CPU stack custody.
#[cfg(target_arch = "aarch64")]
pub fn percpu_stack_selection_routed() -> u64 {
    PERCPU_STACK_SELECTION_ROUTED.load(Ordering::Acquire)
}

#[cfg(all(target_arch = "aarch64", feature = "boot_tests"))]
pub(crate) fn clear_cpu_affinity_for_test(thread_id: u64) {
    crate::tracing::providers::teardown::record_kthread_exit_stage_for_test(thread_id);
    for slot in BOOT_TEST_CPU_AFFINITY.iter() {
        let _ = slot.compare_exchange(thread_id, 0, Ordering::AcqRel, Ordering::Relaxed);
    }
    crate::tracing::providers::teardown::record_kthread_exit_stage_for_test(thread_id);
}

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

#[derive(Clone, Copy, Default)]
struct IoWakeResult {
    enqueued_target: Option<usize>,
    current_cpu: Option<usize>,
}

impl IoWakeResult {
    fn resched_target(self) -> Option<usize> {
        self.enqueued_target.or(self.current_cpu)
    }
}

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
    lock_scheduler()
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

/// Maximum number of unreachable runnable threads retained by one strand-oracle
/// census. The detector keeps the corresponding dwell state in a fixed-size
/// array as well, so sampling never allocates while holding SCHEDULER.
#[cfg(feature = "boot_tests")]
pub const STRAND_CENSUS_CAPACITY: usize = 16;

#[cfg(feature = "boot_tests")]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum StrandShape {
    Running,
    Ready,
}

#[cfg(feature = "boot_tests")]
#[derive(Clone, Copy)]
pub struct StrandCandidate {
    pub tid: u64,
    pub shape: StrandShape,
    pub privilege: ThreadPrivilege,
    pub state: ThreadState,
}

#[cfg(feature = "boot_tests")]
#[derive(Clone, Copy)]
pub struct StrandCensus {
    pub checked: u64,
    pub candidates: usize,
    pub overflow: u64,
    pub worst_nonprogress_ms: u64,
    pub nonprogress: usize,
    pub queued_on_nondispatching_cpu: u64,
    /// Longest queued nonprogress interval, derived from the owning CPU's
    /// scheduler silence rather than a per-thread enqueue stamp. A true
    /// `ready_since_ticks` would require a write at all 23 per-CPU queue push
    /// sites, several on the context-switch hot path; the derived form measures
    /// the same nonprogress without adding a hot-path write.
    pub worst_queued_nondispatch_ms: u64,
    pub worst_cpu_scheduler_silence_ms: u64,
    pub worst_silence_cpu: u64,
}

/// Independent nonprogress/nondispatch/silence fields declared by `StrandCensus`.
/// Structural tests keep this runtime value equal to the declaration-derived count.
#[cfg(feature = "boot_tests")]
pub const STRAND_CENSUS_PROGRESS_AXES: usize = 6;

/// Return the CPU whose registered idle thread has `tid`.
///
/// No live thread carries id 0, so a zero `idle_thread` is unambiguously
/// `EMPTY_STATE`'s empty-slot sentinel and can no longer collide with a real
/// idle thread. CPU 0 therefore needs no special case: its idle thread has an
/// ordinary allocated id like every other CPU's.
///
/// The `!= 0` test stays because it is the sentinel rule, not the CPU-0
/// workaround: slots for CPUs that never came online still hold 0, and an
/// unregistered slot must not answer a lookup.
#[cfg(feature = "boot_tests")]
fn registered_idle_cpu(scheduler: &Scheduler, tid: u64) -> Option<usize> {
    scheduler
        .cpu_state
        .iter()
        .position(|cpu| cpu.idle_thread != 0 && cpu.idle_thread == tid)
}

/// Read-only probe for the per-CPU stack custody oracle's leg D: the thread id
/// registered as CPU 0's idle thread, and whether thread id 0 resolves to a
/// registered idle thread.
///
/// The second answer deliberately goes through `registered_idle_cpu` rather
/// than re-implementing it, so that a change to that helper is visible here
/// instead of being masked by a copy — including the removal of its CPU-0
/// special case.
#[cfg(feature = "percpu_stack_custody_oracle")]
pub fn zero_tid_idle_probe() -> Option<(u64, bool)> {
    with_scheduler(|scheduler| {
        let swapper_tid = scheduler.cpu_state[0].idle_thread;
        let zero_resolves = registered_idle_cpu(scheduler, 0).is_some();
        (swapper_tid, zero_resolves)
    })
}

/// Measurement-only snapshot of the scheduler's current online CPU count.
///
/// This takes the scheduler lock through `with_scheduler`, so it is for
/// boot-test accounting only and must not be called from interrupt, syscall,
/// context-switch or allocator hot paths.
#[cfg(feature = "boot_tests")]
pub fn online_cpu_count_snapshot() -> usize {
    with_scheduler(|scheduler| scheduler.online_cpu_count()).unwrap_or(1)
}

#[cfg(feature = "coreproof")]
pub struct DepartureProbe {
    pub queued_after_block: bool,
    pub state_is_blocked: bool,
    pub unblock_outcome: UnblockOutcome,
    pub membership_changed: bool,
    pub cardinality_before: usize,
    pub cardinality_after: usize,
}

/// Exercise and restore the generic ready-queue departure protocol under one
/// scheduler-lock acquisition with interrupts masked.
///
/// The name is load-bearing, not decorative. #647's caller-side rule
/// (`tests/teardown_structure.rs::validate_block_family_callers_own_no_departure`)
/// forbids any CALLER of the blocking family from open-coding a ready-queue
/// departure, and exempts the family's own members because they are the owners.
/// This probe plants and removes a queue entry ON PURPOSE — it manufactures the
/// one state that makes the departure observable — so it belongs on the same
/// side of that rule as `block_current_departure_gate`, and carries a
/// family name to say so. Renaming it out of the family would make it a caller
/// that open-codes a departure, which is exactly what the rule is for.
#[cfg(feature = "coreproof")]
pub fn block_current_coreproof_probe() -> Option<Result<DepartureProbe, &'static str>> {
    with_scheduler(|scheduler| {
        let cpu = Scheduler::current_cpu_id();
        let Some(tid) = scheduler.cpu_state[cpu].current_thread else {
            return Err("departure probe ran with no current thread");
        };
        let Some(thread) = scheduler.get_thread(tid) else {
            return Err("departure probe found no current thread row");
        };
        let restore_state = thread.state;
        let restore_in_syscall = thread.blocked_in_syscall;
        if !matches!(restore_state, ThreadState::Running | ThreadState::Ready) {
            return Err("departure probe current thread was not runnable");
        }
        if scheduler
            .per_cpu_queues
            .iter()
            .any(|queue| queue.contains(&tid))
        {
            return Err("departure probe current thread already named a ready queue");
        }

        let cardinality_before = scheduler
            .per_cpu_queues
            .iter()
            .map(VecDeque::len)
            .sum();
        scheduler.per_cpu_queues[cpu].push_back(tid);
        scheduler.block_current();

        let queued_after_block = scheduler
            .per_cpu_queues
            .iter()
            .any(|queue| queue.contains(&tid));
        let state_is_blocked = scheduler
            .get_thread(tid)
            .is_some_and(|thread| thread.state == ThreadState::Blocked);

        // Restore the planted membership and thread fields before exercising
        // runnable-unblock idempotence. No fallible exit exists after planting.
        for queue in scheduler.per_cpu_queues.iter_mut() {
            queue.retain(|queued_tid| *queued_tid != tid);
        }
        if let Some(thread) = scheduler.get_thread_mut(tid) {
            thread.state = restore_state;
            thread.blocked_in_syscall = restore_in_syscall;
        }

        let membership_before_unblock = core::array::from_fn::<_, MAX_CPUS, _>(|queue| {
            scheduler.per_cpu_queues[queue].contains(&tid)
        });
        let unblock_outcome = scheduler.unblock(tid);
        let membership_changed = scheduler
            .per_cpu_queues
            .iter()
            .zip(membership_before_unblock)
            .any(|(queue, was_member)| queue.contains(&tid) != was_member);
        let cardinality_after = scheduler
            .per_cpu_queues
            .iter()
            .map(VecDeque::len)
            .sum();

        Ok(DepartureProbe {
            queued_after_block,
            state_is_blocked,
            unblock_outcome,
            membership_changed,
            cardinality_before,
            cardinality_after,
        })
    })
}

/// Collect one fixed-size strand census under the scheduler lock.
///
/// The caller owns the array and processes it after this function returns, so
/// dwell bookkeeping and serial output are both outside the lock hold. The
/// only AArch64-specific reachability source is the lock-free deferred-requeue
/// slot; the rest of the census is shared with x86_64.
#[cfg(feature = "boot_tests")]
pub fn collect_strand_census(
    out: &mut [StrandCandidate; STRAND_CENSUS_CAPACITY],
    nonprogress_out: &mut [u64; STRAND_CENSUS_CAPACITY],
) -> Option<StrandCensus> {
    with_scheduler(|scheduler| {
        let mut checked = 0u64;
        let mut candidates = 0usize;
        let mut overflow = 0u64;
        let mut worst_nonprogress_ms = 0u64;
        let mut nonprogress = 0usize;
        let mut queued_on_nondispatching_cpu = 0u64;
        let mut worst_queued_nondispatch_ms = 0u64;
        let mut worst_cpu_scheduler_silence_ms = 0u64;
        let mut worst_silence_cpu = 0u64;
        let now_ticks = crate::time::get_ticks();
        let online_cpu_count = scheduler.online_cpu_count();

        for cpu in 0..online_cpu_count {
            let silence_ms = now_ticks.wrapping_sub(scheduler.cpu_state[cpu].last_schedule_ticks);
            if silence_ms > worst_cpu_scheduler_silence_ms {
                worst_cpu_scheduler_silence_ms = silence_ms;
                worst_silence_cpu = cpu as u64;
            }
        }

        for thread in scheduler.threads.iter() {
            let shape = match thread.state {
                ThreadState::Running => StrandShape::Running,
                ThreadState::Ready => StrandShape::Ready,
                _ => continue,
            };
            let tid = thread.id();
            let actual_idle_cpu = registered_idle_cpu(scheduler, tid);
            let dormant_idle = actual_idle_cpu.is_some_and(|cpu_id| {
                let cpu = &scheduler.cpu_state[cpu_id];
                #[cfg(target_arch = "aarch64")]
                let parked = is_cpu_idle(cpu_id);
                #[cfg(not(target_arch = "aarch64"))]
                let parked = false;
                let dormancy_dimensions = [
                    parked,
                    cpu.current_thread
                        .is_some_and(|current_tid| current_tid != tid),
                ];
                dormancy_dimensions.into_iter().any(|dormant| dormant)
            });
            if dormant_idle {
                continue;
            }

            checked += 1;
            let current = scheduler
                .cpu_state
                .iter()
                .any(|cpu| cpu.current_thread == Some(tid));
            let mut queued = false;
            let mut queued_nondispatch_cpu = None;
            for cpu in 0..MAX_CPUS {
                if !scheduler.per_cpu_queues[cpu].contains(&tid) {
                    continue;
                }
                queued = true;
                if thread.state == ThreadState::Ready
                    && queued_nondispatch_cpu.is_none()
                    && (cpu >= online_cpu_count || scheduler.cpu_dispatch_stale(cpu))
                {
                    queued_nondispatch_cpu = Some(cpu);
                }
            }

            if let Some(cpu) = queued_nondispatch_cpu {
                queued_on_nondispatching_cpu += 1;
                let queued_nondispatch_ms =
                    now_ticks.wrapping_sub(scheduler.cpu_state[cpu].last_schedule_ticks);
                worst_queued_nondispatch_ms =
                    worst_queued_nondispatch_ms.max(queued_nondispatch_ms);
                if nonprogress < STRAND_CENSUS_CAPACITY {
                    nonprogress_out[nonprogress] = tid;
                    nonprogress += 1;
                }
            }

            let previous = scheduler
                .cpu_state
                .iter()
                .any(|cpu| cpu.previous_thread == Some(tid));

            let pending_next = scheduler
                .cpu_state
                .iter()
                .any(|cpu| cpu.pending_next == Some(tid));

            #[cfg(target_arch = "aarch64")]
            let deferred =
                crate::arch_impl::aarch64::context_switch::deferred_requeue_contains(tid);
            #[cfg(not(target_arch = "aarch64"))]
            let deferred = false;

            if thread.state == ThreadState::Running && actual_idle_cpu.is_none() && !current {
                let nonprogress_ms = now_ticks.saturating_sub(thread.run_start_ticks);
                worst_nonprogress_ms = worst_nonprogress_ms.max(nonprogress_ms);
                if nonprogress < STRAND_CENSUS_CAPACITY {
                    nonprogress_out[nonprogress] = tid;
                    nonprogress += 1;
                }
            }

            let reachability_dimensions = [current, queued, previous, pending_next, deferred];
            if reachability_dimensions
                .into_iter()
                .any(|reachable| reachable)
            {
                continue;
            }

            if candidates < STRAND_CENSUS_CAPACITY {
                out[candidates] = StrandCandidate {
                    tid,
                    shape,
                    privilege: thread.privilege,
                    state: thread.state,
                };
                candidates += 1;
            } else {
                overflow += 1;
            }
        }

        StrandCensus {
            checked,
            candidates,
            overflow,
            worst_nonprogress_ms,
            nonprogress,
            queued_on_nondispatching_cpu,
            worst_queued_nondispatch_ms,
            worst_cpu_scheduler_silence_ms,
            worst_silence_cpu,
        }
    })
}

/// Select a non-current CPU whose scheduler-entry timestamp is stale.
#[cfg(all(target_arch = "aarch64", feature = "boot_tests"))]
pub fn stale_peer_cpu_for_test() -> Option<usize> {
    with_scheduler(|scheduler| {
        let current_cpu = Scheduler::current_cpu_id();
        (0..MAX_CPUS).find(|&cpu| cpu != current_cpu && scheduler.cpu_dispatch_stale(cpu))
    })
    .flatten()
}

/// Lock-free-consumer liveness snapshot for watchdog diagnostics.
///
/// Collected with `SCHEDULER.try_lock()` so callers never block on the
/// scheduler lock while trying to report a wedge.
#[derive(Clone, Copy, Default)]
pub struct SchedulerLivenessSnapshot {
    pub current_thread_id: u64,
    pub ready_queue_len: u64,
    pub total_threads: u64,
    pub blocked_count: u64,
    pub per_cpu_ready_len: [u64; 8],
    pub per_cpu_current: [u64; 8],
}

/// Try to snapshot scheduler state without blocking.
///
/// Returns `None` when the scheduler lock is busy or uninitialized; both are
/// diagnostic for watchdog output.
pub fn try_liveness_snapshot(cpu_id: usize) -> Option<SchedulerLivenessSnapshot> {
    let guard = try_lock_scheduler()?;
    let sched = guard.as_ref()?;
    let cpu = cpu_id.min(MAX_CPUS.saturating_sub(1));

    let mut per_cpu_ready_len = [0u64; 8];
    let mut per_cpu_current = [0u64; 8];
    for idx in 0..MAX_CPUS.min(8) {
        per_cpu_ready_len[idx] = sched.per_cpu_queues[idx].len() as u64;
        per_cpu_current[idx] = sched.cpu_state[idx].current_thread.unwrap_or(0);
    }

    let ready_queue_len = sched.per_cpu_queues.iter().map(|q| q.len()).sum::<usize>() as u64;
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

    Some(SchedulerLivenessSnapshot {
        current_thread_id: sched.cpu_state[cpu].current_thread.unwrap_or(0),
        ready_queue_len,
        total_threads: sched.threads.len() as u64,
        blocked_count,
        per_cpu_ready_len,
        per_cpu_current,
    })
}

/// Try to get a snapshot of scheduler state without blocking.
/// Returns None if the scheduler lock is held (which is itself diagnostic).
/// Safe to call from interrupt context.
pub fn try_dump_state() -> Option<SchedulerDumpInfo> {
    let guard = try_lock_scheduler()?;
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
pub(crate) const MAX_CPUS: usize = 8;
#[cfg(not(target_arch = "aarch64"))]
pub(crate) const MAX_CPUS: usize = 1;

/// A CPU that has not entered the scheduler in 20 ms will not dispatch a wake
/// within any latency budget the kernel cares about.
const CPU_STALL_TICKS: u64 = 20;

/// Scheduler-entry epochs per online CPU.
///
/// A retiring resource records a target two greater than every online CPU's
/// current value. The first bump may be recorded by a handoff that is already
/// in flight on the retiring stack. Requiring a second bump proves that CPU
/// entered the scheduler through a later exception, which can only happen
/// after the in-flight exception return and its old-stack restore completed.
static SCHEDULING_EPOCHS: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];

#[derive(Clone, Copy)]
struct RetirementGrace {
    thread_id: u64,
    after_epoch: RetirementFence,
}

#[derive(Clone, Copy)]
pub(crate) struct RetirementFence {
    pub(crate) epochs: [u64; MAX_CPUS],
    pub(crate) online_mask: u64,
}

#[derive(Clone, Copy)]
pub(crate) struct RetirementSnapshot {
    pub(crate) epochs: [u64; MAX_CPUS],
    pub(crate) online_mask: u64,
}

impl RetirementSnapshot {
    pub(crate) fn capture() -> Self {
        let mut epochs = [0; MAX_CPUS];
        let mut online_mask = 0;
        for cpu_id in 0..MAX_CPUS {
            epochs[cpu_id] = SCHEDULING_EPOCHS[cpu_id].load(Ordering::Acquire);
            #[cfg(target_arch = "aarch64")]
            if crate::arch_impl::aarch64::smp::is_cpu_online(cpu_id) {
                online_mask |= 1 << cpu_id;
            }
            #[cfg(target_arch = "x86_64")]
            {
                // x86 is single-CPU today. Keep CPU 0 unconditionally live so
                // retirement can never silently degenerate to an empty mask.
                online_mask |= 1 << cpu_id;
            }
        }
        core::sync::atomic::fence(Ordering::Acquire);
        Self {
            epochs,
            online_mask,
        }
    }

    pub(crate) fn as_fence(self) -> RetirementFence {
        RetirementFence {
            epochs: self.epochs,
            online_mask: self.online_mask,
        }
    }

    fn target_after(self, advances: u64) -> RetirementFence {
        let mut epochs = self.epochs;
        for (cpu_id, epoch) in epochs.iter_mut().enumerate() {
            if self.online_mask & (1 << cpu_id) != 0 {
                *epoch = epoch.wrapping_add(advances);
            }
        }
        RetirementFence {
            epochs,
            online_mask: self.online_mask,
        }
    }

    pub(crate) fn fence_elapsed(&self, fence: &RetirementFence) -> bool {
        if fence.online_mask == 0 {
            crate::trace_count!(crate::tracing::providers::teardown::RETIRE_EMPTY_ONLINE_MASK);
            return false;
        }
        (0..MAX_CPUS).all(|cpu_id| {
            fence.online_mask & (1 << cpu_id) == 0
                || epoch_reached(self.epochs[cpu_id], fence.epochs[cpu_id])
        })
    }

    pub(crate) fn all_advanced_since(&self, fence: &RetirementFence) -> bool {
        fence.online_mask != 0
            && (0..MAX_CPUS).all(|cpu_id| {
                fence.online_mask & (1 << cpu_id) == 0
                    || epoch_advanced(self.epochs[cpu_id], fence.epochs[cpu_id])
            })
    }

    pub(crate) fn epoch_sum(&self, online_mask: u64) -> u64 {
        (0..MAX_CPUS)
            .filter(|cpu_id| online_mask & (1 << cpu_id) != 0)
            .fold(0u64, |sum, cpu_id| sum.wrapping_add(self.epochs[cpu_id]))
    }
}

#[inline(always)]
fn epoch_reached(now: u64, target: u64) -> bool {
    now.wrapping_sub(target) < (1u64 << 63)
}

#[inline(always)]
fn epoch_advanced(now: u64, before: u64) -> bool {
    let delta = now.wrapping_sub(before);
    delta != 0 && delta < (1u64 << 63)
}

pub(crate) fn retirement_grace_target() -> RetirementFence {
    RetirementSnapshot::capture().target_after(2)
}

pub(crate) fn retirement_grace_elapsed(target: &RetirementFence) -> bool {
    RetirementSnapshot::capture().fence_elapsed(target)
}

/// Record a scheduler entry for the current CPU.
///
/// A single entry does not prove the handoff active at that entry has finished;
/// reclamation targets require a second, subsequent entry on every online CPU.
pub fn note_scheduling_epoch(cpu_id: usize) {
    if cpu_id < MAX_CPUS {
        SCHEDULING_EPOCHS[cpu_id].fetch_add(1, Ordering::Release);
    }
}

/// DIAGNOSTIC: Circular buffer tracking last N cpu_state changes per CPU.
/// Each entry: (setter_id, old_thread, new_thread)
/// Setter IDs:
///   1 = commit_cpu_state_after_save
///   2 = switch_to_idle
///   3 = switch_to_idle_best_effort
///   4 = register_idle_thread
///   5 = init_with_current
///   6 = Scheduler::set_current_thread
///   7 = fix_stale_current_thread_when_idle_executing (schedule_from_kernel
///       idle path -- corrects a stale non-idle current_thread while idle is
///       the thread actually executing, before it can be used as a save
///       target; see ROOT_CAUSE.md's cpu_state/old_id skew candidate)
///   8 = fix_stale_idle_cpu_state
///   9 = fix_exception_cleanup_cpu_state
/// 10-15 = retired path-specific dispatch_thread_locked idle-redirect setters
///  16 = Scheduler::add_thread_as_current
///  17 = Scheduler::terminate_current (new_thread = 0xDEAD means None)
///  18 = Scheduler::new (initial CPU 0 idle thread)
///  19 = setup_idle_return_locked (every exception-frame redirect to idle)
///  20 = drain_asm_resume_pc_refusals (the refusal drain republishing idle over
///       a refused dispatch's own victim, before it terminates that victim)
#[cfg(target_arch = "aarch64")]
const HISTORY_SIZE: usize = 256;
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

#[cfg(all(target_arch = "aarch64", feature = "ec0_fault_inject"))]
static EC0_FAULT_INJECT_TID: AtomicU64 = AtomicU64::new(0);
#[cfg(all(target_arch = "aarch64", feature = "ec0_fault_inject"))]
static EC0_FAULT_INJECT_DEADLINE: AtomicU64 = AtomicU64::new(0);

#[cfg(all(target_arch = "aarch64", feature = "ec0_fault_inject"))]
#[inline(never)]
fn retain_ec0_fault_inject_on_cpu0(
    queue: &mut VecDeque<u64>,
    thread_id: u64,
    current_cpu: usize,
) -> bool {
    if current_cpu == 0 || EC0_FAULT_INJECT_TID.load(Ordering::Acquire) != thread_id {
        return false;
    }
    queue.push_front(thread_id);
    true
}

#[cfg(all(target_arch = "aarch64", feature = "ec0_fault_inject"))]
#[cold]
#[inline(never)]
extern "C" fn ec0_fault_inject_thread(_arg: u64) -> ! {
    let deadline = EC0_FAULT_INJECT_DEADLINE.load(Ordering::Acquire);
    loop {
        let now: u64;
        unsafe {
            core::arch::asm!(
                "mrs {}, cntvct_el0",
                out(reg) now,
                options(nomem, nostack, preserves_flags),
            );
        }
        if now >= deadline {
            break;
        }
        unsafe {
            core::arch::asm!("wfi", options(nomem, nostack));
        }
    }

    if current_cpu_id_raw() != 0 {
        loop {
            unsafe {
                core::arch::asm!("wfi", options(nomem, nostack));
            }
        }
    }

    unsafe {
        core::arch::asm!("udf #0x1234", options(noreturn));
    }
}

#[cfg(all(target_arch = "aarch64", feature = "ec0_fault_inject"))]
#[cold]
#[inline(never)]
fn install_ec0_fault_inject_thread(scheduler: &mut Scheduler) {
    let start: u64;
    let frequency: u64;
    unsafe {
        core::arch::asm!(
            "mrs {start}, cntvct_el0",
            "mrs {frequency}, cntfrq_el0",
            start = out(reg) start,
            frequency = out(reg) frequency,
            options(nomem, nostack, preserves_flags),
        );
    }
    let deadline = start.saturating_add(frequency.saturating_mul(10));
    EC0_FAULT_INJECT_DEADLINE.store(deadline, Ordering::Release);

    let thread = Thread::new_kernel(
        alloc::string::String::from("ec0_fault_inject/0"),
        ec0_fault_inject_thread,
        0,
    )
    .unwrap_or_else(|error| panic!("failed to create EC0 fault injector: {}", error));
    let thread_id = thread.id();
    EC0_FAULT_INJECT_TID.store(thread_id, Ordering::Release);
    scheduler.threads.push(Box::new(thread));
    scheduler.per_cpu_queues[0].push_back(thread_id);
}

/// Record a cpu_state change for diagnostics (circular buffer).
#[cfg(target_arch = "aarch64")]
#[inline(never)]
pub(crate) fn record_cpu_state_change(cpu: usize, setter_id: u64, old_val: u64, new_val: u64) {
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

/// Dump the cpu_state change history for the aarch64 UNHANDLED_EC fatal
/// postmortem (exception.rs): the faulting CPU, plus every OTHER CPU whose
/// SAVE_SKEW slot fired (queried via `save_skew_snapshot`, the same read-side
/// accessor `dump_all_save_skew_snapshots` uses) since a bad save recorded on
/// a peer CPU is exactly the case this history is needed to explain. Kept as
/// its own function (rather than inlined at the call site) so the call site
/// in exception.rs stays a single line. Lock-free (atomics only, no locks)
/// and record-only -- no behavior change.
#[cfg(target_arch = "aarch64")]
pub fn dump_cpu_state_history_postmortem(faulting_cpu: usize) {
    dump_cpu_state_history(faulting_cpu);
    for other_cpu in 0..MAX_CPUS {
        if other_cpu != faulting_cpu
            && crate::arch_impl::aarch64::context_switch::save_skew_snapshot(other_cpu).is_some()
        {
            dump_cpu_state_history(other_cpu);
        }
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
    /// Incoming thread published by schedule_deferred_requeue but not yet
    /// committed after its context save.
    #[cfg_attr(not(target_arch = "aarch64"), allow(dead_code))]
    pub(crate) pending_next: Option<u64>,
    /// Most recent tick at which this CPU entered a scheduling path.
    pub(crate) last_schedule_ticks: u64,
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

    /// Per-thread all-CPU grace targets for kernel-stack reclamation.
    retirement_grace: alloc::vec::Vec<RetirementGrace>,
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
            pending_next: None,
            last_schedule_ticks: 0,
        };
        let mut cpu_state = [EMPTY_STATE; MAX_CPUS];
        #[cfg(target_arch = "aarch64")]
        {
            let old_val = cpu_state[0].current_thread.unwrap_or(0xDEAD);
            record_cpu_state_change(0, 18, old_val, idle_id);
        }
        cpu_state[0] = CpuSchedulerState {
            current_thread: Some(idle_id),
            idle_thread: idle_id,
            previous_thread: None,
            pending_next: None,
            last_schedule_ticks: crate::time::get_ticks(),
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
            retirement_grace: alloc::vec::Vec::new(),
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

    /// Number of CPUs that can actually schedule. Placement must never target
    /// a queue above this: nothing runs there and the reschedule IPI helpers
    /// (correctly) refuse offline targets, so a thread parked there starves.
    #[inline(always)]
    fn online_cpu_count(&self) -> usize {
        #[cfg(target_arch = "aarch64")]
        {
            (crate::arch_impl::aarch64::smp::cpus_online() as usize).clamp(1, MAX_CPUS)
        }
        #[cfg(not(target_arch = "aarch64"))]
        {
            MAX_CPUS
        }
    }

    /// Whether a CPU is online and able to dispatch newly runnable work.
    ///
    /// The current CPU is accepted immediately only when its architecture can
    /// enter the scheduler; otherwise it is judged by the same external
    /// scheduler-entry staleness test as every peer.
    #[inline(always)]
    fn cpu_accepts_wakeups(&self, cpu: usize) -> bool {
        if cpu >= self.online_cpu_count() {
            return false;
        }
        if cpu == Self::current_cpu_id() && arch_can_dispatch_here() {
            return true;
        }

        !self.cpu_dispatch_stale(cpu)
    }

    /// The staleness test, judged from outside the CPU: has it entered the
    /// scheduler recently enough to dispatch newly runnable work?
    #[inline(always)]
    fn cpu_dispatch_stale(&self, cpu: usize) -> bool {
        let last_schedule_ticks = self.cpu_state[cpu].last_schedule_ticks;
        crate::time::get_ticks().wrapping_sub(last_schedule_ticks) > CPU_STALL_TICKS
    }

    /// Migrate runnable work from offline or stalled CPUs onto this CPU.
    fn reclaim_unschedulable_cpu_queues(&mut self) {
        let online_cpus = self.online_cpu_count();
        let current_cpu = Self::current_cpu_id();

        for cpu in 0..MAX_CPUS {
            if cpu == current_cpu {
                continue;
            }
            let offline = cpu >= online_cpus;
            let stalled = !offline && !self.cpu_accepts_wakeups(cpu);
            if !offline && !stalled {
                continue;
            }

            let candidates = self.per_cpu_queues[cpu].len();
            let mut reclaimed = 0u64;
            for _ in 0..candidates {
                let Some(thread_id) = self.per_cpu_queues[cpu].pop_front() else {
                    break;
                };
                #[cfg(all(target_arch = "aarch64", feature = "boot_tests"))]
                if retain_cpu_affine_test_thread(
                    &mut self.per_cpu_queues[cpu],
                    thread_id,
                    current_cpu,
                ) {
                    continue;
                }
                self.per_cpu_queues[current_cpu].push_back(thread_id);
                reclaimed += 1;
            }

            if reclaimed != 0 {
                if offline {
                    ENQUEUE_OFFLINE_RECLAIMED.fetch_add(reclaimed, Ordering::Relaxed);
                } else {
                    ENQUEUE_STALLED_RECLAIMED.fetch_add(reclaimed, Ordering::Relaxed);
                }
            }
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
        let old_val = self.cpu_state[cpu_id].current_thread.unwrap_or(0xDEAD);
        record_cpu_state_change(cpu_id, 4, old_val, idle_id);
        self.cpu_state[cpu_id].current_thread = Some(idle_id);
        self.cpu_state[cpu_id].last_schedule_ticks = crate::time::get_ticks();
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

    #[cfg(all(target_arch = "aarch64", feature = "boot_tests"))]
    fn add_thread_on_cpu_for_test(&mut self, thread: Box<Thread>, cpu: usize) {
        debug_assert!(cpu < MAX_CPUS);
        let thread_id = thread.id();
        self.threads.push(thread);
        self.per_cpu_queues[cpu].push_back(thread_id);
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

    /// Drop terminated threads only after their stack is architecturally dead.
    ///
    /// Userspace kernel stacks are owned by scheduler threads because the
    /// scheduler clone can outlive the process-table copy until it has fully
    /// disappeared from CPU current/previous slots.
    pub fn reclaim_terminated_threads(&mut self) -> alloc::vec::Vec<alloc::boxed::Box<Thread>> {
        for queue in self.per_cpu_queues.iter_mut() {
            queue.retain(|&thread_id| self.threads.iter().any(|thread| thread.id() == thread_id));
        }

        let terminated_ids: alloc::vec::Vec<u64> = self
            .threads
            .iter()
            .filter(|thread| thread.state == ThreadState::Terminated)
            .map(|thread| thread.id())
            .collect();
        self.retirement_grace
            .retain(|grace| terminated_ids.contains(&grace.thread_id));
        for thread_id in terminated_ids.iter().copied() {
            if !self
                .retirement_grace
                .iter()
                .any(|grace| grace.thread_id == thread_id)
            {
                self.retirement_grace.push(RetirementGrace {
                    thread_id,
                    after_epoch: retirement_grace_target(),
                });
            }
        }

        let idle_ids: alloc::vec::Vec<u64> = self
            .cpu_state
            .iter()
            .map(|state| state.idle_thread)
            .collect();
        let graces = &self.retirement_grace;
        let mut reclaimed_ids = alloc::vec::Vec::new();
        let mut retained_threads = alloc::vec::Vec::with_capacity(self.threads.len());
        let mut reclaimed_threads = alloc::vec::Vec::new();
        for thread in self.threads.drain(..) {
            if thread.state != ThreadState::Terminated || idle_ids.contains(&thread.id()) {
                retained_threads.push(thread);
                continue;
            }

            let grace_elapsed = graces
                .iter()
                .find(|grace| grace.thread_id == thread.id())
                .map(|grace| retirement_grace_elapsed(&grace.after_epoch))
                .unwrap_or(false);
            if !grace_elapsed {
                retained_threads.push(thread);
                continue;
            }
            if thread
                .kernel_stack_top
                .map(|top| crate::memory::kernel_stack::is_kernel_stack_slot_live(top.as_u64()))
                .unwrap_or(false)
            {
                retained_threads.push(thread);
                continue;
            }

            reclaimed_ids.push(thread.id());
            reclaimed_threads.push(thread);
        }
        self.threads = retained_threads;
        self.retirement_grace
            .retain(|grace| !reclaimed_ids.contains(&grace.thread_id));
        for queue in self.per_cpu_queues.iter_mut() {
            queue.retain(|thread_id| !reclaimed_ids.contains(thread_id));
        }
        reclaimed_threads
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
        let cpu = Self::current_cpu_id();
        #[cfg(target_arch = "aarch64")]
        {
            let old_val = self.cpu_state[cpu].current_thread.unwrap_or(0xDEAD);
            record_cpu_state_change(cpu, 16, old_val, thread_id);
        }
        self.cpu_state[cpu].current_thread = Some(thread_id);
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
        let current_cpu = Self::current_cpu_id();
        self.cpu_state[current_cpu].last_schedule_ticks = crate::time::get_ticks();
        self.reclaim_unschedulable_cpu_queues();

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
                self.unblock_for_io_from_isr_buffer(tid);
            }
        }

        if crate::net::loopback_queue_has_work() {
            let pump_tid = crate::net::loopback_pump_tid();
            if pump_tid != 0 && self.unblock(pump_tid) == UnblockOutcome::Transitioned {
                crate::net::record_loopback_pump_rearm_from_sched();
            }
        }

        // Wake expired timers BEFORE reading the outgoing thread's disposition.
        //
        // #568 (second defect): this call used to sit AFTER the block below, and
        // the two together lost the wake outright. The block reads the outgoing
        // thread's state to decide whether to requeue it; a thread still marked
        // BlockedOnTimer is deliberately left off the ready queue. Only then did
        // wake_expired_timers() run -- and because this CPU has not yet selected
        // a successor, `cpu_state[cpu].current_thread` is still the outgoing
        // thread, so the pop took the "is_current_on_any_cpu" arm: it published
        // Ready, consumed the thread's only timer-heap entry, and deliberately
        // did NOT enqueue, on the documented assumption that either the running
        // thread would notice after its halt or DEFERRED_REQUEUE would catch it.
        // Neither holds here: the thread is being switched out in this very
        // call so it never runs to notice, and DEFERRED_REQUEUE together with
        // every requeue_thread_after_save() caller is aarch64-only. The thread
        // was left Ready, in no queue, with no heap entry -- stranded for good.
        //
        // sys_poll is the caller that made this deterministic: it re-arms a 1ms
        // slice, so its deadline has essentially always expired by the time
        // schedule() runs. nanosleep arms its full duration, so nothing pops in
        // this window and the enqueue happens cleanly later.
        //
        // Waking first removes the read-before-write inversion: the outgoing
        // thread is already Ready when the block below reads it, and the
        // existing enqueue there takes ownership. The wake path's SMP guard is
        // deliberately left exactly as it is -- this gives the wake an owner
        // rather than relaxing the rule that protects it.
        self.wake_expired_timers();

        // If current thread is still runnable, put it back in ready queue
        if let Some(current_id) = self.cpu_state[Self::current_cpu_id()].current_thread {
            if current_id != self.cpu_state[Self::current_cpu_id()].idle_thread {
                // Check the state and determine what to do
                let (is_terminated, is_blocked, published_ready) =
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
                        let published_ready = if !was_blocked && !was_terminated {
                            let now = crate::time::get_ticks();
                            current.cpu_ticks_total += now.wrapping_sub(current.run_start_ticks);
                            current.run_start_ticks = now;
                            current.set_ready();
                            WAKE_SITE_SCHEDULE.fetch_add(1, Ordering::Relaxed);
                            record_ready_site(current_id, READY_SITE_SCHEDULE);
                            true
                        } else {
                            // Reset run_start_ticks so the next dispatch doesn't
                            // charge stale time from the blocked period.
                            current.run_start_ticks = crate::time::get_ticks();
                            false
                        };

                        (was_terminated, was_blocked, published_ready)
                    } else {
                        (true, false, false)
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
                    if published_ready {
                        ENQUEUE_SAME_LOCK_OK.fetch_add(1, Ordering::Relaxed);
                    }
                } else if published_ready && in_queue {
                    ENQUEUE_ALREADY_QUEUED_OK.fetch_add(1, Ordering::Relaxed);
                }
            }
        }

        // Get next thread from ready queue (local first, then steal), skipping terminated.
        let current_cpu = Self::current_cpu_id();
        let mut next_thread_id = 'outer: loop {
            // Try local queue first
            let local_candidates = self.per_cpu_queues[current_cpu].len();
            for _ in 0..local_candidates {
                let Some(n) = self.per_cpu_queues[current_cpu].pop_front() else {
                    break;
                };
                let (terminated, owner_pid) = self
                    .get_thread(n)
                    .map(|thread| (thread.state == ThreadState::Terminated, thread.owner_pid))
                    .unwrap_or((false, None));
                if terminated {
                    if owner_pid.is_some_and(|pid| !observe_exit_kick(pid)) {
                        self.per_cpu_queues[current_cpu].push_back(n);
                    }
                    continue;
                }
                break 'outer n;
            }
            // Local queue empty — work-steal from other CPUs
            for steal_cpu in 0..MAX_CPUS {
                if steal_cpu == current_cpu {
                    continue;
                }
                let steal_candidates = self.per_cpu_queues[steal_cpu].len();
                for _ in 0..steal_candidates {
                    let Some(n) = self.per_cpu_queues[steal_cpu].pop_front() else {
                        break;
                    };
                    #[cfg(all(target_arch = "aarch64", feature = "boot_tests"))]
                    if retain_cpu_affine_test_thread(
                        &mut self.per_cpu_queues[steal_cpu],
                        n,
                        current_cpu,
                    ) {
                        continue;
                    }
                    #[cfg(all(target_arch = "aarch64", feature = "ec0_fault_inject"))]
                    if retain_ec0_fault_inject_on_cpu0(
                        &mut self.per_cpu_queues[steal_cpu],
                        n,
                        current_cpu,
                    ) {
                        break;
                    }
                    #[cfg(target_arch = "aarch64")]
                    if let Some(home) = self.percpu_stack_home_cpu(n, current_cpu) {
                        PERCPU_STACK_SELECTION_ROUTED.fetch_add(1, Ordering::Relaxed);
                        self.per_cpu_queues[home].push_back(n);
                        continue;
                    }
                    let (terminated, owner_pid) = self
                        .get_thread(n)
                        .map(|thread| (thread.state == ThreadState::Terminated, thread.owner_pid))
                        .unwrap_or((false, None));
                    if terminated {
                        if owner_pid.is_some_and(|pid| !observe_exit_kick(pid)) {
                            self.per_cpu_queues[steal_cpu].push_back(n);
                        }
                        continue;
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
                #[cfg(all(target_arch = "aarch64", feature = "ec0_fault_inject"))]
                let mut retained_injector = false;
                if let Some(n) = self.per_cpu_queues[current_cpu].pop_front() {
                    found = Some(n);
                } else {
                    for steal_cpu in 0..MAX_CPUS {
                        if steal_cpu == current_cpu {
                            continue;
                        }
                        if let Some(n) = self.per_cpu_queues[steal_cpu].pop_front() {
                            #[cfg(all(target_arch = "aarch64", feature = "boot_tests"))]
                            if retain_cpu_affine_test_thread(
                                &mut self.per_cpu_queues[steal_cpu],
                                n,
                                current_cpu,
                            ) {
                                continue;
                            }
                            #[cfg(all(target_arch = "aarch64", feature = "ec0_fault_inject"))]
                            if retain_ec0_fault_inject_on_cpu0(
                                &mut self.per_cpu_queues[steal_cpu],
                                n,
                                current_cpu,
                            ) {
                                retained_injector = true;
                                continue;
                            }
                            #[cfg(target_arch = "aarch64")]
                            if let Some(home) = self.percpu_stack_home_cpu(n, current_cpu) {
                                PERCPU_STACK_SELECTION_ROUTED.fetch_add(1, Ordering::Relaxed);
                                self.per_cpu_queues[home].push_back(n);
                                continue;
                            }
                            found = Some(n);
                            break;
                        }
                    }
                }
                #[cfg(all(target_arch = "aarch64", feature = "ec0_fault_inject"))]
                if found.is_none() && retained_injector {
                    // The injector remains on its CPU 0 queue. If no later peer is
                    // stealable, remove the queued duplicate of the already-running
                    // current thread and keep running that thread on this CPU.
                    if let Some(position) = self.per_cpu_queues[current_cpu]
                        .iter()
                        .position(|&id| id == next_thread_id)
                    {
                        if let Some(current_id) = self.per_cpu_queues[current_cpu].remove(position)
                        {
                            found = Some(current_id);
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

    #[cfg(target_arch = "aarch64")]
    pub(crate) fn resolve_pending_next_locked(&mut self, cpu: usize) {
        let Some(tid) = self.cpu_state[cpu].pending_next else {
            return;
        };
        crate::proof_cover!(PendingNext);

        // CORE-PROOF MUTATION LEG `coreproof_mut_pending_next` (#589, fixed by
        // PR #614): the incoming handoff is CONSUMED without being resolved.
        // The slot is cleared, nothing re-queues the thread, and no other
        // reachability source names it — the loseable handoff exactly. The
        // resolution below is compiled out wholesale rather than jumped over, so
        // the mutated build carries no unreachable tail and the unmutated build
        // carries no branch. Test profiles only.
        // Expected predicate: REDISPATCH_LIVENESS.
        #[cfg(feature = "coreproof_mut_pending_next")]
        {
            let _ = self.cpu_state[cpu].pending_next.take();
            let _ = tid;
        }

        #[cfg(not(feature = "coreproof_mut_pending_next"))]
        {
            if self
                .cpu_state
                .iter()
                .any(|state| state.idle_thread == tid)
                || self
                    .cpu_state
                    .iter()
                    .any(|state| state.current_thread == Some(tid))
                || self
                    .per_cpu_queues
                    .iter()
                    .any(|queue| queue.contains(&tid))
                || self
                    .cpu_state
                    .iter()
                    .any(|state| state.previous_thread == Some(tid))
                || crate::arch_impl::aarch64::context_switch::deferred_requeue_contains(
                    tid,
                )
            {
                return;
            }

            let Some(state) = self.get_thread(tid).map(|thread| thread.state) else {
                return;
            };
            if !matches!(state, ThreadState::Running | ThreadState::Ready) {
                return;
            }

            let Some(thread) = self.get_thread_mut(tid) else {
                return;
            };
            thread.set_ready();
            let _ = self.cpu_state[cpu].pending_next.take();
            self.per_cpu_queues[cpu].push_back(tid);
            crate::per_cpu_aarch64::set_need_resched(true);
            self.send_resched_ipi();
            #[cfg(feature = "boot_tests")]
            crate::task::strand_oracle::note_pending_next_resolved(tid);
        }
    }

    /// Stage the recoverable handoff state consumed by
    /// `resolve_pending_next_locked` and drive the real resolver.
    ///
    /// Normal scheduling commits `pending_next` before another scheduler entry,
    /// so merely calling `schedule_from_kernel` cannot exercise the recovery
    /// guard. The core-proof churn peer supplies a newly queued, CPU-affine
    /// kthread. Under this existing scheduler lock, the probe removes that
    /// thread from its queue, marks it selected, and makes `pending_next` its
    /// sole reachability source — the state the null-scheduler fallback leaves.
    /// The production resolver restores it to Ready and requeues it. The M4
    /// mutation consumes the slot and does neither, reproducing the loseable
    /// handoff rather than merely incrementing a counter near it.
    #[cfg(all(target_arch = "aarch64", feature = "coreproof"))]
    pub(crate) fn exercise_pending_next_coreproof_probe(&mut self, tid: u64) -> bool {
        let cpu = Self::current_cpu_id();
        if self.cpu_state[cpu].pending_next.is_some()
            || self
                .cpu_state
                .iter()
                .any(|state| state.current_thread == Some(tid))
            || self
                .cpu_state
                .iter()
                .any(|state| state.previous_thread == Some(tid))
            || crate::arch_impl::aarch64::context_switch::deferred_requeue_contains(tid)
            || !self
                .get_thread(tid)
                .is_some_and(|thread| thread.state == ThreadState::Ready)
        {
            return false;
        }

        let Some((queue_cpu, position)) = self.per_cpu_queues.iter().enumerate().find_map(
            |(queue_cpu, queue)| {
                queue
                    .iter()
                    .position(|queued_tid| *queued_tid == tid)
                    .map(|position| (queue_cpu, position))
            },
        ) else {
            return false;
        };
        if self.per_cpu_queues[queue_cpu].remove(position) != Some(tid) {
            return false;
        }

        let Some(thread) = self.get_thread_mut(tid) else {
            return false;
        };
        thread.set_running();
        self.cpu_state[cpu].pending_next = Some(tid);
        self.resolve_pending_next_locked(cpu);
        true
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
        self.cpu_state[cpu].last_schedule_ticks = crate::time::get_ticks();
        self.reclaim_unschedulable_cpu_queues();
        self.resolve_pending_next_locked(cpu);

        let current_is_idle =
            self.cpu_state[cpu].current_thread == Some(self.cpu_state[cpu].idle_thread);
        set_cpu_idle(cpu, current_is_idle);
        let current_thread = self.cpu_state[cpu].current_thread.unwrap_or(0);
        let previous_thread = self.cpu_state[cpu].previous_thread.unwrap_or(0);
        trace_sched_diag(
            TRACE_SCHED_DIAG_ENTER,
            current_thread,
            current_thread,
            0,
            ((previous_thread as u32) << 16)
                | ((current_is_idle as u32) << 1)
                | self.ready_queue_length() as u32,
        );

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
                self.unblock_for_io_from_isr_buffer(tid);
            }
        }

        // If current thread is still runnable, mark it as Ready but DON'T add to queue.
        //
        // Linux invariant: a task published as runnable must either already be
        // queued, still be current, or have an unloseable handoff marker.  The
        // AArch64 context-switch tail cannot enqueue the old thread until its
        // context is saved, so publish previous_thread before Ready for the
        // deferred case.  That keeps other CPUs from observing Ready without a
        // queue/deferred owner.
        let mut should_requeue_old = false;
        if let Some(current_id) = self.cpu_state[Self::current_cpu_id()].current_thread {
            if current_id != self.cpu_state[Self::current_cpu_id()].idle_thread {
                let (is_terminated, is_blocked, terminated_owner_pid) =
                    if let Some(current) = self.get_thread(current_id) {
                        let was_terminated = current.state == ThreadState::Terminated;
                        let was_blocked = current.state == ThreadState::Blocked
                            || current.state == ThreadState::BlockedOnSignal
                            || current.state == ThreadState::BlockedOnChildExit
                            || current.state == ThreadState::BlockedOnTimer
                            || current.state == ThreadState::BlockedOnIO;

                        (
                            was_terminated,
                            was_blocked,
                            was_terminated.then_some(current.owner_pid).flatten(),
                        )
                    } else {
                        (true, false, None)
                    };

                // This peer scheduling pass is declining to dispatch a thread
                // already quarantined by terminate_process_threads. Consume the
                // matching teardown kick without taking any additional lock.
                if let Some(owner_pid) = terminated_owner_pid {
                    observe_exit_kick(owner_pid);
                }

                let published_ready = !is_terminated && !is_blocked;
                let in_queue = self.per_cpu_queues.iter().any(|q| q.contains(&current_id));
                // Instead of adding to a queue, just record whether we SHOULD
                should_requeue_old = published_ready && !in_queue;
                if published_ready {
                    if should_requeue_old {
                        self.cpu_state[Self::current_cpu_id()].previous_thread = Some(current_id);
                    }
                    if let Some(current) = self.get_thread_mut(current_id) {
                        let now = crate::time::get_ticks();
                        current.cpu_ticks_total += now.wrapping_sub(current.run_start_ticks);
                        current.run_start_ticks = now;
                        current.set_ready();
                    }
                    WAKE_SITE_SCHEDULE.fetch_add(1, Ordering::Relaxed);
                    record_ready_site(current_id, READY_SITE_SCHEDULE);
                    if should_requeue_old {
                        ENQUEUE_DEFERRED.fetch_add(1, Ordering::Relaxed);
                    } else if in_queue {
                        ENQUEUE_ALREADY_QUEUED_OK.fetch_add(1, Ordering::Relaxed);
                    }
                }
                trace_sched_diag(
                    TRACE_SCHED_DIAG_CURRENT_READY,
                    current_id,
                    current_id,
                    0,
                    ((should_requeue_old as u32) << 31)
                        | ((in_queue as u32) << 30)
                        | self.ready_queue_length() as u32,
                );
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
            let local_candidates = self.per_cpu_queues[current_cpu].len();
            for _ in 0..local_candidates {
                let Some(n) = self.per_cpu_queues[current_cpu].pop_front() else {
                    break;
                };
                let (terminated, owner_pid) = self
                    .get_thread(n)
                    .map(|thread| (thread.state == ThreadState::Terminated, thread.owner_pid))
                    .unwrap_or((false, None));
                if terminated {
                    if owner_pid.is_some_and(|pid| !observe_exit_kick(pid)) {
                        self.per_cpu_queues[current_cpu].push_back(n);
                    }
                    continue;
                }
                break 'sched_outer n;
            }
            // Work-steal from other CPUs
            for steal_cpu in 0..MAX_CPUS {
                if steal_cpu == current_cpu {
                    continue;
                }
                let steal_candidates = self.per_cpu_queues[steal_cpu].len();
                for _ in 0..steal_candidates {
                    let Some(n) = self.per_cpu_queues[steal_cpu].pop_front() else {
                        break;
                    };
                    #[cfg(all(target_arch = "aarch64", feature = "boot_tests"))]
                    if retain_cpu_affine_test_thread(
                        &mut self.per_cpu_queues[steal_cpu],
                        n,
                        current_cpu,
                    ) {
                        continue;
                    }
                    #[cfg(all(target_arch = "aarch64", feature = "ec0_fault_inject"))]
                    if retain_ec0_fault_inject_on_cpu0(
                        &mut self.per_cpu_queues[steal_cpu],
                        n,
                        current_cpu,
                    ) {
                        break;
                    }
                    #[cfg(target_arch = "aarch64")]
                    if let Some(home) = self.percpu_stack_home_cpu(n, current_cpu) {
                        PERCPU_STACK_SELECTION_ROUTED.fetch_add(1, Ordering::Relaxed);
                        self.per_cpu_queues[home].push_back(n);
                        continue;
                    }
                    let (terminated, owner_pid) = self
                        .get_thread(n)
                        .map(|thread| (thread.state == ThreadState::Terminated, thread.owner_pid))
                        .unwrap_or((false, None));
                    if terminated {
                        if owner_pid.is_some_and(|pid| !observe_exit_kick(pid)) {
                            self.per_cpu_queues[steal_cpu].push_back(n);
                        }
                        continue;
                    }
                    break 'sched_outer n;
                }
            }
            break self.cpu_state[current_cpu].idle_thread;
        };

        trace_sched_diag(
            TRACE_SCHED_DIAG_PICK,
            next_thread_id,
            self.cpu_state[current_cpu].current_thread.unwrap_or(0),
            next_thread_id,
            self.ready_queue_length() as u32,
        );

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
                            #[cfg(all(target_arch = "aarch64", feature = "boot_tests"))]
                            if retain_cpu_affine_test_thread(
                                &mut self.per_cpu_queues[steal_cpu],
                                n,
                                current_cpu,
                            ) {
                                continue;
                            }
                            #[cfg(all(target_arch = "aarch64", feature = "ec0_fault_inject"))]
                            if retain_ec0_fault_inject_on_cpu0(
                                &mut self.per_cpu_queues[steal_cpu],
                                n,
                                current_cpu,
                            ) {
                                continue;
                            }
                            #[cfg(target_arch = "aarch64")]
                            if let Some(home) = self.percpu_stack_home_cpu(n, current_cpu) {
                                PERCPU_STACK_SELECTION_ROUTED.fetch_add(1, Ordering::Relaxed);
                                self.per_cpu_queues[home].push_back(n);
                                continue;
                            }
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
                    // Restore Running state and clear the deferred marker (both
                    // were set above before we knew this thread would continue).
                    if self.cpu_state[current_cpu].previous_thread == Some(next_thread_id) {
                        self.cpu_state[current_cpu].previous_thread = None;
                    }
                    if let Some(t) = self.get_thread_mut(next_thread_id) {
                        t.set_running();
                    }
                    trace_sched_diag(
                        TRACE_SCHED_DIAG_RETURN_NONE,
                        next_thread_id,
                        next_thread_id,
                        next_thread_id,
                        self.ready_queue_length() as u32,
                    );
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
                trace_sched_diag(
                    TRACE_SCHED_DIAG_RETURN_NONE,
                    next_thread_id,
                    next_thread_id,
                    next_thread_id,
                    self.ready_queue_length() as u32,
                );
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
        self.cpu_state[current_cpu].pending_next = Some(next_thread_id);

        // Update per-CPU idle flag (lock-free, used by timer handler)
        let is_switching_to_idle = next_thread_id == self.cpu_state[current_cpu].idle_thread;
        set_cpu_idle(current_cpu, is_switching_to_idle);

        trace_sched_diag(
            TRACE_SCHED_DIAG_RETURN,
            next_thread_id,
            old_thread_id,
            next_thread_id,
            ((should_requeue_old as u32) << 31)
                | ((is_switching_to_idle as u32) << 30)
                | self.ready_queue_length() as u32,
        );
        #[cfg(not(feature = "coreproof_component_c"))]
        crate::proof_point!(DeferredRequeueClaim);
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
        if self.cpu_state[cpu].pending_next == Some(new_thread_id) {
            self.cpu_state[cpu].pending_next = None;
        }
    }

    /// Add a thread to the ready queue after its context has been saved.
    ///
    /// This completes the deferred requeue from `schedule_deferred_requeue()`.
    /// Must be called only after the thread's context has been fully saved
    /// to prevent other CPUs from dispatching it with stale state.
    #[cfg(target_arch = "aarch64")]
    pub fn requeue_thread_after_save(&mut self, thread_id: u64) {
        self.requeue_thread_after_save_onto(thread_id, Self::current_cpu_id());
    }

    /// The CPU that owns the per-CPU stack slot a ready thread's saved kernel
    /// SP stands in, when that is not this CPU.
    ///
    /// SELECTION-SIDE CUSTODY. The dispatch refuses a foreign resume stack and
    /// routes the thread to the owning CPU, but routing does not pin: the ready
    /// queues work-steal, so the declining CPU can take the same thread straight
    /// back off the owner's queue and refuse it again — a tight loop rather than
    /// a redirect, which is what the round-4 review recorded. Declining at
    /// SELECTION is what makes the thread genuinely stack-pinned.
    ///
    /// The rule is deliberately narrower than the dispatch's: this only declines
    /// to STEAL: a thread that reaches a CPU's own queue by any other route
    /// still meets the dispatch adjudication, is recorded there and is routed
    /// home from there. Selection does not manufacture the foreign resume; it
    /// also does not hide one that arrives another way.
    ///
    /// `None` for every ordinary thread: a heap-backed kernel stack names no
    /// slot, and that is decided in two comparisons before anything else is
    /// read. Idle threads are exempt by identity — each legitimately stands on
    /// its own CPU's slot — and the check runs only in the rare case where an
    /// address did name a slot.
    #[cfg(target_arch = "aarch64")]
    fn percpu_stack_home_cpu(&self, thread_id: u64, current_cpu: usize) -> Option<usize> {
        let saved_sp = self.get_thread(thread_id)?.context.sp;
        let slot = crate::arch_impl::aarch64::constants::percpu_stack_slot_of(saved_sp)?;
        if slot == current_cpu {
            return None;
        }
        if (0..MAX_CPUS).any(|cpu| self.cpu_state[cpu].idle_thread == thread_id) {
            return None;
        }
        Some(slot)
    }

    /// Requeue a thread onto the CPU that owns the resources it must resume on,
    /// rather than onto the CPU that is declining it.
    ///
    /// The aarch64 dispatch path uses this when a thread's saved kernel SP
    /// stands in another CPU's per-CPU stack slot: that stack belongs to one
    /// CPU, so the thread is runnable on exactly one CPU, and putting it back on
    /// the declining CPU's own queue would hand it straight back (work-stealing
    /// would hand it to a third). Every admission check in
    /// `requeue_thread_after_save` still applies — only the destination queue
    /// differs.
    #[cfg(target_arch = "aarch64")]
    pub fn requeue_thread_on_cpu(&mut self, thread_id: u64, cpu: usize) {
        if cpu >= MAX_CPUS {
            return;
        }
        self.requeue_thread_after_save_onto(thread_id, cpu);
    }

    #[cfg(target_arch = "aarch64")]
    fn requeue_thread_after_save_onto(&mut self, thread_id: u64, target_cpu: usize) {
        // Don't requeue idle threads (they are never in the ready queue)
        if (0..MAX_CPUS).any(|cpu| self.cpu_state[cpu].idle_thread == thread_id) {
            return;
        }
        let cpu = Self::current_cpu_id();
        let same_cpu_marker_present =
            cpu < MAX_CPUS && self.cpu_state[cpu].previous_thread == Some(thread_id);
        if same_cpu_marker_present {
            self.cpu_state[cpu].previous_thread = None;
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
        // Safety checks: only requeue if the thread is runnable and not already queued.
        // Also handle the deferred-window race: if unblock_for_io() fired while the thread
        // was in the deferred slot (set Ready but couldn't enqueue), enqueue it now.
        let publish_running = if let Some(thread) = self.get_thread(thread_id) {
            match thread.state {
                ThreadState::Ready => false,
                ThreadState::Running => {
                    // A pending_next entry is a live ownership record: its CPU
                    // will commit this thread or roll it back at its next scheduler entry.
                    if self
                        .cpu_state
                        .iter()
                        .any(|state| state.pending_next == Some(thread_id))
                    {
                        return;
                    }
                    true
                }
                _ => return, // Thread state changed (terminated/blocked) - don't requeue
            }
        } else {
            return;
        };
        let in_any_queue = self.per_cpu_queues.iter().any(|q| q.contains(&thread_id));
        if !in_any_queue {
            if publish_running {
                if let Some(thread) = self.get_thread_mut(thread_id) {
                    thread.set_ready();
                }
            }
            self.per_cpu_queues[target_cpu].push_back(thread_id);
            ENQUEUE_DEFERRED_DRAINED_OK.fetch_add(1, Ordering::Relaxed);
            // Send IPI to wake an idle CPU to pick up the requeued thread
            self.send_resched_ipi();
        } else {
            ENQUEUE_ALREADY_QUEUED_OK.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Block the current thread.
    ///
    /// This is the generic member of the `block_current*` family: it is the only
    /// place in the kernel that publishes the plain `Blocked` state, so P9's
    /// no-new-block interlock has one site to install here rather than one per
    /// caller.
    ///
    /// The primitive owns the ready-queue departure: the publication and the
    /// departure happen in one scheduler-lock acquisition, so no window exists
    /// in which a `Blocked` thread is still dispatchable. That window mattered
    /// because the dispatch loop admits every candidate it pops except a
    /// `Terminated` one — a queued `Blocked` thread would be dispatched exactly
    /// like a `Ready` one, with no refusal anywhere on the path. Callers must
    /// not open-code the departure; the caller rule in
    /// `tests/teardown_structure.rs` fires when one does.
    ///
    /// `blocked_in_syscall` is deliberately not written here: it records which
    /// kind of context is saved in `thread.context`, not blocked-ness. A caller
    /// blocking inside a syscall uses `block_current_in_syscall()`, which
    /// publishes that fact in the same critical section instead of leaving it to
    /// a follow-up write at the call site.
    pub fn block_current(&mut self) {
        self.block_current_inner(false);
    }

    /// Block the current thread from inside a syscall.
    ///
    /// The syscall-context variant of `block_current()`: same publication and
    /// same departure, plus `blocked_in_syscall`, so the context-switch path
    /// saves and restores the kernel-side syscall context rather than stale
    /// userspace context. Every syscall-path caller of the generic primitive
    /// uses this variant, which is why none of them writes the flag itself.
    pub fn block_current_in_syscall(&mut self) {
        self.block_current_inner(true);
    }

    /// The generic block: charge, publish, depart — in that order, under the
    /// one scheduler-lock acquisition the caller already holds.
    fn block_current_inner(&mut self, in_syscall: bool) {
        #[cfg(not(feature = "coreproof_component_c"))]
        crate::proof_point!(BlockEntry);
        let Some(current_id) = self.cpu_state[Self::current_cpu_id()].current_thread else {
            return;
        };

        if let Some(current) = self.get_thread_mut(current_id) {
            // Charge elapsed CPU ticks before blocking
            let now = crate::time::get_ticks();
            current.cpu_ticks_total += now.wrapping_sub(current.run_start_ticks);
            current.run_start_ticks = now;

            current.state = ThreadState::Blocked;
            #[cfg(not(feature = "coreproof_component_c"))]
            crate::proof_point!(BlockAfterStateStore);
            if in_syscall {
                current.blocked_in_syscall = true;
            }
        }

        // The departure the family's other members already perform. The current
        // thread is not expected to name a ready queue at all, so this is the
        // same defence in depth they document — but it is now unconditional for
        // the generic block too, rather than a post-condition each caller had to
        // remember.
        #[cfg(not(feature = "coreproof_component_c"))]
        crate::proof_point!(BlockBeforeDeparture);
        crate::proof_cover!(BlockDeparture);
        // CORE-PROOF MUTATION LEG `coreproof_mut_block_departure` (#647, scoped
        // closed by PR #648): the departure is skipped, so a thread that has
        // published `Blocked` still names a ready queue and is dispatchable —
        // blocked-yet-dispatchable, which is the defect verbatim. Test profiles
        // only. Expected predicate: BLOCKED_NOT_IN_READYQ.
        #[cfg(not(feature = "coreproof_mut_block_departure"))]
        for q in self.per_cpu_queues.iter_mut() {
            q.retain(|&id| id != current_id);
        }
        #[cfg(not(feature = "coreproof_component_c"))]
        crate::proof_point!(BlockAfterDeparture);
    }

    /// Boot-test probe for the departure post-condition of the generic block.
    ///
    /// The departure is not observable on a healthy boot: under ordinary
    /// scheduling the current thread names no ready queue, so deleting the
    /// departure from the primitive changes nothing a running kernel would
    /// notice. This probe manufactures the one state that makes it observable —
    /// the current thread present in its own CPU's ready queue while it
    /// publishes `Blocked`, i.e. blocked-yet-dispatchable — and proves the
    /// primitive removed it, for both the plain and the syscall variant.
    ///
    /// Everything runs inside the single scheduler-lock acquisition the caller
    /// holds with interrupts masked, and the thread's pre-probe state and queue
    /// membership are restored before that lock is released, so no other CPU can
    /// observe the planted entry. The only residue is the CPU-tick charge the
    /// primitive performs, which is idempotent across back-to-back calls.
    #[cfg(feature = "boot_tests")]
    pub fn block_current_departure_gate(&mut self) -> Result<(), &'static str> {
        let cpu = Self::current_cpu_id();
        let Some(tid) = self.cpu_state[cpu].current_thread else {
            return Err("departure gate ran with no current thread");
        };
        let Some(thread) = self.get_thread(tid) else {
            return Err("departure gate found no row for the current thread");
        };
        let restore_state = thread.state;
        let restore_in_syscall = thread.blocked_in_syscall;
        if self.per_cpu_queues.iter().any(|q| q.contains(&tid)) {
            return Err("departure gate started with the current thread already queued");
        }

        // Leg 1: the plain primitive departs the planted entry, publishes
        // Blocked, and leaves the syscall-context fact alone.
        self.per_cpu_queues[cpu].push_back(tid);
        self.block_current();
        let mut outcome = if self.per_cpu_queues.iter().any(|q| q.contains(&tid)) {
            Err("block_current left the blocked thread dispatchable")
        } else if self.get_thread(tid).map(|thread| thread.state) != Some(ThreadState::Blocked) {
            Err("block_current did not publish Blocked")
        } else if self.get_thread(tid).map(|thread| thread.blocked_in_syscall)
            != Some(restore_in_syscall)
        {
            Err("block_current wrote blocked_in_syscall")
        } else {
            Ok(())
        };

        // Leg 2: the syscall variant departs the planted entry too, and adds the
        // syscall-context fact its callers no longer write themselves.
        if outcome.is_ok() {
            self.per_cpu_queues[cpu].push_back(tid);
            self.block_current_in_syscall();
            outcome = if self.per_cpu_queues.iter().any(|q| q.contains(&tid)) {
                Err("block_current_in_syscall left the blocked thread dispatchable")
            } else if self.get_thread(tid).map(|thread| thread.blocked_in_syscall) != Some(true) {
                Err("block_current_in_syscall did not publish the syscall context")
            } else {
                Ok(())
            };
        }

        // Restore exactly, before the lock is released.
        self.per_cpu_queues[cpu].retain(|&id| id != tid);
        if let Some(thread) = self.get_thread_mut(tid) {
            thread.state = restore_state;
            thread.blocked_in_syscall = restore_in_syscall;
        }
        outcome
    }

    /// Unblock a thread by ID.
    ///
    /// This generic wake path only transitions `Blocked`, `BlockedOnSignal`,
    /// `BlockedOnTimer`, and `BlockedOnIO`. Other blocked states have dedicated
    /// wake paths and are reported as `UnblockOutcome::NotFound` here.
    pub fn unblock(&mut self, thread_id: u64) -> UnblockOutcome {
        #[cfg(not(feature = "coreproof_component_c"))]
        crate::proof_point!(UnblockEntry);
        // Increment the call counter for testing (tracks that unblock was called)
        UNBLOCK_CALL_COUNT.fetch_add(1, Ordering::SeqCst);

        let mut outcome = UnblockOutcome::NotFound;
        if let Some(thread) = self.get_thread_mut(thread_id) {
            if thread.state == ThreadState::Blocked
                || thread.state == ThreadState::BlockedOnSignal
                || thread.state == ThreadState::BlockedOnTimer
                || thread.state == ThreadState::BlockedOnIO
            {
                thread.set_ready();
                #[cfg(not(feature = "coreproof_component_c"))]
                crate::proof_point!(UnblockAfterSetReady);
                outcome = UnblockOutcome::Transitioned;
                WAKE_SITE_UNBLOCK.fetch_add(1, Ordering::Relaxed);
                record_ready_site(thread_id, READY_SITE_UNBLOCK);
                // blocked_in_syscall does not record blocked-ness (ThreadState does); it
                // records which kind of context is saved in thread.context. Only code
                // running on the thread itself may clear it. This keeps unblock()
                // consistent with unblock_for_signal() and the BlockedOnIO arm of
                // wake_io_thread_locked(), which already refuse to clear it.

                // SMP safety: Don't add to ready_queue if thread is currently
                // running on any CPU. If a thread is blocked in a syscall's WFI
                // loop (e.g., sys_read waiting for keyboard input), it's still
                // the "current thread" on that CPU. Adding it to the ready_queue
                // would allow another CPU to schedule it simultaneously, causing
                // double-scheduling: two CPUs executing the same thread with the
                // same stack, leading to context corruption and crashes (ELR=0x0).
                // The CPU running the thread will detect the state change (Blocked
                // → Ready) when its WFI loop checks the thread state after waking.
                #[cfg(target_arch = "aarch64")]
                let current_cpu = (0..MAX_CPUS)
                    .find(|&cpu| self.cpu_state[cpu].current_thread == Some(thread_id));
                #[cfg(target_arch = "aarch64")]
                let is_current_on_any_cpu = current_cpu.is_some();
                #[cfg(not(target_arch = "aarch64"))]
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

                let already_queued = self.per_cpu_queues.iter().any(|q| q.contains(&thread_id));
                if !is_current_on_any_cpu
                    && !is_in_deferred
                    && thread_id != self.cpu_state[Self::current_cpu_id()].idle_thread
                    && !already_queued
                {
                    let target = self.find_target_cpu_for_wakeup(thread_id);
                    #[cfg(not(feature = "coreproof_component_c"))]
                    crate::proof_point!(UnblockBeforeEnqueue);
                    self.per_cpu_queues[target].push_back(thread_id);
                    #[cfg(not(feature = "coreproof_component_c"))]
                    crate::proof_point!(UnblockAfterEnqueue);
                    ENQUEUE_SAME_LOCK_OK.fetch_add(1, Ordering::Relaxed);
                    // CRITICAL: Only log on x86_64 to avoid deadlock on ARM64
                    #[cfg(target_arch = "x86_64")]
                    log_serial_println!(
                        "unblock({}): Added to per_cpu_queues[{}]",
                        thread_id,
                        target
                    );

                    // Send IPI to wake an idle CPU so it can pick up the unblocked thread
                    #[cfg(target_arch = "aarch64")]
                    self.send_resched_ipi();
                } else if is_current_on_any_cpu || is_in_deferred {
                    ENQUEUE_DEFERRED.fetch_add(1, Ordering::Relaxed);
                    #[cfg(target_arch = "aarch64")]
                    if let Some(target) = current_cpu {
                        self.trace_wake_current(thread_id, target);
                        self.send_resched_ipi_to_cpu(target);
                    }
                } else if already_queued {
                    ENQUEUE_ALREADY_QUEUED_OK.fetch_add(1, Ordering::Relaxed);
                }
            } else if thread.state == ThreadState::Ready || thread.state == ThreadState::Running {
                outcome = UnblockOutcome::AlreadyRunnable;
            }
        }

        outcome
    }

    #[cfg(target_arch = "aarch64")]
    fn trace_wake_current(&self, thread_id: u64, target_cpu: usize) {
        crate::tracing::record_event(
            crate::tracing::TraceEventType::SCHED_WAKE_CURRENT,
            0,
            (((target_cpu as u32) & 0xFFFF) << 16) | ((thread_id as u32) & 0xFFFF),
        );
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

    /// Send a reschedule IPI to the CPU that received a newly runnable task.
    #[cfg(target_arch = "aarch64")]
    fn send_resched_ipi_to_cpu(&self, target_cpu: usize) {
        use crate::arch_impl::aarch64::smp;

        if target_cpu == Self::current_cpu_id() {
            return;
        }
        if target_cpu >= MAX_CPUS || target_cpu >= smp::cpus_online() as usize {
            return;
        }

        crate::tracing::record_event(
            crate::tracing::TraceEventType::SCHED_RESCHED_IPI_SEND,
            0,
            (((target_cpu as u32) & 0xFFFF) << 16) | ((Self::current_cpu_id() as u32) & 0xFFFF),
        );

        crate::arch_impl::aarch64::gic::send_sgi(
            crate::arch_impl::aarch64::constants::SGI_RESCHEDULE as u8,
            target_cpu as u8,
        );
    }

    /// Publish teardown-attributed evidence, then broadcast a reschedule SGI
    /// to every other online CPU. This is intentionally separate from the two
    /// generic wakeup helpers and must be called with no lock held.
    pub fn send_exit_expedite_sgi(victim_pid: u64, batch: GroupBatchId) {
        #[cfg(target_arch = "aarch64")]
        {
            use crate::arch_impl::aarch64::{constants::SGI_RESCHEDULE, gic, smp};
            use crate::tracing::providers::teardown::{
                KickPublishResult, EXIT_KICK_BUCKETS, EXIT_KICK_PUBLISHED, EXIT_KICK_SLOTS,
            };

            let bucket = victim_pid as usize % EXIT_KICK_BUCKETS;
            let slot = &EXIT_KICK_SLOTS[bucket];
            let at = crate::tracing::trace_timestamp();
            match slot.publish(victim_pid, at) {
                KickPublishResult::Published { displaced, .. } => {
                    if displaced {
                        crate::tracing::providers::teardown::record_exit_kick_collision(bucket);
                    }
                    crate::trace_count!(EXIT_KICK_PUBLISHED);
                    crate::tracing::providers::teardown::record_exit_kick_published(bucket);
                }
                KickPublishResult::ReservationLost => {
                    crate::tracing::providers::teardown::record_exit_kick_collision(bucket);
                }
            }

            crate::trace_count!(crate::tracing::providers::teardown::EXIT_SGI_SENT);
            crate::tracing::providers::teardown::record_exit_sgi_sent(victim_pid, batch.as_u64());

            let current_cpu = Self::current_cpu_id();
            let online = smp::cpus_online() as usize;
            for cpu in 0..online.min(MAX_CPUS) {
                if cpu != current_cpu {
                    gic::send_sgi(SGI_RESCHEDULE as u8, cpu as u8);
                }
            }
        }

        #[cfg(not(target_arch = "aarch64"))]
        let _ = (victim_pid, batch);
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
                WAKE_SITE_SIGNAL.fetch_add(1, Ordering::Relaxed);
                record_ready_site(thread_id, READY_SITE_SIGNAL);
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

                let already_queued = self.per_cpu_queues.iter().any(|q| q.contains(&thread_id));
                if !is_current_on_any_cpu
                    && !is_in_deferred
                    && thread_id != self.cpu_state[Self::current_cpu_id()].idle_thread
                    && !already_queued
                {
                    let target = self.find_target_cpu_for_wakeup(thread_id);
                    self.per_cpu_queues[target].push_back(thread_id);
                    ENQUEUE_SAME_LOCK_OK.fetch_add(1, Ordering::Relaxed);
                    #[cfg(target_arch = "x86_64")]
                    log_serial_println!(
                        "unblock_for_signal: Thread {} unblocked, added to per_cpu_queues[{}]",
                        thread_id,
                        target
                    );

                    // Send IPI to wake an idle CPU
                    #[cfg(target_arch = "aarch64")]
                    self.send_resched_ipi();
                } else if is_current_on_any_cpu || is_in_deferred {
                    ENQUEUE_DEFERRED.fetch_add(1, Ordering::Relaxed);
                } else if already_queued {
                    ENQUEUE_ALREADY_QUEUED_OK.fetch_add(1, Ordering::Relaxed);
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
                WAKE_SITE_CHILD_EXIT.fetch_add(1, Ordering::Relaxed);
                record_ready_site(thread_id, READY_SITE_CHILD_EXIT);

                // SMP safety: Don't add to ready_queue if thread is current on any CPU
                // (same rationale as unblock() - prevents double-scheduling)
                let is_current_on_any_cpu =
                    (0..MAX_CPUS).any(|cpu| self.cpu_state[cpu].current_thread == Some(thread_id));

                #[cfg(target_arch = "aarch64")]
                let is_in_deferred = self.is_in_deferred_requeue(thread_id);
                #[cfg(not(target_arch = "aarch64"))]
                let is_in_deferred = false;

                let already_queued = self.per_cpu_queues.iter().any(|q| q.contains(&thread_id));
                if !is_current_on_any_cpu
                    && !is_in_deferred
                    && thread_id != self.cpu_state[Self::current_cpu_id()].idle_thread
                    && !already_queued
                {
                    let target = self.find_target_cpu_for_wakeup(thread_id);
                    self.per_cpu_queues[target].push_back(thread_id);
                    ENQUEUE_SAME_LOCK_OK.fetch_add(1, Ordering::Relaxed);
                    // CRITICAL: Only log on x86_64 to avoid deadlock on ARM64
                    #[cfg(target_arch = "x86_64")]
                    log_serial_println!(
                        "Thread {} unblocked by child exit, queued to cpu {}",
                        thread_id,
                        target
                    );

                    // Send IPI to wake an idle CPU
                    #[cfg(target_arch = "aarch64")]
                    self.send_resched_ipi();
                } else if is_current_on_any_cpu || is_in_deferred {
                    ENQUEUE_DEFERRED.fetch_add(1, Ordering::Relaxed);
                } else if already_queued {
                    ENQUEUE_ALREADY_QUEUED_OK.fetch_add(1, Ordering::Relaxed);
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
            #[cfg(target_arch = "aarch64")]
            set_cpu_idle(Self::current_cpu_id(), true);
            // Insert into timer heap for O(1) expiry detection
            self.timer_heap.push(Reverse((wake_time_ns, current_id)));
            for q in self.per_cpu_queues.iter_mut() {
                q.retain(|&id| id != current_id);
            }
        }
    }

    fn block_current_for_io_publish(&mut self, wake_time_ns: Option<u64>) -> Option<u64> {
        let current_id = self.cpu_state[Self::current_cpu_id()].current_thread?;
        let thread = self.get_thread_mut(current_id)?;

        // Charge elapsed CPU ticks before blocking
        let now = crate::time::get_ticks();
        thread.cpu_ticks_total += now.wrapping_sub(thread.run_start_ticks);
        thread.run_start_ticks = now;

        thread.state = ThreadState::BlockedOnIO;
        thread.wake_time_ns = wake_time_ns;
        // The observation belongs to this wait, not to whatever the thread did
        // before it.
        thread.timer_pop_wake_time_set = None;
        // Mark blocked_in_syscall so the context switch path resumes
        // inside the syscall (wait_timeout loop) rather than restoring
        // stale userspace context.
        thread.blocked_in_syscall = true;

        // The departure belongs to the publication, not to the wrapper: this is
        // the only function in the family that published a blocked state
        // without departing the thread itself, which left the post-condition
        // resting on whichever caller remembered it.
        for q in self.per_cpu_queues.iter_mut() {
            q.retain(|&id| id != current_id);
        }

        // Linux set_current_state() is smp_store_mb(): publish the sleep
        // state before later condition checks or schedule entry observe
        // wakeups. On AArch64 Rust lowers SeqCst fence to a full DMB.
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        Some(current_id)
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
        let _ = self.block_current_for_io_with_timeout(None);
    }

    /// Block the current thread for device I/O, optionally with a timeout.
    ///
    /// A timed BlockedOnIO wait is used by completions: the ISR wakes it via
    /// unblock_for_io(), while the timer path wakes it by observing
    /// wake_time_ns without clearing blocked_in_syscall prematurely.
    pub fn block_current_for_io_with_timeout(&mut self, wake_time_ns: Option<u64>) -> bool {
        if let Some(current_id) = self.block_current_for_io_publish(wake_time_ns) {
            // Insert into timer heap if a timeout was specified
            if let Some(wt) = wake_time_ns {
                self.timer_heap.push(Reverse((wt, current_id)));
            }
            true
        } else {
            false
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
        self.unblock_for_io_attributed(tid, false);
    }

    fn unblock_for_io_from_isr_buffer(&mut self, tid: u64) {
        self.unblock_for_io_attributed(tid, true);
    }

    fn unblock_for_io_attributed(&mut self, tid: u64, from_isr_buffer: bool) {
        let wake = self.wake_io_thread_locked(tid, from_isr_buffer);
        #[cfg(target_arch = "aarch64")]
        if let Some(target) = wake.current_cpu {
            self.trace_wake_current(tid, target);
        }
        #[cfg(target_arch = "aarch64")]
        if let Some(target) = wake.resched_target() {
            self.send_resched_ipi_to_cpu(target);
        }
        #[cfg(not(target_arch = "aarch64"))]
        let _ = wake.resched_target();
    }

    /// Immediate task-context waitqueue wake.
    ///
    /// Mirrors Linux's waitqueue -> try_to_wake_up nesting: the waitqueue lock
    /// is held by the caller, and this helper performs the scheduler state
    /// transition before the waitqueue wake returns.
    pub fn wake_waitqueue_thread(&mut self, tid: u64) {
        let wake = self.wake_io_thread_locked(tid, false);
        #[cfg(target_arch = "aarch64")]
        if let Some(target) = wake.current_cpu {
            self.trace_wake_current(tid, target);
        }
        #[cfg(target_arch = "aarch64")]
        if let Some(target) = wake.resched_target() {
            self.send_resched_ipi_to_cpu(target);
        }
        #[cfg(not(target_arch = "aarch64"))]
        let _ = wake.resched_target();
    }

    fn wake_io_thread_locked(&mut self, tid: u64, from_isr_buffer: bool) -> IoWakeResult {
        let mut wake = IoWakeResult::default();
        if let Some(thread) = self.get_thread_mut(tid) {
            let mut published_ready = false;
            let should_queue = match thread.state {
                ThreadState::BlockedOnIO => {
                    thread.set_ready();
                    WAKE_SITE_WAKE_IO_LOCKED.fetch_add(1, Ordering::Relaxed);
                    record_ready_site(
                        tid,
                        if from_isr_buffer {
                            READY_SITE_WAKE_IO_ISR_DRAIN
                        } else {
                            READY_SITE_WAKE_IO_LOCKED
                        },
                    );
                    published_ready = true;
                    thread.wake_time_ns = None;
                    // Do NOT clear blocked_in_syscall here — the wait_timeout
                    // caller manages it after detecting the wakeup.
                    true
                }
                ThreadState::Blocked => {
                    thread.set_ready();
                    WAKE_SITE_WAKE_IO_LOCKED.fetch_add(1, Ordering::Relaxed);
                    record_ready_site(
                        tid,
                        if from_isr_buffer {
                            READY_SITE_WAKE_IO_ISR_DRAIN
                        } else {
                            READY_SITE_WAKE_IO_LOCKED
                        },
                    );
                    published_ready = true;
                    thread.wake_time_ns = None;
                    // Legacy TCP socket waits still use plain Blocked and
                    // expect the generic unblock semantics.
                    thread.blocked_in_syscall = false;
                    true
                }
                _ => {
                    // If the completion wakeup was deferred through the lock-free
                    // ISR buffer, the thread may already have been marked Ready by
                    // another path before the buffer is drained. As long as it is
                    // still blocked in the syscall, it still needs a ready-queue
                    // insertion to resume and observe `done=token`.
                    thread.state == ThreadState::Ready && thread.blocked_in_syscall
                }
            };

            if should_queue {
                // Do not enqueue a thread that is still current on some CPU.
                // For BlockedOnIO waiters this means the wakeup won the race
                // against the old CPU's context save; that CPU will publish
                // the saved context and requeue the Ready thread after the
                // save point completes.
                wake.current_cpu =
                    (0..MAX_CPUS).find(|&cpu| self.cpu_state[cpu].current_thread == Some(tid));

                #[cfg(target_arch = "aarch64")]
                let is_in_deferred = self.is_in_deferred_requeue(tid);
                #[cfg(not(target_arch = "aarch64"))]
                let is_in_deferred = false;

                let already_queued = self.per_cpu_queues.iter().any(|q| q.contains(&tid));
                if wake.current_cpu.is_none()
                    && !is_in_deferred
                    && tid != self.cpu_state[Self::current_cpu_id()].idle_thread
                    && !already_queued
                {
                    let target = self.find_target_cpu_for_wakeup(tid);
                    self.per_cpu_queues[target].push_back(tid);
                    wake.enqueued_target = Some(target);
                    if from_isr_buffer {
                        ENQUEUE_ISR_BUFFER_DRAINED_OK.fetch_add(1, Ordering::Relaxed);
                    } else {
                        ENQUEUE_SAME_LOCK_OK.fetch_add(1, Ordering::Relaxed);
                    }
                } else if published_ready && (wake.current_cpu.is_some() || is_in_deferred) {
                    ENQUEUE_DEFERRED.fetch_add(1, Ordering::Relaxed);
                } else if already_queued {
                    ENQUEUE_ALREADY_QUEUED_OK.fetch_add(1, Ordering::Relaxed);
                }
                set_need_resched();
            }
        }
        wake
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

            // Record what this pop saw before deciding anything with it. A
            // popped entry whose thread has already had wake_time_ns cleared
            // makes the pop a no-op, and a timed wait that was relying on it
            // stays blocked with its deadline in the past. That is the fact
            // the futex timed-wait record needs (#608 F4); the store is a
            // single Option<bool> and costs nothing on the reschedule path.
            if let Some(thread) = self.get_thread_mut(tid) {
                thread.timer_pop_wake_time_set = Some(thread.wake_time_ns.is_some());
            }

            // Validate: thread might have been woken already (by ISR, signal, etc.)
            // or terminated. Only process if still in a timed-wait state with a
            // wake_time set.
            let is_timed_wait = if let Some(thread) = self.get_thread(tid) {
                (matches!(thread.state, ThreadState::BlockedOnTimer)
                    || (thread.state == ThreadState::BlockedOnIO && thread.wake_time_ns.is_some()))
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
                    WAKE_SITE_TIMER.fetch_add(1, Ordering::Relaxed);
                    record_ready_site(tid, READY_SITE_TIMER);
                    ENQUEUE_DEFERRED.fetch_add(1, Ordering::Relaxed);
                    thread.wake_time_ns = None;
                    if !was_blocked_on_io {
                        thread.blocked_in_syscall = false;
                    }
                    trace_sched_diag(
                        TRACE_SCHED_DIAG_WAKE_TIMER_CURRENT,
                        tid,
                        tid,
                        0,
                        ((was_blocked_on_io as u32) << 31) | self.ready_queue_length() as u32,
                    );
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
                WAKE_SITE_TIMER.fetch_add(1, Ordering::Relaxed);
                record_ready_site(tid, READY_SITE_TIMER);
                thread.wake_time_ns = None;

                // Timer-driven I/O wakeups resume back into wait_timeout() so
                // blocked_in_syscall must stay set until the waiter consumes
                // the wake reason. Ordinary BlockedOnTimer sleeps clear it here.
                if !was_blocked_on_io {
                    thread.blocked_in_syscall = false;
                }

                if !in_deferred_requeue && !is_idle && !already_queued {
                    let target = self.find_target_cpu_for_wakeup(tid);
                    trace_sched_diag(
                        TRACE_SCHED_DIAG_WAKE_TIMER_ENQUEUE,
                        tid,
                        tid,
                        target as u64,
                        ((was_blocked_on_io as u32) << 31)
                            | ((target as u32 & 0xF) << 16)
                            | self.ready_queue_length() as u32,
                    );
                    self.per_cpu_queues[target].push_back(tid);
                    ENQUEUE_SAME_LOCK_OK.fetch_add(1, Ordering::Relaxed);
                } else if in_deferred_requeue {
                    trace_sched_diag(
                        TRACE_SCHED_DIAG_WAKE_TIMER_DEFERRED,
                        tid,
                        tid,
                        0,
                        ((was_blocked_on_io as u32) << 31) | self.ready_queue_length() as u32,
                    );
                    ENQUEUE_DEFERRED.fetch_add(1, Ordering::Relaxed);
                } else if already_queued {
                    trace_sched_diag(
                        TRACE_SCHED_DIAG_WAKE_TIMER_ALREADY_QUEUED,
                        tid,
                        tid,
                        0,
                        ((was_blocked_on_io as u32) << 31) | self.ready_queue_length() as u32,
                    );
                    ENQUEUE_ALREADY_QUEUED_OK.fetch_add(1, Ordering::Relaxed);
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
        let cpu = Self::current_cpu_id();
        #[cfg(target_arch = "aarch64")]
        {
            let old_val = self.cpu_state[cpu].current_thread.unwrap_or(0xDEAD);
            record_cpu_state_change(cpu, 17, old_val, 0xDEAD);
        }
        self.cpu_state[cpu].current_thread = None;
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

    /// Check for a live userspace thread other than the one completing exit.
    #[cfg(target_arch = "aarch64")]
    pub fn has_userspace_threads_other_than(&self, exiting_thread_id: u64) -> bool {
        self.threads.iter().any(|thread| {
            thread.id() != exiting_thread_id
                && !self
                    .cpu_state
                    .iter()
                    .any(|state| state.idle_thread == thread.id())
                && thread.privilege == super::thread::ThreadPrivilege::User
                && thread.state != ThreadState::Terminated
        })
    }

    /// Make every scheduler-owned thread for a process non-runnable.
    pub fn terminate_process_threads(&mut self, owner_pid: u64) {
        crate::tracing::providers::teardown::record_quarantine(owner_pid);
        if crate::process::process_manager_held_on_current_cpu() {
            crate::trace_count!(crate::tracing::providers::teardown::TEARDOWN_LOCK_ORDER_SUSPECT);
        }
        // Preserve one scheduler-visible quarantine token per victim thread. A
        // peer can take SCHEDULER in the intentional gap before EXIT_KICK is
        // published; a pre-publication decline requeues this token, and the SGI
        // pass after publication consumes it. No CPU-residency predicate is
        // needed, and unobserved collision cases age out with thread retirement.
        let online_cpus = self.online_cpu_count();
        for index in 0..self.threads.len() {
            let thread_id = {
                let thread = &mut self.threads[index];
                if thread.owner_pid != Some(owner_pid) {
                    continue;
                }
                thread.set_terminated();
                thread.id()
            };
            if !self
                .per_cpu_queues
                .iter()
                .any(|queue| queue.contains(&thread_id))
            {
                // A quarantine pass may not allocate while SCHEDULER is held.
                // A running thread was previously popped from a queue, so the
                // normal path has retained capacity for this token. If every
                // queue is genuinely full, the outgoing-current observation
                // hook remains the evidence path for a running victim.
                if let Some(target) = (0..online_cpus)
                    .filter(|&cpu| {
                        self.per_cpu_queues[cpu].len() < self.per_cpu_queues[cpu].capacity()
                    })
                    .min_by_key(|&cpu| self.per_cpu_queues[cpu].len())
                {
                    self.per_cpu_queues[target].push_back(thread_id);
                }
            }
        }
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
        (0..self.online_cpu_count())
            .filter(|&cpu| self.cpu_accepts_wakeups(cpu))
            .min_by_key(|&cpu| self.per_cpu_queues[cpu].len())
            .unwrap_or(current_cpu)
    }

    /// Find the CPU with the fewest threads in its queue.
    /// Used when spawning new threads.
    fn least_loaded_cpu(&self) -> usize {
        let current_cpu = Self::current_cpu_id();
        (0..self.online_cpu_count())
            .filter(|&cpu| self.cpu_accepts_wakeups(cpu))
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

    /// Emit where a thread sits in the scheduler for placement diagnostics.
    ///
    /// The scheduler state is snapshotted under one lock acquisition, then
    /// emitted after the lock is released so serial output cannot invert the
    /// scheduler/serial lock ordering. This is diagnostic-only and intended
    /// for thread context.
    fn dump_thread_placement(tid: u64, label: &str) {
        const QUEUE_PREVIEW: usize = 4;

        struct PlacementSnapshot {
            state: Option<ThreadState>,
            current_cpus: [bool; MAX_CPUS],
            queue_cpus: [bool; MAX_CPUS],
            queue_indices: [usize; MAX_CPUS],
            deferred_requeue: bool,
            current_threads: [u64; MAX_CPUS],
            idle_threads: [u64; MAX_CPUS],
            ready_lengths: [usize; MAX_CPUS],
            ready_first: [[u64; QUEUE_PREVIEW]; MAX_CPUS],
            ready_first_lengths: [usize; MAX_CPUS],
            need_resched: [bool; MAX_CPUS],
        }

        let snapshot = with_scheduler(|scheduler| {
            let mut snapshot = PlacementSnapshot {
                state: scheduler.get_thread(tid).map(|thread| thread.state),
                current_cpus: [false; MAX_CPUS],
                queue_cpus: [false; MAX_CPUS],
                queue_indices: [usize::MAX; MAX_CPUS],
                deferred_requeue: false,
                current_threads: [0; MAX_CPUS],
                idle_threads: [0; MAX_CPUS],
                ready_lengths: [0; MAX_CPUS],
                ready_first: [[0; QUEUE_PREVIEW]; MAX_CPUS],
                ready_first_lengths: [0; MAX_CPUS],
                need_resched: [false; MAX_CPUS],
            };

            #[cfg(target_arch = "aarch64")]
            {
                snapshot.deferred_requeue = scheduler.is_in_deferred_requeue(tid);
            }

            for cpu in 0..MAX_CPUS {
                let cpu_state = &scheduler.cpu_state[cpu];
                snapshot.current_threads[cpu] = cpu_state.current_thread.unwrap_or(0);
                snapshot.idle_threads[cpu] = cpu_state.idle_thread;
                snapshot.current_cpus[cpu] = cpu_state.current_thread == Some(tid);
                snapshot.ready_lengths[cpu] = scheduler.per_cpu_queues[cpu].len();
                snapshot.need_resched[cpu] = {
                    #[cfg(target_arch = "aarch64")]
                    {
                        crate::per_cpu_aarch64::need_resched_for_cpu(cpu)
                    }
                    #[cfg(not(target_arch = "aarch64"))]
                    {
                        cpu == 0 && crate::per_cpu::need_resched()
                    }
                };

                for (index, queued_tid) in scheduler.per_cpu_queues[cpu]
                    .iter()
                    .take(QUEUE_PREVIEW)
                    .enumerate()
                {
                    snapshot.ready_first[cpu][index] = *queued_tid;
                    snapshot.ready_first_lengths[cpu] += 1;
                }

                if let Some(index) = scheduler.per_cpu_queues[cpu]
                    .iter()
                    .position(|&queued_tid| queued_tid == tid)
                {
                    snapshot.queue_cpus[cpu] = true;
                    snapshot.queue_indices[cpu] = index;
                }
            }

            snapshot
        });

        let Some(snapshot) = snapshot else {
            crate::serial_println!(
                "[sched-placement] label={} tid={} scheduler=unavailable",
                label,
                tid
            );
            return;
        };

        crate::serial_println!(
            "[sched-placement] label={} tid={} state={:?} current_cpus={:?} queue_cpus={:?} queue_indices={:?} deferred_requeue={}",
            label,
            tid,
            snapshot.state,
            snapshot.current_cpus,
            snapshot.queue_cpus,
            snapshot.queue_indices,
            snapshot.deferred_requeue,
        );
        for cpu in 0..MAX_CPUS {
            crate::serial_println!(
                "[sched-placement] label={} cpu={} current={} idle={} ready_len={} ready_first={:?} need_resched={}",
                label,
                cpu,
                snapshot.current_threads[cpu],
                snapshot.idle_threads[cpu],
                snapshot.ready_lengths[cpu],
                &snapshot.ready_first[cpu][..snapshot.ready_first_lengths[cpu]],
                snapshot.need_resched[cpu],
            );
        }
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
        #[cfg(target_arch = "aarch64")]
        {
            let cpu = Self::current_cpu_id();
            let old_val = self.cpu_state[cpu].current_thread.unwrap_or(0xDEAD);
            record_cpu_state_change(cpu, 6, old_val, thread_id);
        }
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

    /// Return whether any scheduler-owned thread still caches a matching
    /// userspace translation-table root. The caller supplies the root matcher
    /// so the scheduler lock is acquired exactly once for an entire receipt.
    #[cfg(target_arch = "aarch64")]
    pub(crate) fn any_cached_ttbr0_matches<F>(&self, mut root_matches: F) -> bool
    where
        F: FnMut(u64) -> bool,
    {
        self.threads.iter().any(|thread| {
            thread.state != ThreadState::Terminated
                && thread.cached_ttbr0 != 0
                && root_matches(thread.cached_ttbr0)
        })
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
            record_cpu_state_change(cpu, 8, idle, real_tid);
            self.cpu_state[cpu].current_thread = Some(real_tid);
        }
    }

    /// Fix stale cpu_state where it names a non-idle thread as current while
    /// idle is actually the thread executing (i.e. `current_thread_ptr` is
    /// NULL, the per-CPU fast-path marker for "idle is running").
    ///
    /// This is the inverse of `fix_stale_idle_cpu_state` above: that function
    /// corrects "cpu_state says idle, but a real thread is running"; this one
    /// corrects "cpu_state says a real (non-idle) thread, but idle is what's
    /// actually running" -- e.g. after an idle dispatch that branched directly
    /// into `idle_loop_arm64` without updating `cpu_state.current_thread`,
    /// followed by a timer IRQ re-entering `schedule_from_kernel` while idle
    /// is executing but `cpu_state[cpu].current_thread` still names whatever
    /// thread was current before that redirect.
    ///
    /// Without this correction, the stale non-idle `current_thread` would be
    /// read straight through as `old_id` (the save target) by the scheduling
    /// decision that follows, and idle's live register file would be written
    /// into that unrelated (still-alive) thread's context -- the confirmed
    /// "idle register file leaking into a non-idle thread's dispatch frame"
    /// mechanism behind the ERET_ANOMALY / EC=0x0 crash family (see
    /// docs/planning/aarch64-launcher-spawn-crash/ROOT_CAUSE.md, candidate #1:
    /// "cpu_state / `old_id` save-target skew"). Callers must invoke this
    /// BEFORE the scheduling decision (`schedule_deferred_requeue`) runs.
    #[cfg(target_arch = "aarch64")]
    pub fn fix_stale_current_thread_when_idle_executing(&mut self) {
        let cpu = Self::current_cpu_id();
        let idle = self.cpu_state[cpu].idle_thread;
        if let Some(current) = self.cpu_state[cpu].current_thread {
            if current != idle {
                record_cpu_state_change(cpu, 7, current, idle);
                self.cpu_state[cpu].current_thread = Some(idle);
            }
        }
    }

    #[cfg(target_arch = "aarch64")]
    fn resolve_exception_cleanup_previous_thread(&mut self, cpu: usize) {
        if cpu >= MAX_CPUS {
            return;
        }
        let Some(previous) = self.cpu_state[cpu].previous_thread else {
            return;
        };

        let is_idle = (0..MAX_CPUS).any(|c| self.cpu_state[c].idle_thread == previous);
        let is_ready = self
            .get_thread(previous)
            .map(|thread| thread.state == ThreadState::Ready)
            .unwrap_or(false);
        let is_queued = self.per_cpu_queues.iter().any(|q| q.contains(&previous));
        let is_current = (0..MAX_CPUS).any(|c| self.cpu_state[c].current_thread == Some(previous));
        let is_other_deferred =
            (0..MAX_CPUS).any(|c| c != cpu && self.cpu_state[c].previous_thread == Some(previous));

        if is_ready && !is_idle && !is_queued && !is_current && !is_other_deferred {
            self.per_cpu_queues[cpu].push_back(previous);
            ENQUEUE_DEFERRED_DRAINED_OK.fetch_add(1, Ordering::Relaxed);
            set_need_resched();
        }

        self.cpu_state[cpu].previous_thread = None;
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
            record_cpu_state_change(cpu, 9, current, idle);
            self.cpu_state[cpu].current_thread = Some(idle);
        }
        self.resolve_exception_cleanup_previous_thread(cpu);
        set_cpu_idle(cpu, true);
    }
}

/// Initialize the global scheduler
#[allow(dead_code)]
pub fn init(idle_thread: Box<Thread>) {
    let mut scheduler_lock = lock_scheduler();
    *scheduler_lock = Some(Scheduler::new(idle_thread));
    // CRITICAL: Only log on x86_64 to avoid deadlock on ARM64
    #[cfg(target_arch = "x86_64")]
    log_serial_println!("Scheduler initialized");
}

/// Initialize scheduler with the current thread as the idle task (Linux-style)
/// This is used during boot where the boot thread becomes the idle task
pub fn init_with_current(current_thread: Box<Thread>) {
    let mut scheduler_lock = lock_scheduler();
    let thread_id = current_thread.id();

    // Create scheduler with current thread as both idle and current
    let mut scheduler = Scheduler::new(current_thread);
    #[cfg(all(target_arch = "aarch64", feature = "ec0_fault_inject"))]
    install_ec0_fault_inject_thread(&mut scheduler);
    #[cfg(target_arch = "aarch64")]
    {
        let old_val = scheduler.cpu_state[0].current_thread.unwrap_or(0xDEAD);
        record_cpu_state_change(0, 5, old_val, thread_id);
    }
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
        let mut scheduler_lock = lock_scheduler();
        if let Some(scheduler) = scheduler_lock.as_mut() {
            scheduler.register_idle_thread(cpu_id, idle_thread);
        }
    });
}

/// Add a thread to the scheduler
pub fn spawn(thread: Box<Thread>) {
    note_scheduler_publication();
    // Disable interrupts to prevent timer interrupt deadlock
    without_interrupts(|| {
        let mut scheduler_lock = lock_scheduler();
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

/// Test-only deterministic placement for concurrent protocol gates.
#[cfg(all(target_arch = "aarch64", feature = "boot_tests"))]
pub(crate) fn spawn_on_cpu_for_test(thread: Box<Thread>, cpu: usize) {
    without_interrupts(|| {
        let thread_id = thread.id();
        let mut scheduler_lock = lock_scheduler();
        let scheduler = scheduler_lock
            .as_mut()
            .expect("scheduler not initialized for test placement");
        BOOT_TEST_CPU_AFFINITY[cpu].store(thread_id, Ordering::Release);
        scheduler.add_thread_on_cpu_for_test(thread, cpu);
        NEED_RESCHED.store(true, Ordering::Relaxed);
        crate::per_cpu_aarch64::set_need_resched(true);
        scheduler.send_resched_ipi_to_cpu(cpu);
    });
}

/// Release a forced-placement probe and make it runnable on this CPU.
#[cfg(all(target_arch = "aarch64", feature = "boot_tests"))]
pub fn release_cpu_affine_thread_for_test(thread_id: u64) -> bool {
    without_interrupts(|| {
        let mut scheduler_lock = lock_scheduler();
        let scheduler = scheduler_lock
            .as_mut()
            .expect("scheduler not initialized for test release");
        clear_cpu_affinity_for_test(thread_id);
        if !scheduler
            .get_thread(thread_id)
            .is_some_and(|thread| thread.state == ThreadState::Ready)
        {
            return false;
        }

        for queue in scheduler.per_cpu_queues.iter_mut() {
            queue.retain(|queued_tid| *queued_tid != thread_id);
        }
        let current_cpu = Scheduler::current_cpu_id();
        scheduler.per_cpu_queues[current_cpu].push_back(thread_id);
        set_need_resched();
        true
    })
}

/// Test-only placement oracle: publish through the PRODUCTION placement path,
/// then pin the thread to whatever CPU that path chose.
///
/// This is the arm-A stimulus for #609. It deliberately does NOT choose a CPU:
/// `add_thread` runs `least_loaded_cpu()` exactly as production does, and only
/// afterwards is the resulting queue recorded in the existing boot-test affinity
/// table, so peers stop rescuing the thread off it. The pin is symmetric — it
/// names whichever CPU placement picked, never CPU 0 specifically — so the arm
/// asks one question and nothing else: can the CPU that placement chose
/// actually dispatch this thread? Weakening or removing the pin would make the
/// arm measure the rescue instead of the placement, so the pin never moves.
///
/// Returns the CPU the production placement path selected.
#[cfg(all(target_arch = "aarch64", feature = "arm_a_609"))]
pub(crate) fn spawn_pinned_where_placed_for_test(thread: Box<Thread>) -> usize {
    note_scheduler_publication();
    without_interrupts(|| {
        let thread_id = thread.id();
        let mut scheduler_lock = lock_scheduler();
        let scheduler = scheduler_lock
            .as_mut()
            .expect("scheduler not initialized for the placement oracle");
        scheduler.add_thread(thread);
        let placed_cpu = scheduler
            .per_cpu_queues
            .iter()
            .position(|queue| queue.contains(&thread_id))
            .unwrap_or_else(Scheduler::current_cpu_id);
        BOOT_TEST_CPU_AFFINITY[placed_cpu].store(thread_id, Ordering::Release);
        NEED_RESCHED.store(true, Ordering::Relaxed);
        crate::per_cpu_aarch64::set_need_resched(true);
        scheduler.send_resched_ipi();
        placed_cpu
    })
}

/// Add a thread to the front of the ready queue.
/// Used for fork children so they run before other queued threads.
pub fn spawn_front(thread: Box<Thread>) {
    note_scheduler_publication();
    without_interrupts(|| {
        let mut scheduler_lock = lock_scheduler();
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

pub fn reclaim_terminated_threads() {
    // Two masked regions, deliberately, with the scheduler lock held in neither
    // of the frees: the harvest under the lock, then the release. Splitting them
    // keeps the second window covering the free and nothing else.
    let reclaimed_threads = without_interrupts(|| {
        let mut scheduler_lock = lock_scheduler();
        if let Some(scheduler) = scheduler_lock.as_mut() {
            scheduler.reclaim_terminated_threads()
        } else {
            alloc::vec::Vec::new()
        }
    });
    release_reclaimed_threads(reclaimed_threads);
}

/// Free reclaimed control blocks with interrupts MASKED.
///
/// Dropping a `Box<Thread>` reaches `KernelStack::drop` -> `free_kernel_stack`,
/// which takes `ARM64_STACK_BITMAP`. Doing that preemptibly is link 1 of the
/// `#609` chain (docs/planning/teardown-unification/609-RCA-RETRACTION-2026-08-21.md
/// §2.3, which names this exact drop): a timer interrupt taken part-way through
/// leaves a bitmap holder preemptible, and builds its exception frame on
/// whatever stack the reaper is standing on. Masking closes that link, and
/// `#632` having made the bitmap lock an `IrqSafeMutex` is what makes the masked
/// wait bounded rather than a spin on an orphaned lock.
///
/// The masked work here is a slot-bitmap return plus a lock-free tracing
/// recorder — a bounded handful of instructions per thread.
#[cfg(target_arch = "aarch64")]
fn release_reclaimed_threads(reclaimed_threads: alloc::vec::Vec<Box<Thread>>) {
    #[cfg(feature = "coreproof")]
    if !reclaimed_threads.is_empty() {
        crate::proof_cover!(MaskedLock);
        #[cfg(feature = "coreproof_mut_masked_lock_bare")]
        crate::proof_cover!(MaskedLockBare);
    }
    // CORE-PROOF MUTATION LEG `coreproof_mut_masked_lock` (#609, fixed by PR
    // #645; PR #632 made the bitmap lock irq-safe): the release runs
    // preemptibly again, so a timer interrupt taken
    // part-way through leaves an `ARM64_STACK_BITMAP` holder preemptible and
    // builds its exception frame on whatever stack the reaper is standing on —
    // link 1 of the #609 chain, restored. Test profiles only. Expected
    // predicate: PERCPU_STACK_ALIEN.
    //
    // ROUND 3 MEASUREMENT, recorded where the leg is rather than only in the
    // register: this half of link 1 is no longer sufficient on its own. PR #632
    // typed `ARM64_STACK_BITMAP` as an `IrqSafeMutex`, so the holder cannot be
    // preempted however this call is entered, and the orphaned lock the field
    // failure needed cannot form. 15 mutated boots moved no existing census.
    // A faithful re-introduction has to restore the bare `spin::Mutex` too.
    //
    // M7 (`coreproof_mut_masked_lock_bare`, rung 2): the bare-mutex half of
    // #609's real defect, planted alongside `kernel_stack.rs`'s matching
    // `ARM64_STACK_BITMAP` type swap (both must move together, see that
    // file). This arm is REUSED, not duplicated: M7 shares M6's unmasked drop
    // exactly, because the drop being unmasked is only half of what M7 needs
    // to reopen. Expected outcome for M7 is NOT a `[COREPROOF:VIOLATION:...]`
    // line — it is the gate's own missing-RUN-record detector, because a
    // bitmap holder preempted mid-drop while the bitmap is a bare `spin::Mutex`
    // is exactly the #609 field failure: a wedged boot, not a marker. See
    // `kernel/src/proof/mutations.rs`'s M7 entry.
    #[cfg(any(
        feature = "coreproof_mut_masked_lock",
        feature = "coreproof_mut_masked_lock_bare"
    ))]
    drop(reclaimed_threads);
    #[cfg(not(any(
        feature = "coreproof_mut_masked_lock",
        feature = "coreproof_mut_masked_lock_bare"
    )))]
    without_interrupts(|| {
        drop(reclaimed_threads);
    });
}

/// Free reclaimed control blocks with interrupts as the caller left them.
///
/// The `#609` race the masked aarch64 release closes is an `ARM64_STACK_BITMAP`
/// race and does not exist here. What does exist here is the cost: the x86_64
/// `KernelStack::drop` walks `KERNEL_STACK_SIZE / 4096` pages, taking the kernel
/// page-table lock and the frame-allocator lock and doing TLB maintenance on
/// every one, then logs. Masking that would put an unbounded page-table teardown
/// inside a no-interrupt window on a path reachable from
/// `interrupts/context_switch.rs`, buying nothing. Mask what correctness
/// requires and no more.
#[cfg(not(target_arch = "aarch64"))]
fn release_reclaimed_threads(reclaimed_threads: alloc::vec::Vec<Box<Thread>>) {
    drop(reclaimed_threads);
}

/// Add a thread as the current running thread without scheduling.
///
/// Used when manually starting the first userspace thread (init process).
/// The thread is added to the scheduler's thread list and marked as current,
/// but NOT added to the ready queue and need_resched is NOT set.
/// This allows the thread to run without the scheduler trying to preempt it.
#[allow(dead_code)]
pub fn spawn_as_current(thread: Box<Thread>) {
    note_scheduler_publication();
    without_interrupts(|| {
        let mut scheduler_lock = lock_scheduler();
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
    // Component C's one seam (rung 2). Every caller reaches this point with
    // interrupts ENABLED — masking is `schedule_from_kernel`'s own job,
    // further down a call chain this harness may never seam
    // (`context_switch.rs` is permanently prohibited,
    // `scripts/check-coreproof-seams.sh`) — so `SiteClass::Open` admits every
    // stimulus action, including a `TimerSqueeze` at its full drawable range.
    // This is also the exact function the existing `KernelSchedule` and
    // `Steal` adversarial ops already call on every peer step, so no new call
    // site is needed to reach it. See `kernel/src/proof/driver_c.rs`.
    //
    // Gated additionally on `coreproof_component_c`: Component A's build
    // compiles a DIFFERENT `SiteId` (twelve variants, none named
    // `ScheduleEntry`), so this invocation must not exist at all outside a
    // Component C build — see `kernel/src/proof/sites.rs`.
    #[cfg(feature = "coreproof_component_c")]
    crate::proof_point!(ScheduleEntry);
    crate::arch_impl::aarch64::context_switch::run_deferred_reclamation();
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
            let mut scheduler_lock = lock_scheduler();
            if let Some(scheduler) = scheduler_lock.as_mut() {
                scheduler.schedule().map(|(old, new)| (old.id(), new.id()))
            } else {
                None
            }
        })
    } else {
        // Already in interrupt context - don't try to disable interrupts again
        let mut scheduler_lock = lock_scheduler();
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
    if let Some(mut scheduler_lock) = try_lock_scheduler() {
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
    if let Some(scheduler_lock) = try_lock_scheduler() {
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
        let mut scheduler_lock = lock_scheduler();
        let _scheduler_scope = crate::tracing::providers::teardown::SchedulerScope::enter();
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

/// Emit scheduler placement facts for one thread.
pub fn dump_thread_placement(tid: u64, label: &str) {
    Scheduler::dump_thread_placement(tid, label);
}

/// Collect the idle thread ID for each online CPU into a fixed-size buffer.
///
/// Returns the number of idle thread IDs written into `out` (one per online CPU).
/// The caller must pass a buffer large enough for `cpus_online` entries.
/// Safe to call from kernel context; disables interrupts internally.
pub fn collect_idle_thread_ids(out: &mut [u64]) -> usize {
    without_interrupts(|| {
        let scheduler_lock = lock_scheduler();
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
        let mut scheduler_lock = lock_scheduler();
        scheduler_lock
            .as_mut()
            .and_then(|sched| sched.get_thread_mut(thread_id).map(f))
    })
}

static CREATION_PUBLICATIONS: AtomicU64 = AtomicU64::new(0);
static CREATION_PUBLICATIONS_PM_HELD: AtomicU64 = AtomicU64::new(0);
static CREATION_PUBLICATIONS_PM_HELD_INJECTED: AtomicU64 = AtomicU64::new(0);

#[inline]
fn account_creation_publication_pm_held() -> bool {
    let pm_held = crate::process::process_manager_held_on_current_cpu();
    if pm_held {
        CREATION_PUBLICATIONS_PM_HELD.fetch_add(1, Ordering::Relaxed);
    }
    pm_held
}

/// Publishing a thread to the scheduler while this CPU still holds the
/// process-manager lock is the PM->SCHEDULER nesting PR #577 removed from the
/// exec path. Detect it at the publication seam so the creation paths carry the
/// same evidence the exec commit path does.
#[inline]
fn note_scheduler_publication() {
    CREATION_PUBLICATIONS.fetch_add(1, Ordering::Relaxed);
    if account_creation_publication_pm_held() {
        crate::serial_println!("[CREATION_LOCK_ORDER:VIOLATION:PM_HELD]");
    }
}

/// Drive the publication-seam lock-order detector deliberately, through the same
/// process-manager-held predicate the production seam uses, so the production
/// counter's zero is asserted against a demonstrated ability to fire.
#[cfg(feature = "boot_tests")]
pub fn probe_publication_lock_order_injection() -> bool {
    let pm_held = account_creation_publication_pm_held();
    if pm_held {
        CREATION_PUBLICATIONS_PM_HELD_INJECTED.fetch_add(1, Ordering::Relaxed);
        crate::serial_println!("[CREATION_LOCK_ORDER:INJECTED:PM_HELD]");
    }
    pm_held
}

#[derive(Clone, Copy)]
pub struct CreationLockOrderCounters {
    pub publications: u64,
    pub pm_held: u64,
    pub pm_held_injected: u64,
}

/// Read by the boot-test oracle.
pub fn creation_lock_order_counters() -> CreationLockOrderCounters {
    CreationLockOrderCounters {
        publications: CREATION_PUBLICATIONS.load(Ordering::Relaxed),
        pm_held: CREATION_PUBLICATIONS_PM_HELD.load(Ordering::Relaxed),
        pm_held_injected: CREATION_PUBLICATIONS_PM_HELD_INJECTED.load(Ordering::Relaxed),
    }
}

/// Number of exec scheduler-side commits applied (floor oracle: proves the path ran).
#[cfg(target_arch = "aarch64")]
pub static EXEC_SCHED_COMMITS: AtomicU64 = AtomicU64::new(0);

/// Times a commit ran while this CPU still owned the process-manager lock (must stay 0).
#[cfg(target_arch = "aarch64")]
pub static SCHED_AFTER_PM_VIOLATIONS: AtomicU64 = AtomicU64::new(0);

/// Times a commit ran while the exec'd thread was NOT this CPU's current thread (must stay 0).
#[cfg(target_arch = "aarch64")]
pub static EXEC_COMMIT_UNPINNED: AtomicU64 = AtomicU64::new(0);

/// Times a commit found no scheduler-side thread to write to (must stay 0).
///
/// The old in-manager `with_thread_mut` swallowed this case silently; the guaranteed consequence
/// is that the exec'd thread keeps its pre-exec context and faults on the first restore (the
/// historical `elr_el1 = 0` crash). Report it, never swallow it.
#[cfg(target_arch = "aarch64")]
pub static EXEC_COMMIT_MISSING_THREAD: AtomicU64 = AtomicU64::new(0);

/// Scheduler-side half of an aarch64 exec, staged under the process-manager lock and
/// committed after it is released.
///
/// `manager.rs` finalizes the process-manager copy of `main_thread`, snapshots it into this
/// receipt, and returns it. The caller drops the process-manager guard and then calls
/// [`ExecSchedCommit::apply`], which is the only place the SCHEDULER lock is taken for exec.
/// This keeps the Level 1 (SCHEDULER) / Level 2 (PROCESS_MANAGER) hierarchy un-nested.
#[cfg(target_arch = "aarch64")]
#[must_use = "the scheduler-side exec state must be committed after the process-manager lock is released"]
pub struct ExecSchedCommit {
    thread_id: u64,
    context: CpuContext,
    stack_top: VirtAddr,
    stack_bottom: VirtAddr,
    kernel_stack_top: Option<VirtAddr>,
    tls_block: VirtAddr,
    new_ttbr0: u64,
}

#[cfg(target_arch = "aarch64")]
impl ExecSchedCommit {
    pub fn new(
        thread_id: u64,
        context: CpuContext,
        stack_top: VirtAddr,
        stack_bottom: VirtAddr,
        kernel_stack_top: Option<VirtAddr>,
        tls_block: VirtAddr,
        new_ttbr0: u64,
    ) -> Self {
        Self {
            thread_id,
            context,
            stack_top,
            stack_bottom,
            kernel_stack_top,
            tls_block,
            new_ttbr0,
        }
    }

    pub fn new_ttbr0(&self) -> u64 {
        self.new_ttbr0
    }

    pub fn apply(self) {
        without_interrupts(|| {
            // Oracle 1: the whole point of the receipt is that the process-manager lock is gone.
            let pm_held = crate::process::process_manager_held_on_current_cpu();

            // Oracle 2: the safety argument rests on the exec'd thread being this CPU's current
            // thread (a current thread is in no run queue, so no peer can dispatch it in the gap).
            let mut unpinned = false;
            let mut applied = false;
            {
                let mut scheduler_lock = lock_scheduler();
                if let Some(sched) = scheduler_lock.as_mut() {
                    unpinned = sched.current_thread_id_inner() != Some(self.thread_id);
                    if let Some(t) = sched.get_thread_mut(self.thread_id) {
                        #[cfg(feature = "ret_zero_pc_oracle_exec")]
                        crate::task::ret_zero_pc_oracle::inject_exec_commit_if_armed(t);
                        t.context = self.context;
                        t.clear_inline_schedule_state();
                        t.stack_top = self.stack_top;
                        t.stack_bottom = self.stack_bottom;
                        t.kernel_stack_top = self.kernel_stack_top;
                        t.tls_block = self.tls_block;
                        t.state = crate::task::thread::ThreadState::Ready;
                        #[cfg(feature = "ret_zero_pc_oracle_exec")]
                        crate::task::ret_zero_pc_oracle::record_exec_commit_inline_state(t);
                        applied = true;
                    }
                }
            }

            // Gate-pinned lines must take the serial lock so a concurrent writer cannot
            // tear their bytes. The scheduler guard above is already out of scope.
            if pm_held {
                SCHED_AFTER_PM_VIOLATIONS.fetch_add(1, Ordering::Relaxed);
                crate::serial_println!("[EXEC_LOCK_ORDER:VIOLATION:PM_HELD]");
            }
            if unpinned {
                EXEC_COMMIT_UNPINNED.fetch_add(1, Ordering::Relaxed);
                crate::serial_println!("[EXEC_LOCK_ORDER:VIOLATION:UNPINNED]");
            }
            if !applied {
                EXEC_COMMIT_MISSING_THREAD.fetch_add(1, Ordering::Relaxed);
                crate::serial_println!("[EXEC_LOCK_ORDER:VIOLATION:NO_SCHED_THREAD]");
            }
            if applied && EXEC_SCHED_COMMITS.fetch_add(1, Ordering::Relaxed) == 0 {
                crate::serial_println!("[EXEC_LOCK_ORDER:FIRST_COMMIT]");
            }
        })
    }
}

/// Wake a waitqueue waiter immediately from task context.
///
/// The caller must not be in interrupt context; hard IRQ paths must use
/// `isr_unblock_for_io` so they never spin on the scheduler lock.
pub fn wake_waitqueue_thread(tid: u64) {
    with_scheduler(|sched| sched.wake_waitqueue_thread(tid));
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
        if let Some(scheduler_lock) = try_lock_scheduler() {
            if let Some(scheduler) = scheduler_lock.as_ref() {
                let now = crate::time::get_ticks();
                return scheduler
                    .threads
                    .iter()
                    .filter_map(|t| {
                        t.owner_pid.map(|pid| {
                            let mut ticks = t.cpu_ticks_total;
                            // If thread is currently running, add in-flight ticks
                            if t.state == super::thread::ThreadState::Running
                                && !t.blocked_in_syscall
                            {
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

/// Get a process display state from its scheduler-owned threads.
///
/// The process manager's coarse state can remain Ready while all of the
/// process's threads are blocked in syscalls. Procfs consumers such as btop
/// need the thread-level runtime state to avoid presenting sleepers as CPU-bound.
pub fn get_process_display_state(owner_pid: u64) -> Option<&'static str> {
    without_interrupts(|| {
        let scheduler_lock = try_lock_scheduler()?;
        let scheduler = scheduler_lock.as_ref()?;

        let mut saw_ready = false;
        let mut saw_blocked = false;
        let mut saw_terminated = false;

        for thread in scheduler
            .threads
            .iter()
            .filter(|thread| thread.owner_pid == Some(owner_pid))
        {
            match thread.state {
                super::thread::ThreadState::Running => return Some("Running"),
                super::thread::ThreadState::Ready => saw_ready = true,
                super::thread::ThreadState::Blocked
                | super::thread::ThreadState::BlockedOnSignal
                | super::thread::ThreadState::BlockedOnChildExit
                | super::thread::ThreadState::BlockedOnTimer
                | super::thread::ThreadState::BlockedOnIO => saw_blocked = true,
                super::thread::ThreadState::Terminated => saw_terminated = true,
            }
        }

        if saw_ready {
            Some("Ready")
        } else if saw_blocked {
            Some("Blocked")
        } else if saw_terminated {
            Some("Terminated")
        } else {
            None
        }
    })
}

/// Get the current thread ID
/// This function disables interrupts to prevent deadlock with timer interrupt
pub fn current_thread_id() -> Option<u64> {
    without_interrupts(|| {
        let scheduler_lock = lock_scheduler();
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
        let mut scheduler_lock = lock_scheduler();
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

/// Count pending ISR wakeups for one CPU.
///
/// This is diagnostic-only and lock-free; it reads the wake buffer atomically
/// without draining it.
pub fn isr_wakeup_depth(cpu: usize) -> usize {
    if cpu < ISR_WAKEUP_BUFFERS.len() {
        ISR_WAKEUP_BUFFERS[cpu].depth()
    } else {
        0
    }
}

/// Number of interrupt-context wakes that could not be buffered.
pub fn isr_wakeup_buffer_full() -> u64 {
    ENQUEUE_ISR_BUFFER_FULL.load(Ordering::Relaxed)
}

/// Number of interrupt-context wakes deduplicated against an existing slot.
pub fn isr_wakeup_buffer_dedup() -> u64 {
    ENQUEUE_ISR_BUFFER_DEDUP.load(Ordering::Relaxed)
}

/// Number of threads recovered from ready queues belonging to offline CPUs.
pub fn enqueue_offline_reclaimed() -> u64 {
    ENQUEUE_OFFLINE_RECLAIMED.load(Ordering::Relaxed)
}

/// Number of threads recovered from ready queues belonging to stalled CPUs.
pub fn enqueue_stalled_reclaimed() -> u64 {
    ENQUEUE_STALLED_RECLAIMED.load(Ordering::Relaxed)
}

/// Return the first offline ready queue that still contains threads.
pub fn offline_queue_occupancy() -> Option<(usize, usize)> {
    with_scheduler(|scheduler| {
        (scheduler.online_cpu_count()..MAX_CPUS).find_map(|cpu| {
            let occupancy = scheduler.per_cpu_queues[cpu].len();
            (occupancy != 0).then_some((cpu, occupancy))
        })
    })
    .flatten()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeOutcome {
    /// State transition applied inline under the scheduler lock.
    Applied,
    /// The target thread exists and is already Ready or Running.
    AlreadyRunnable,
    /// Buffered lock-free into the per-CPU ISR wakeup buffer; the next schedule() applies it.
    Buffered,
    /// The wake was neither applied nor buffered. The caller must retain it.
    Rejected,
}

/// Wake `tid` from any context.
pub fn wake_thread_any_context(tid: u64) -> WakeOutcome {
    if crate::per_cpu::is_initialized() && crate::per_cpu::in_interrupt() {
        let outcome = buffer_isr_wakeup(tid);
        if outcome == WakeOutcome::Buffered {
            set_need_resched();
        }
        outcome
    } else {
        match with_scheduler(|scheduler| scheduler.unblock(tid)) {
            Some(UnblockOutcome::Transitioned) => {
                set_need_resched();
                WakeOutcome::Applied
            }
            Some(UnblockOutcome::AlreadyRunnable) => WakeOutcome::AlreadyRunnable,
            Some(UnblockOutcome::NotFound) | None => WakeOutcome::Rejected,
        }
    }
}

fn buffer_isr_wakeup(tid: u64) -> WakeOutcome {
    let cpu = current_cpu_id_raw();
    if cpu >= ISR_WAKEUP_BUFFERS.len() {
        ENQUEUE_ISR_BUFFER_FULL.fetch_add(1, Ordering::Relaxed);
        return WakeOutcome::Rejected;
    }

    WAKE_SITE_ISR_UNBLOCK.fetch_add(1, Ordering::Relaxed);
    match ISR_WAKEUP_BUFFERS[cpu].push(tid) {
        IsrWakePush::Inserted => {
            ENQUEUE_ISR_BUFFER.fetch_add(1, Ordering::Relaxed);
            WakeOutcome::Buffered
        }
        IsrWakePush::AlreadyPending => {
            ENQUEUE_ISR_BUFFER_DEDUP.fetch_add(1, Ordering::Relaxed);
            WakeOutcome::Buffered
        }
        IsrWakePush::Full => {
            if !crate::per_cpu::in_interrupt() {
                match with_scheduler(|scheduler| scheduler.unblock(tid)) {
                    Some(UnblockOutcome::Transitioned) => WakeOutcome::Applied,
                    Some(UnblockOutcome::AlreadyRunnable) => WakeOutcome::AlreadyRunnable,
                    Some(UnblockOutcome::NotFound) | None => WakeOutcome::Rejected,
                }
            } else {
                ENQUEUE_ISR_BUFFER_FULL.fetch_add(1, Ordering::Relaxed);
                WakeOutcome::Rejected
            }
        }
    }
}

/// Buffer an I/O waiter wake from interrupt or thread context.
///
/// Called from the AHCI ISR (via `Completion::complete()`) instead of
/// `with_scheduler(|s| s.unblock_for_io(tid))`.  This avoids acquiring the
/// global SCHEDULER mutex on the normal completion path, which was the root
/// cause of CPU 0's IRQ death: the ISR would spin on the lock with IRQs masked,
/// starving the timer for milliseconds. On thread-context buffer overflow,
/// `buffer_isr_wakeup` applies the wake inline so it is not lost.
///
/// The scheduler drains the buffer under its own lock at the top of every
/// `schedule_deferred_requeue()` / `schedule()` call.
pub fn isr_unblock_for_io(tid: u64) {
    let _ = buffer_isr_wakeup(tid);
    set_need_resched();
    // The current CPU will drain the wake buffer on IRQ-return scheduling.
    // Avoid broadcasting reschedule SGIs from hard IRQ context; Linux's TTWU
    // path queues wake work to a selected target CPU rather than scanning idle CPUs.
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

/// Make a thread that the dispatch path refused queue-reachable again.
///
/// x86_64 counterpart of the aarch64 `requeue_thread_after_save` call in the
/// `RowUnpublished`/`PmLockBusy` dispatch arm: a retry-only refusal must leave the
/// refused thread reachable, or it is stranded (neither current nor queued) forever.
/// Callers must have already redirected this CPU to idle, so `current_thread` no
/// longer names the refused thread.
#[cfg(target_arch = "x86_64")]
pub fn requeue_refused_dispatch(thread_id: u64) {
    with_scheduler(|sched| {
        if (0..MAX_CPUS).any(|cpu| sched.cpu_state[cpu].idle_thread == thread_id) {
            return;
        }
        if (0..MAX_CPUS).any(|cpu| sched.cpu_state[cpu].current_thread == Some(thread_id)) {
            return;
        }
        let Some(thread) = sched.get_thread(thread_id) else {
            return;
        };
        if thread.state != ThreadState::Ready {
            return;
        }
        if sched
            .per_cpu_queues
            .iter()
            .any(|queue| queue.contains(&thread_id))
        {
            return;
        }
        let cpu = Scheduler::current_cpu_id();
        sched.per_cpu_queues[cpu].push_back(thread_id);
    });
}

/// Undo a dispatch the interrupt-return path could not complete: return the
/// undispatched thread to the ready queue and restore the interrupted thread's
/// pre-dispatch state. Only a runnable interrupted thread is transitioned back to
/// Running and dequeued. The scheduler's current_thread still records the context
/// IRETQ will actually resume; handing the CPU to idle here would strand whatever
/// lock the interrupted thread holds - which is the condition that aborts dispatch.
#[cfg(target_arch = "x86_64")]
pub fn abort_dispatch_and_resume(aborted_thread_id: u64, resume_thread_id: u64) {
    with_scheduler(|sched| {
        let cpu_id = Scheduler::current_cpu_id();

        if sched.get_thread(aborted_thread_id).is_none()
            || sched.get_thread(resume_thread_id).is_none()
        {
            log::error!(
                "Cannot abort dispatch of thread {} and resume thread {}: scheduler thread missing",
                aborted_thread_id,
                resume_thread_id
            );
            return;
        }

        let should_queue = match sched.get_thread_mut(aborted_thread_id) {
            Some(thread) if thread.state != ThreadState::Terminated => {
                thread.set_ready();
                true
            }
            _ => false,
        };
        let in_queue = sched
            .per_cpu_queues
            .iter()
            .any(|queue| queue.contains(&aborted_thread_id));
        if should_queue && !in_queue {
            sched.per_cpu_queues[cpu_id].push_back(aborted_thread_id);
        }

        let (resume_runnable, kernel_stack_top, thread_ptr) =
            match sched.get_thread_mut(resume_thread_id) {
                Some(thread) => {
                    let resume_runnable =
                        matches!(thread.state, ThreadState::Ready | ThreadState::Running);
                    // A wake before rollback is already visible as Ready, so taking the
                    // runnable transition preserves it; preserving Blocked keeps the
                    // thread matchable by the next wake. No wake can land between
                    // schedule and rollback while interrupts and the scheduler lock are
                    // held; buffered ISR wakes drain at the next schedule.
                    if resume_runnable {
                        thread.set_running();
                    }
                    (
                        resume_runnable,
                        thread.kernel_stack_top,
                        thread as *const _ as *mut crate::task::thread::Thread,
                    )
                }
                None => return,
            };

        if resume_runnable {
            for queue in sched.per_cpu_queues.iter_mut() {
                if let Some(position) = queue.iter().position(|&id| id == resume_thread_id) {
                    queue.remove(position);
                }
            }
        }

        sched.cpu_state[cpu_id].current_thread = Some(resume_thread_id);
        crate::per_cpu::set_current_thread(thread_ptr);
        if let Some(kernel_stack_top) = kernel_stack_top {
            crate::per_cpu::update_tss_rsp0(kernel_stack_top.as_u64());
        }
    });
}

/// Best-effort thread termination for EL1 crash recovery.
///
/// A fatal exception may interrupt a context that already owns SCHEDULER, so
/// this helper refuses to wait for the lock and leaves cleanup to a later path.
#[cfg(target_arch = "aarch64")]
pub fn terminate_thread_best_effort(thread_id: u64) -> bool {
    let Some(mut scheduler_lock) = try_lock_scheduler() else {
        return false;
    };
    let Some(sched) = scheduler_lock.as_mut() else {
        return false;
    };

    if let Some(thread) = sched.get_thread_mut(thread_id) {
        thread.set_terminated();
    }
    sched.remove_from_ready_queue(thread_id);
    true
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
    if let Some(mut scheduler_lock) = try_lock_scheduler() {
        if let Some(sched) = scheduler_lock.as_mut() {
            let cpu_id = Scheduler::current_cpu_id();
            let idle_id = sched.cpu_state[cpu_id].idle_thread;
            let old_val = sched.cpu_state[cpu_id].current_thread.unwrap_or(0xDEAD);
            record_cpu_state_change(cpu_id, 3, old_val, idle_id);
            sched.cpu_state[cpu_id].current_thread = Some(idle_id);
            // Resolve previous_thread before returning to idle. If it is a stranded
            // Ready thread, make it queue-reachable instead of simply dropping the marker.
            sched.resolve_exception_cleanup_previous_thread(cpu_id);
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

/// Boot-test entry point for the blocked-yet-dispatchable probe.
///
/// Runs at `SerialBoot` because the probe deliberately mutates shared scheduler
/// state and restores it exactly.
#[cfg(feature = "boot_tests")]
pub fn block_current_departure_gate_test() -> crate::test_framework::registry::TestResult {
    use crate::test_framework::registry::TestResult;
    match with_scheduler(|sched| sched.block_current_departure_gate()) {
        Some(Ok(())) => TestResult::Pass,
        Some(Err(reason)) => TestResult::Fail(reason),
        None => TestResult::Fail("departure gate could not reach the scheduler"),
    }
}

/// Drive real scheduler boundaries on every online CPU for teardown grace-period
/// boot tests. This must not use the idle-only wake helper: a retiring thread's
/// last recorded owner can still appear non-idle until that CPU crosses a
/// scheduler boundary and republishes its live-stack snapshot.
#[cfg(feature = "boot_tests")]
pub fn nudge_retirement_grace_for_test() {
    #[cfg(target_arch = "aarch64")]
    {
        use crate::arch_impl::aarch64::{constants::SGI_RESCHEDULE, gic, smp};

        let current_cpu = Scheduler::current_cpu_id();
        let online = smp::cpus_online() as usize;
        for cpu in 0..online.min(MAX_CPUS) {
            if cpu != current_cpu {
                gic::send_sgi(SGI_RESCHEDULE as u8, cpu as u8);
            }
        }
    }
    set_need_resched();
}

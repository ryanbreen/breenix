//! The park contract, bound to the kernel (E2.2,
//! docs/plans/e2-sleepable-contexts.md §3.1-§3.4).
//!
//! [`context`] is the one place that decides whether the current context may
//! park. [`wait`] and [`wait_any`] are the sleep, on the E2.1 event queue.
//! Both are `task::park_core`, bound here to the scheduler, the per-CPU
//! preemption count and the thread's recorded baseline.
//!
//! # Baselines (§3.2)
//!
//! A syscall holds one preemption level, the syscall brake, for its whole
//! length. Syscall entry records that as the thread's baseline
//! ([`enter_syscall`]); the return to user mode clears it
//! ([`leave_syscall`]). Kernel threads run at baseline 0. A kernel thread
//! that must keep its CPU except while it sleeps, as a syscall does, holds a
//! [`Braked`] guard, which raises the depth and the baseline together.
//!
//! # What still decides by hand
//!
//! A few sleeps have not moved onto [`wait`] yet: the wait-queue's own
//! `schedule_current_wait`, `Completion`, the syscall prepared waits and the
//! retry pause. Their level handling is [`release_for_sleep`] and
//! [`retake_after_sleep`], here, and [`sleepable_braked`] names the contexts
//! their sleep paths serve. E2.3 and E2.4 remove those callers.

use super::event_queue::EventQueue;
use super::park_core::{self, ContextInputs, ParkHooks, SignalEnd, WaitWord};
use super::waitqueue::{KernelHooks, WaitQueueHead};
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

pub use super::park_core::{
    announce, exit_switch, should_wake, withdraw, Arrival, AtomicReason, ExitSwitch, ExitThread, ParkContext, ParkError,
    Receipt, WaitEnd, WaitMode,
};

/// A thread's park state: its wait word and its baseline.
pub(crate) struct ParkState {
    pub(crate) word: WaitWord,
    baseline: AtomicU32,
    baseline_irqs_masked: AtomicBool,
}

impl ParkState {
    pub(crate) const fn new() -> Self {
        Self { word: WaitWord::new(), baseline: AtomicU32::new(0), baseline_irqs_masked: AtomicBool::new(false) }
    }
}

fn current_thread() -> Option<&'static super::thread::Thread> {
    #[cfg(target_arch = "x86_64")]
    let thread = crate::per_cpu::current_thread();
    #[cfg(target_arch = "aarch64")]
    let thread = crate::per_cpu_aarch64::current_thread();
    thread.map(|thread| &*thread)
}

fn depth() -> u32 {
    crate::per_cpu::preempt_count() & crate::arch_impl::current::constants::PREEMPT_MASK
}

fn baseline() -> u32 {
    current_thread().map_or(0, |thread| thread.park.baseline.load(Ordering::Relaxed))
}

/// The scheduler dispatches here: it has a current thread and, on ARM64, the
/// timer runs, so something ends a sleep. (Until E2.4's early-boot token,
/// these are the conditions the converted predicates tested.)
fn dispatching() -> bool {
    #[cfg(target_arch = "aarch64")]
    if !crate::arch_impl::aarch64::timer_interrupt::timer_is_running() {
        return false;
    }
    crate::task::scheduler::current_thread_id().is_some()
}

/// Whether the current context may park (design §3.1). The only
/// implementation of that decision.
pub fn context() -> ParkContext {
    let mut inputs = ContextInputs {
        hardirq: crate::per_cpu::in_hardirq(),
        softirq: crate::per_cpu::in_softirq(),
        nmi: crate::per_cpu::in_nmi(),
        ..ContextInputs::default()
    };
    // Interrupt context first: the reads below take scheduler locks, which
    // interrupt context may not.
    if inputs.hardirq || inputs.softirq || inputs.nmi {
        return park_core::classify(inputs);
    }
    inputs.dispatching = dispatching();
    if inputs.dispatching {
        let tid = crate::task::scheduler::current_thread_id();
        let thread = current_thread().filter(|thread| Some(thread.id) == tid);
        inputs.has_thread = thread.is_some();
        inputs.idle = crate::task::scheduler::with_current(|sched| Some(sched.idle_thread()) == tid) != Some(false);
        if let Some(thread) = thread {
            inputs.baseline = thread.park.baseline.load(Ordering::Relaxed);
            inputs.baseline_irqs_masked = thread.park.baseline_irqs_masked.load(Ordering::Relaxed);
        }
    }
    inputs.depth = depth();
    inputs.irqs_masked = !crate::arch_interrupts_enabled();
    park_core::classify(inputs)
}

/// Sleepable, at a baseline of exactly one braked level (a syscall, or a
/// [`Braked`] kernel thread). The sleep paths that release the syscall brake
/// by hand release exactly one level, so they serve exactly these contexts:
/// at a deeper baseline such a sleep would park with a level still held.
pub fn sleepable_braked() -> bool {
    context() == ParkContext::Sleepable && baseline() == 1
}

/// A violation by the current context at the caller: a receipt, and the
/// error. For code about to do something that may sleep (design §3.1).
#[track_caller]
pub fn require_sleepable() -> Result<(), ParkError> {
    match context() {
        ParkContext::Sleepable => Ok(()),
        refused => {
            record(refused, site());
            Err(ParkError::NotSleepable(refused))
        }
    }
}

/// Whether the preempt count is 0, so that a waiter halting as the current thread
/// (`arch_halt_while_blocked`) may switch away rather than keep its CPU.
pub(crate) fn may_switch_now() -> bool {
    // The whole count, interrupt-context fields included, as the halt tested
    // before E2.2: a waiter inside hard or soft IRQ context keeps its CPU.
    crate::per_cpu::preempt_count() == 0
}

// ------------------------------------------------------------------ receipts

const RECEIPT_CPUS: usize = 16;
const RECEIPTS_PER_CPU: usize = 16;

static RECEIPTS: [park_core::ReceiptRing<RECEIPTS_PER_CPU>; RECEIPT_CPUS] =
    [const { park_core::ReceiptRing::new() }; RECEIPT_CPUS];

#[track_caller]
fn site() -> u64 {
    core::panic::Location::caller() as *const core::panic::Location<'static> as u64
}

fn record(context: ParkContext, site: u64) {
    #[cfg(target_arch = "x86_64")]
    let cpu = crate::per_cpu::cpu_id() % RECEIPT_CPUS;
    #[cfg(target_arch = "aarch64")]
    let cpu = crate::arch_impl::aarch64::cpu::cpu_id() as usize % RECEIPT_CPUS;
    let tid = current_thread().map_or(0, |thread| thread.id);
    RECEIPTS[cpu].record(Receipt { site, context, tid, depth: depth() });
}

/// Contract violations recorded on `cpu` so far, newest first into `out`.
/// Diagnostics stay on and are read out only when something fails.
pub fn receipts(cpu: usize, out: &mut [Receipt]) -> usize {
    RECEIPTS[cpu % RECEIPT_CPUS].latest(out)
}

/// How many violations `cpu` has recorded.
pub fn receipt_count(cpu: usize) -> u64 {
    RECEIPTS[cpu % RECEIPT_CPUS].written()
}

// ----------------------------------------------------------------- baselines

/// Syscall entry: the syscall brake is the thread's baseline. x86 syscalls
/// run with interrupts masked (until design §4.3), so their baseline masks
/// them.
pub(crate) fn enter_syscall() {
    if let Some(thread) = current_thread() {
        thread.park.baseline.store(1, Ordering::Relaxed);
        thread.park.baseline_irqs_masked.store(cfg!(target_arch = "x86_64"), Ordering::Relaxed);
    }
}

/// The return to user mode: no syscall brake from here on.
pub(crate) fn leave_syscall() {
    if let Some(thread) = current_thread() {
        thread.park.baseline.store(0, Ordering::Relaxed);
        thread.park.baseline_irqs_masked.store(false, Ordering::Relaxed);
    }
}

/// Raise the depth and the baseline together by one, for a kernel thread
/// that keeps its CPU except while it sleeps (design §3.2).
pub struct Braked {
    _level: park_core::Braked<'static, KernelHooks>,
}

static HOOKS: KernelHooks = KernelHooks;

impl Braked {
    #[must_use = "the level is released when the guard drops"]
    pub fn new() -> Self {
        Self { _level: park_core::Braked::new(&HOOKS) }
    }
}

impl Default for Braked {
    fn default() -> Self {
        Self::new()
    }
}

/// Run `f` with the innermost [`Braked`] level released, depth and baseline
/// together, and take it back afterwards: for a step inside a braked section
/// that must let this CPU dispatch other threads.
pub fn without_braked<R>(f: impl FnOnce() -> R) -> R {
    let base = baseline();
    debug_assert!(base >= 1 && depth() >= 1, "without_braked outside a Braked section");
    HOOKS.set_baseline(base.saturating_sub(1));
    crate::per_cpu::preempt_enable();
    let result = f();
    crate::per_cpu::preempt_disable();
    HOOKS.set_baseline(base);
    result
}

// ---------------------------------------------------------------- the waits

/// Wait on `queue` until `done()` holds or the wait ends (design §3.3). From
/// a context that may not park: a receipt and `Err(NotSleepable)`, and
/// the thread does not publish. A wake is a hint; recheck the condition.
#[track_caller]
pub fn wait(
    queue: &WaitQueueHead,
    done: impl FnMut() -> bool,
    deadline_ns: Option<u64>,
    mode: WaitMode,
) -> Result<WaitEnd, ParkError> {
    park_core::wait(&HOOKS, site(), queue.event_queue(), done, deadline_ns, mode)
}

/// [`wait`], except that the ARM64 boot continuation, which runs as CPU 0's
/// idle thread and reads storage there until E2.4 moves those reads into
/// a kernel thread (design §6), keeps the sleep it had before E2.2: publish,
/// release its one level, switch, finish. It still gets its receipt. Other
/// refused contexts get `Err(NotSleepable)`.
#[track_caller]
pub fn wait_allowing_boot_idle(
    queue: &WaitQueueHead,
    mut done: impl FnMut() -> bool,
    deadline_ns: Option<u64>,
    mode: WaitMode,
) -> Result<WaitEnd, ParkError> {
    match park_core::wait(&HOOKS, site(), queue.event_queue(), &mut done, deadline_ns, mode) {
        Err(ParkError::NotSleepable(ParkContext::Atomic(AtomicReason::Idle))) => {
            use super::thread::ThreadState;
            use super::waitqueue::PrepareOutcome;
            Ok(match queue.prepare_to_wait_checked(ThreadState::BlockedOnIO, deadline_ns, || !done()) {
                PrepareOutcome::Queued => {
                    super::waitqueue::schedule_current_wait();
                    queue.finish_wait();
                    WaitEnd::Woken
                }
                PrepareOutcome::Mismatch => WaitEnd::Condition,
                PrepareOutcome::OutOfMemory => WaitEnd::NoMemory,
                PrepareOutcome::PublishFailed => WaitEnd::Killed,
            })
        }
        other => other,
    }
}

/// The most queues one [`wait_any`] waits on.
pub const WAIT_ANY_MAX: usize = 8;

/// Wait on up to [`WAIT_ANY_MAX`] queues at once, with one wait sequence.
#[track_caller]
pub fn wait_any(
    queues: &[&WaitQueueHead],
    done: impl FnMut() -> bool,
    deadline_ns: Option<u64>,
    mode: WaitMode,
) -> Result<WaitEnd, ParkError> {
    let site = site();
    let Some(first) = queues.first() else {
        return Ok(WaitEnd::Condition);
    };
    if queues.len() > WAIT_ANY_MAX {
        return Ok(WaitEnd::NoMemory);
    }
    let mut events: [&EventQueue; WAIT_ANY_MAX] = [first.event_queue(); WAIT_ANY_MAX];
    for (slot, queue) in events.iter_mut().zip(queues) {
        *slot = queue.event_queue();
    }
    park_core::wait_any(&HOOKS, site, &events[..queues.len()], done, deadline_ns, mode)
}

/// A queue waker found `tid` linked under the queue lock: end its current
/// wait as woken. Lock-free, so interrupt and deferred wakers may call it.
/// A thread the lock-free index cannot reach yet is skipped: an interrupt
/// wake cannot reach it either, and a task-context wake's caller falls back
/// to the scheduler lock (`arrive_woken_task`).
pub(crate) fn arrive_woken(tid: u64) -> bool {
    crate::task::scheduler::with_park_word(tid, |word| {
        word.arrive_current(Arrival::Woken);
    })
    .is_some()
}

/// A task-context waker: the lock-free path, or the thread under the
/// scheduler lock.
pub(crate) fn arrive_woken_task(tid: u64) {
    if !arrive_woken(tid) {
        crate::task::scheduler::with_thread_mut(tid, |thread| {
            thread.park.word.arrive_current(Arrival::Woken);
        });
    }
}

/// The timer expired a live deadline of a thread published blocked on I/O.
pub(crate) fn arrive_timeout(thread: &super::thread::Thread) {
    thread.park.word.arrive_current(Arrival::Timeout);
}

// ------------------------------------------------------ the interrupt exit

/// How an interrupt exit sees `thread` (design §3.5).
pub(crate) fn exit_thread(thread: &super::thread::Thread) -> ExitThread {
    use super::thread::ThreadState;
    match thread.state {
        ThreadState::Terminated => ExitThread::Terminated,
        ThreadState::BlockedOnIO => ExitThread::BlockedOnIo(thread.park.word.state().1),
        state if state.is_blocked() => ExitThread::Blocked,
        _ => ExitThread::Runnable,
    }
}

/// The interrupt exit's decision for `thread`, the thread it interrupted
/// (`park_core::exit_switch`). The caller holds the lock that owns the
/// thread's scheduler state.
pub(crate) fn exit_switch_for(thread: &super::thread::Thread, need_resched: bool) -> ExitSwitch {
    park_core::exit_switch(exit_thread(thread), need_resched)
}

// -------------------------------------------- sleeps not yet on park::wait

/// What [`release_for_sleep`] released.
#[derive(Clone, Copy)]
#[must_use]
pub(crate) struct SleepRelease(bool);

/// For the sleeps not yet on [`wait`]: release the one level such a sleep
/// releases today, if a level is held (B1 requirement 9). Callers do no
/// arithmetic of their own; E2.3 and E2.4 move them onto [`wait`].
pub(crate) fn release_for_sleep() -> SleepRelease {
    let held = depth() != 0;
    if held {
        crate::per_cpu::preempt_enable();
    }
    SleepRelease(held)
}

/// Take back what [`release_for_sleep`] released.
pub(crate) fn retake_after_sleep(released: SleepRelease) {
    if released.0 {
        crate::per_cpu::preempt_disable();
    }
}

/// Back off when no waiter could be published (no memory for the entry):
/// let another thread run here until the next interrupt, then return with
/// the caller's depth.
pub(crate) fn retry_pause() {
    crate::task::scheduler::set_need_resched();
    let released = release_for_sleep();
    crate::arch_halt_with_interrupts();
    retake_after_sleep(released);
}

// --------------------------------------------------------------- the hooks

impl ParkHooks for KernelHooks {
    fn context(&self) -> ParkContext {
        context()
    }

    fn receipt(&self, context: ParkContext, site: u64) {
        record(context, site);
    }

    fn word(&self) -> Option<&WaitWord> {
        current_thread().map(|thread| &thread.park.word)
    }

    fn depth(&self) -> u32 {
        depth()
    }

    fn baseline(&self) -> u32 {
        baseline()
    }

    fn set_baseline(&self, baseline: u32) {
        if let Some(thread) = current_thread() {
            thread.park.baseline.store(baseline, Ordering::Relaxed);
        }
    }

    fn raise(&self) {
        crate::per_cpu::preempt_disable();
    }

    fn lower(&self) {
        crate::per_cpu::preempt_enable();
    }

    fn irqs_enabled(&self) -> bool {
        crate::arch_interrupts_enabled()
    }

    fn set_irqs(&self, enabled: bool) {
        // SAFETY: restores the interrupt state the wait's caller entered with.
        unsafe {
            if enabled {
                crate::arch_enable_interrupts();
            } else {
                crate::arch_disable_interrupts();
            }
        }
    }

    fn now_ns(&self) -> u64 {
        let (secs, nanos) = crate::time::get_monotonic_time_ns();
        (secs as u64).saturating_mul(1_000_000_000).saturating_add(nanos as u64)
    }

    fn pending_signal(&self, mode: WaitMode) -> Option<SignalEnd> {
        let killed = || crate::syscall::fatal_signal_pending() || super::thread::current_wait_must_unwind();
        match mode {
            WaitMode::Uninterruptible => None,
            WaitMode::Killable => killed().then_some(SignalEnd::Killed),
            WaitMode::Interruptible => {
                if killed() {
                    Some(SignalEnd::Killed)
                } else {
                    crate::syscall::check_signals_for_restartable_wait().map(SignalEnd::Interrupted)
                }
            }
        }
    }

    fn republish(&self, tid: u64, deadline_ns: Option<u64>) -> bool {
        use super::event_queue::Hooks;
        self.publish_blocked(tid, deadline_ns, 0)
    }

    fn sleep(&self) {
        super::waitqueue::sleep_released();
    }
}

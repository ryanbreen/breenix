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
//! `schedule_current_wait`, the syscall prepared waits and the retry pause.
//! Their level handling is [`release_for_sleep`] and [`retake_after_sleep`],
//! here, and [`sleepable_braked`] names the contexts their sleep paths serve.
//! E2.10 moves those callers onto [`wait`].
//!
//! # Block I/O (E2.4, §6)
//!
//! Block I/O requires a context that may park or the early-boot token
//! ([`io_context`]). `kernel_main` mints the token before any storage
//! command and revokes it at the first dispatch ([`revoke_early_boot_io`]),
//! which is also when [`context`] stops answering `EarlyBoot`. `Completion`
//! decides its wait by the same rule: a sleepable caller sleeps, releasing
//! its baseline ([`release_baseline`]), and early boot polls.

use super::event_queue::EventQueue;
use super::park_core::{self, CompletionSources, ContextInputs, BLOCK_COMPLETION_SOURCES, EarlyBootIo, ParkHooks, SignalEnd, WaitWord};
use super::waitqueue::{KernelHooks, WaitQueueHead};
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

pub use super::park_core::{
    announce, exit_switch, should_wake, withdraw, Arrival, AtomicReason, ExitSwitch, ExitThread, IoContext, ParkContext,
    ParkError, Receipt, WaitEnd, WaitMode,
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

/// The scheduler dispatches here: the early-boot token has been revoked,
/// which `kernel_main` does at the first dispatch once the timer runs
/// (design §6), and this CPU has a current thread.
fn dispatching() -> bool {
    EARLY_BOOT_IO.revoked() && crate::task::scheduler::current_thread_id().is_some()
}

// ------------------------------------------- block I/O's caller rule (E2.4)

static EARLY_BOOT_IO: EarlyBootIo = EarlyBootIo::new();

/// Mint the early-boot I/O token: `kernel_main`, once, before any storage
/// driver issues a command (design §6).
pub fn mint_early_boot_io() {
    let minted = EARLY_BOOT_IO.mint();
    debug_assert!(minted, "the early-boot I/O token is minted once");
}

/// Whether the early-boot I/O token is held: the boot thread polls its
/// commands.
pub fn early_boot_io_held() -> bool {
    EARLY_BOOT_IO.held()
}

/// The block I/O caller rule for the current context (design §6): a context
/// that may park, or early boot with the token held. A refusal writes a
/// receipt at the caller and returns `NotSleepable`.
#[track_caller]
pub fn io_context() -> Result<IoContext, ParkError> {
    park_core::io_context(&HOOKS, site(), &EARLY_BOOT_IO)
}

/// A refusal decided through a seam of the caller's own (AHCI admission):
/// the receipt `io_context` would have written.
#[track_caller]
pub fn record_io_refusal(context: ParkContext) {
    record(context, site());
}

/// The attached block devices' completion sources (design §6).
static COMPLETION_SOURCES: CompletionSources<BLOCK_COMPLETION_SOURCES> = CompletionSources::new();

pub use park_core::BlockSource;

const fn source_key(kind: BlockSource, index: u32) -> u64 {
    kind.key(index)
}

fn source_name(key: u64) -> &'static str {
    match key >> 32 {
        1 => "ahci",
        2 => "virtio-blk-pci",
        3 => "virtio-blk-mmio",
        _ => "unknown",
    }
}

/// Block device `index` of `kind` attaches, with its completion source armed
/// (its interrupt routed and unmasked, or its poller started) or not.
pub fn attach_completion_source(kind: BlockSource, index: u32, armed: bool) {
    if !COMPLETION_SOURCES.attach(source_key(kind, index), armed) {
        crate::serial_println!(
            "[park] completion source table full: {} {} is never ready",
            source_name(source_key(kind, index)), index
        );
    }
}

/// Block device `index` of `kind` can complete a sleeping caller's command.
pub fn arm_completion_source(kind: BlockSource, index: u32) {
    COMPLETION_SOURCES.arm(source_key(kind, index));
}

/// Whether a sleeping caller may issue a command to block device `index` of
/// `kind`: its completion source is armed. A device that is not ready
/// refuses the request, and counts the refusal, rather than polling.
pub fn completion_source_ready(kind: BlockSource, index: u32) -> bool {
    COMPLETION_SOURCES.ready(source_key(kind, index))
}

/// The first dispatch (design §6): revoke the early-boot token. Before it,
/// `kernel_main` has started the timer and armed each attached block
/// device's completion source; each one that is not armed gets a receipt
/// naming it, and its requests are refused until it is.
pub fn revoke_early_boot_io() {
    #[cfg(target_arch = "aarch64")]
    if !crate::arch_impl::aarch64::timer_interrupt::timer_is_running() {
        crate::serial_println!("[park] READINESS_RECEIPT the timer is not running at the first dispatch");
    }
    let mut unready = [0u64; BLOCK_COMPLETION_SOURCES];
    match park_core::revoke(&EARLY_BOOT_IO, &COMPLETION_SOURCES, &mut unready) {
        None => crate::serial_println!("[park] READINESS_RECEIPT the early-boot I/O token was not held"),
        Some(count) => {
            for &key in &unready[..count.min(unready.len())] {
                crate::serial_println!(
                    "[park] READINESS_RECEIPT {} {} has no armed completion source at the first dispatch",
                    source_name(key),
                    key & 0xffff_ffff
                );
            }
        }
    }
}

/// Whether the current context may park (design §3.1). The only
/// implementation of that decision.
pub fn context() -> ParkContext {
    park_core::classify(context_inputs())
}

/// The facts [`context`] decides from, read for the current context, for a
/// caller that decides through a seam of its own over `park_core::classify`
/// (AHCI admission, `drivers::ahci::admission`).
pub fn context_inputs() -> ContextInputs {
    let mut inputs = ContextInputs {
        hardirq: crate::per_cpu::in_hardirq(),
        softirq: crate::per_cpu::in_softirq(),
        nmi: crate::per_cpu::in_nmi(),
        ..ContextInputs::default()
    };
    // Interrupt context first: the reads below take scheduler locks, which
    // interrupt context may not.
    if inputs.hardirq || inputs.softirq || inputs.nmi {
        return inputs;
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
    inputs
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

/// A waiter published blocked that may not switch away is about to halt as
/// the current thread (`arch_halt_while_blocked`, design §3.5). That is the
/// same as running idle in the idle thread and before the scheduler
/// dispatches. Anywhere else the caller holds a preemption level the halt
/// does not release, and the halt is a contract violation: a receipt naming
/// the levels held. A caller at its baseline (a braked syscall or a
/// `Braked` kernel thread) is recorded as holding those levels too, because
/// the halt, unlike [`wait`], does not release the baseline.
#[track_caller]
pub(crate) fn halt_as_current() {
    let refused = match context() {
        ParkContext::EarlyBoot | ParkContext::Atomic(AtomicReason::Idle) => return,
        ParkContext::Sleepable => {
            ParkContext::Atomic(AtomicReason::LocksHeld(depth().min(u8::MAX as u32) as u8))
        }
        refused => refused,
    };
    record(refused, site());
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
/// (`park_core::exit_switch`), without acting on it: for the ARM64 exit's
/// lock-free check, which asks only whether the thread must be switched out.
pub(crate) fn exit_switch_for(thread: &super::thread::Thread, need_resched: bool) -> ExitSwitch {
    let seen = exit_thread(thread);
    let exit = park_core::exit_switch(seen, need_resched);
    #[cfg(feature = "boot_tests")]
    exit_probe::saw_exit(thread.id, seen, exit);
    exit
}

/// The interrupt exit's decision for `thread`, the thread it interrupted,
/// made under the lock that owns the thread's scheduler state. A
/// [`ExitSwitch::Requeue`] is applied here: the thread's wait is published
/// but not committed, so it is returned to running and the switch requeues
/// it (design §3.5). Its wait stays linked on its queues and its word stays
/// `Parking`, so a wake still ends the wait; when the wait's switch finds it
/// running, it returns at once and the wait publishes again
/// (`park_core::wait`).
pub(crate) fn apply_exit_switch(thread: &mut super::thread::Thread, need_resched: bool) -> ExitSwitch {
    let exit = exit_switch_for(thread, need_resched);
    if exit == ExitSwitch::Requeue {
        // What ending a wait does to the scheduler state (`end_wait`); the
        // deadline's timer entry is no longer live without `wake_time_ns`.
        thread.blocked_in_syscall = false;
        thread.wake_time_ns = None;
        thread.set_running();
    }
    exit
}

/// The E2.3 kernel test's fixed point (design §8, "E2.3: parking and
/// parked"). An armed thread's next wait, once published and past its
/// signal check, takes an interrupt with a reschedule pending, before its
/// commit; the probe records each exit decision made about that thread while
/// its wait is uncommitted, and whether the thread ran again.
#[cfg(feature = "boot_tests")]
pub(crate) mod exit_probe {
    use super::park_core::{ExitSwitch, ExitThread, Phase, Point};
    use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

    static ARMED: AtomicU64 = AtomicU64::new(0);
    static WATCHED: AtomicU64 = AtomicU64::new(0);
    static EXITS: AtomicU32 = AtomicU32::new(0);
    static SWITCHED_OUT: AtomicU32 = AtomicU32::new(0);
    static RAN_AGAIN: AtomicBool = AtomicBool::new(false);

    /// Arm the probe for thread `tid`'s next wait.
    pub(crate) fn arm(tid: u64) {
        EXITS.store(0, Ordering::SeqCst);
        SWITCHED_OUT.store(0, Ordering::SeqCst);
        RAN_AGAIN.store(false, Ordering::SeqCst);
        WATCHED.store(tid, Ordering::SeqCst);
        ARMED.store(tid, Ordering::SeqCst);
    }

    /// Stop recording exits.
    pub(crate) fn disarm() {
        ARMED.store(0, Ordering::SeqCst);
        WATCHED.store(0, Ordering::SeqCst);
    }

    /// Whether the thread ran again after the forced interrupt.
    pub(crate) fn ran_again() -> bool {
        RAN_AGAIN.load(Ordering::SeqCst)
    }

    /// Exit decisions made about the watched thread while its wait was
    /// published and uncommitted, and how many of them switched it out.
    pub(crate) fn exits() -> (u32, u32) {
        (EXITS.load(Ordering::SeqCst), SWITCHED_OUT.load(Ordering::SeqCst))
    }

    pub(super) fn saw_exit(tid: u64, seen: ExitThread, exit: ExitSwitch) {
        if tid != WATCHED.load(Ordering::Relaxed) || seen != ExitThread::BlockedOnIo(Phase::Parking) {
            return;
        }
        EXITS.fetch_add(1, Ordering::SeqCst);
        if exit == ExitSwitch::SwitchOut {
            SWITCHED_OUT.fetch_add(1, Ordering::SeqCst);
        }
    }

    pub(super) fn at(point: Point) {
        if point != Point::SignalChecked {
            return;
        }
        let Some(tid) = crate::task::scheduler::current_thread_id() else {
            return;
        };
        if ARMED.compare_exchange(tid, 0, Ordering::SeqCst, Ordering::SeqCst).is_err() {
            return;
        }
        force_interrupt_exit();
        RAN_AGAIN.store(true, Ordering::SeqCst);
    }

    /// Take an interrupt now, with a reschedule pending, and return once the
    /// thread runs again: on ARM64 a reschedule SGI to this CPU, whose
    /// handler sets the reschedule; on x86 the reschedule vector, entered by
    /// a software interrupt as `arch_halt_while_blocked` does.
    fn force_interrupt_exit() {
        #[cfg(target_arch = "aarch64")]
        {
            let cpu = crate::arch_impl::aarch64::cpu::cpu_id() as u8;
            crate::arch_impl::aarch64::gic::send_sgi(crate::arch_impl::aarch64::constants::SGI_RESCHEDULE as u8, cpu);
            // The SGI is taken at an instruction boundary shortly after the
            // write; wait, boundedly, for its exit to decide about us.
            let start = crate::arch_read_timestamp();
            while EXITS.load(Ordering::SeqCst) == 0
                && crate::arch_ticks_to_nanos(crate::arch_read_timestamp().wrapping_sub(start)) < 100_000_000
            {
                core::hint::spin_loop();
            }
        }
        #[cfg(target_arch = "x86_64")]
        {
            crate::task::scheduler::set_need_resched();
            // SAFETY: the reschedule vector's gate saves and restores the
            // general-purpose registers, as for `arch_halt_while_blocked`.
            unsafe {
                core::arch::asm!("int {vector}", vector = const crate::interrupts::RESCHEDULE_VECTOR);
            }
        }
    }
}

// -------------------------------------------- sleeps not yet on park::wait

/// What [`release_for_sleep`] released.
#[derive(Clone, Copy)]
#[must_use]
pub(crate) struct SleepRelease(bool);

/// For the sleeps not yet on [`wait`]: release the one level such a sleep
/// releases today, if a level is held (B1 requirement 9). Callers do no
/// arithmetic of their own; E2.4 and E2.10 move them onto [`wait`].
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

/// What [`release_baseline`] released.
#[derive(Clone, Copy)]
#[must_use]
pub(crate) struct BaselineRelease(u32);

/// For a sleep in a sleepable context that is not on [`wait`]
/// (`Completion`): release the levels of the thread's baseline, as [`wait`]
/// does for its switch (§3.2). In a sleepable context the depth is the
/// baseline, so the CPU may switch away afterwards at any baseline: 0 for a
/// kernel thread, 1 for a syscall or a [`Braked`] thread, more for nested
/// guards.
pub(crate) fn release_baseline() -> BaselineRelease {
    let levels = baseline();
    for _ in 0..levels {
        crate::per_cpu::preempt_enable();
    }
    BaselineRelease(levels)
}

/// Take back what [`release_baseline`] released.
pub(crate) fn retake_baseline(released: BaselineRelease) {
    for _ in 0..released.0 {
        crate::per_cpu::preempt_disable();
    }
}

/// For a prepared wait (`syscall::blocking_io::wait_prepared`), which
/// releases one braked level for its sleep and holds it through publication
/// and its signal check: a kernel thread at baseline 0 takes a syscall's
/// shape, one [`Braked`] level, for the wait (design §3.2). A context already
/// at a braked baseline, or one that may not park, is left as it is.
pub fn braked_for_prepared_wait() -> Option<Braked> {
    (baseline() == 0 && context() == ParkContext::Sleepable).then(Braked::new)
}

/// Run `f` with the baseline released when the current context may park,
/// and as it is otherwise: for a bounded spin between device polls that
/// should let the CPU be preempted (the GPU receipt wait), not a sleep.
pub fn with_baseline_released<R>(f: impl FnOnce() -> R) -> R {
    if context() != ParkContext::Sleepable {
        return f();
    }
    let released = release_baseline();
    let result = f();
    retake_baseline(released);
    result
}

/// Back off when no waiter could be published (no memory for the entry):
/// let another thread run here until the next interrupt, then return with
/// the caller's depth. A caller holding more than the one level this
/// releases still keeps its CPU for the halt, which records a receipt at the
/// caller's site (design §3.5).
#[track_caller]
pub(crate) fn retry_pause() {
    crate::task::scheduler::set_need_resched();
    let released = release_for_sleep();
    if !may_switch_now() {
        halt_as_current();
    }
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

    #[cfg(feature = "boot_tests")]
    fn point(&self, point: park_core::Point) {
        exit_probe::at(point);
    }
}

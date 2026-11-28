//! The park contract's core: who may sleep, and how a sleep begins and ends.
//!
//! E2.2 (docs/plans/e2-sleepable-contexts.md §3.1-§3.4, rulings DEV-37(2),
//! DEV-38(4)). Our own design, from the E2 plan, the event queue of E2.1, the
//! memory-ordering chapters of the Intel SDM (Vol. 3A §9.2) and the Arm ARM
//! (DDI 0487, B2), and Lampson and Redell's monitor semantics (CACM 23(2),
//! 1980): a wake is a hint, and the woken thread rechecks its condition.
//!
//! This file has no kernel dependencies. The kernel binds it to the scheduler
//! in `task::park`, and `tools/park-host-tests` drives this same code against
//! a recording scheduler.
//!
//! # The decision
//!
//! [`classify`] is the only implementation of "may the current context park?"
//! (design §3.1). A thread records the preemption depth it may legitimately
//! hold while sleeping, its *baseline* (§3.2): 1 for a syscall that holds the
//! syscall brake, 0 for a kernel thread. Depth above the baseline means the
//! context holds something a sleep would strand, and it may not park.
//!
//! # A wait
//!
//! Each wait of a thread has a sequence number, and the thread's [`WaitWord`]
//! holds that number with the wait's phase and, once it has ended, the reason.
//! A wait ends by exactly one transition of the word from `Parking(seq)` or
//! `Parked(seq)` to `Runnable(seq, reason)`, made by a compare-and-swap, so the
//! first arrival (wake, deadline, signal or kill) wins and later ones leave
//! the word as it is. A waker reaches the word while it holds the queue lock with the
//! waiter's entry still linked, and the entry is unlinked only by a waker or
//! by the waiter's own finish, so the word's current number is the one the
//! entry was made for. A wake the scheduler applies late, after that wait has
//! ended and another has begun, finds the new wait's word unchanged; the
//! waiter sees that its word did not end and goes back to sleep.

use super::event_queue::{EventQueue, Hooks as QueueHooks, PrepareOutcome};
use core::sync::atomic::{fence, AtomicU32, AtomicU64, Ordering};

/// Why a context must not park.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AtomicReason {
    HardIrq,
    SoftIrq,
    Nmi,
    Idle,
    NoThread,
    /// The preemption depth differs from the thread's baseline. The value is
    /// how far it exceeds it; 0 means it is below the baseline.
    LocksHeld(u8),
    /// Interrupts are masked and the thread's baseline does not mask them.
    IrqsMasked,
}

/// What the current context may do (design §3.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParkContext {
    /// May park.
    Sleepable,
    /// Must not park. A park here is a contract violation.
    Atomic(AtomicReason),
    /// The scheduler is not dispatching yet. Only early-boot polling applies.
    EarlyBoot,
}

impl ParkContext {
    /// A compact code for receipts: 0 Sleepable, 1 EarlyBoot, 2.. the
    /// atomic reasons, with `LocksHeld`'s excess in bits 8-15.
    pub const fn code(self) -> u64 {
        match self {
            ParkContext::Sleepable => 0,
            ParkContext::EarlyBoot => 1,
            ParkContext::Atomic(reason) => match reason {
                AtomicReason::HardIrq => 2,
                AtomicReason::SoftIrq => 3,
                AtomicReason::Nmi => 4,
                AtomicReason::Idle => 5,
                AtomicReason::NoThread => 6,
                AtomicReason::LocksHeld(n) => 7 | (n as u64) << 8,
                AtomicReason::IrqsMasked => 8,
            },
        }
    }

    /// The inverse of [`ParkContext::code`].
    pub const fn from_code(code: u64) -> Option<Self> {
        Some(match code & 0xff {
            0 => ParkContext::Sleepable,
            1 => ParkContext::EarlyBoot,
            2 => ParkContext::Atomic(AtomicReason::HardIrq),
            3 => ParkContext::Atomic(AtomicReason::SoftIrq),
            4 => ParkContext::Atomic(AtomicReason::Nmi),
            5 => ParkContext::Atomic(AtomicReason::Idle),
            6 => ParkContext::Atomic(AtomicReason::NoThread),
            7 => ParkContext::Atomic(AtomicReason::LocksHeld((code >> 8) as u8)),
            8 => ParkContext::Atomic(AtomicReason::IrqsMasked),
            _ => return None,
        })
    }
}

/// The facts [`classify`] decides from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ContextInputs {
    /// The scheduler dispatches on this CPU.
    pub dispatching: bool,
    /// A current thread exists, with a wait word.
    pub has_thread: bool,
    /// The current thread is this CPU's idle thread.
    pub idle: bool,
    pub hardirq: bool,
    pub softirq: bool,
    pub nmi: bool,
    /// The CPU's preemption depth (its preempt-disable count).
    pub depth: u32,
    /// The current thread's baseline depth (§3.2).
    pub baseline: u32,
    pub irqs_masked: bool,
    /// The baseline runs with interrupts masked (an x86 syscall until §4.3).
    pub baseline_irqs_masked: bool,
}

/// Whether the context described by `inputs` may park (design §3.1).
pub const fn classify(inputs: ContextInputs) -> ParkContext {
    use AtomicReason::*;
    if inputs.nmi {
        return ParkContext::Atomic(Nmi);
    }
    if inputs.hardirq {
        return ParkContext::Atomic(HardIrq);
    }
    if inputs.softirq {
        return ParkContext::Atomic(SoftIrq);
    }
    if !inputs.dispatching {
        return ParkContext::EarlyBoot;
    }
    if !inputs.has_thread {
        return ParkContext::Atomic(NoThread);
    }
    if inputs.idle {
        return ParkContext::Atomic(Idle);
    }
    if inputs.depth != inputs.baseline {
        let excess = inputs.depth.saturating_sub(inputs.baseline);
        let excess = if excess > u8::MAX as u32 { u8::MAX } else { excess as u8 };
        return ParkContext::Atomic(LocksHeld(excess));
    }
    if inputs.irqs_masked && !inputs.baseline_irqs_masked {
        return ParkContext::Atomic(IrqsMasked);
    }
    ParkContext::Sleepable
}

// ------------------------------------------------------------------ receipts

/// One contract violation: where, why, who, and at what depth.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Receipt {
    /// The call site's identity (the kernel uses its `Location`'s address).
    pub site: u64,
    pub context: ParkContext,
    pub tid: u64,
    pub depth: u32,
}

struct ReceiptSlot {
    /// One more than the receipt's index, or 0 while empty or being written.
    stamp: AtomicU64,
    site: AtomicU64,
    tid: AtomicU64,
    /// `ParkContext::code` in bits 0-31, depth in bits 32-63.
    what: AtomicU64,
}

impl ReceiptSlot {
    const fn new() -> Self {
        Self { stamp: AtomicU64::new(0), site: AtomicU64::new(0), tid: AtomicU64::new(0), what: AtomicU64::new(0) }
    }
}

/// A fixed ring of receipts. It does not allocate or block, so it can
/// be written from any context; a reader skips a slot being rewritten.
pub struct ReceiptRing<const N: usize> {
    written: AtomicU64,
    slots: [ReceiptSlot; N],
}

impl<const N: usize> Default for ReceiptRing<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> ReceiptRing<N> {
    pub const fn new() -> Self {
        Self { written: AtomicU64::new(0), slots: [const { ReceiptSlot::new() }; N] }
    }

    pub fn record(&self, receipt: Receipt) {
        let index = self.written.fetch_add(1, Ordering::Relaxed);
        let slot = &self.slots[(index % N as u64) as usize];
        slot.stamp.store(0, Ordering::Release);
        slot.site.store(receipt.site, Ordering::Relaxed);
        slot.tid.store(receipt.tid, Ordering::Relaxed);
        slot.what.store(receipt.context.code() & 0xffff_ffff | (receipt.depth as u64) << 32, Ordering::Relaxed);
        slot.stamp.store(index + 1, Ordering::Release);
    }

    /// How many receipts were ever recorded.
    pub fn written(&self) -> u64 {
        self.written.load(Ordering::Acquire)
    }

    /// Copy the newest receipts into `out`, newest first. Returns the count.
    pub fn latest(&self, out: &mut [Receipt]) -> usize {
        let written = self.written();
        let mut n = 0;
        let mut index = written;
        while n < out.len() && index > 0 && written - index < N as u64 {
            index -= 1;
            let slot = &self.slots[(index % N as u64) as usize];
            if slot.stamp.load(Ordering::Acquire) != index + 1 {
                continue;
            }
            let what = slot.what.load(Ordering::Relaxed);
            let receipt = Receipt {
                site: slot.site.load(Ordering::Relaxed),
                context: match ParkContext::from_code(what & 0xffff_ffff) {
                    Some(context) => context,
                    None => continue,
                },
                tid: slot.tid.load(Ordering::Relaxed),
                depth: (what >> 32) as u32,
            };
            if slot.stamp.load(Ordering::Acquire) == index + 1 {
                out[n] = receipt;
                n += 1;
            }
        }
        n
    }
}

// ------------------------------------------------------------- the wait word

/// How a wait ended, as recorded in its word.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arrival {
    Woken,
    Timeout,
    Interrupted,
    Killed,
}

impl Arrival {
    const fn code(self) -> u64 {
        match self {
            Arrival::Woken => 1,
            Arrival::Timeout => 2,
            Arrival::Interrupted => 3,
            Arrival::Killed => 4,
        }
    }

    const fn from_code(code: u64) -> Option<Self> {
        match code {
            1 => Some(Arrival::Woken),
            2 => Some(Arrival::Timeout),
            3 => Some(Arrival::Interrupted),
            4 => Some(Arrival::Killed),
            _ => None,
        }
    }
}

/// A wait word's phase.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// No wait in progress.
    Idle,
    /// Published, not committed (design §3.5).
    Parking,
    /// Committed: the thread switches out.
    Parked,
    /// Ended, with the first arrival's reason.
    Runnable(Arrival),
}

const PHASE_IDLE: u64 = 0;
const PHASE_PARKING: u64 = 1;
const PHASE_PARKED: u64 = 2;
const PHASE_RUNNABLE: u64 = 3;
const SEQ_SHIFT: u32 = 8;

const fn pack(seq: u64, phase: u64, reason: u64) -> u64 {
    seq << SEQ_SHIFT | phase << 4 | reason
}

const fn seq_of(word: u64) -> u64 {
    word >> SEQ_SHIFT
}

const fn phase_of(word: u64) -> u64 {
    (word >> 4) & 0xf
}

/// One thread's wait state: sequence number, phase and end reason, in one
/// atomic word. The owning thread begins, commits and finishes its waits;
/// anyone may make the one ending transition.
pub struct WaitWord(AtomicU64);

impl Default for WaitWord {
    fn default() -> Self {
        Self::new()
    }
}

impl WaitWord {
    pub const fn new() -> Self {
        Self(AtomicU64::new(0))
    }

    /// The current wait's sequence number and phase.
    pub fn state(&self) -> (u64, Phase) {
        let word = self.0.load(Ordering::Acquire);
        let phase = match phase_of(word) {
            PHASE_PARKING => Phase::Parking,
            PHASE_PARKED => Phase::Parked,
            PHASE_RUNNABLE => match Arrival::from_code(word & 0xf) {
                Some(arrival) => Phase::Runnable(arrival),
                None => Phase::Idle,
            },
            _ => Phase::Idle,
        };
        (seq_of(word), phase)
    }

    /// Owner: begin a new wait in `Parking` and return its sequence number.
    pub fn begin(&self) -> u64 {
        let seq = (seq_of(self.0.load(Ordering::Relaxed)) + 1) & (u64::MAX >> SEQ_SHIFT);
        self.0.store(pack(seq, PHASE_PARKING, 0), Ordering::SeqCst);
        seq
    }

    /// End wait `seq` with `arrival`, if it is still `Parking` or `Parked`.
    /// True when this call made the transition.
    pub fn arrive(&self, seq: u64, arrival: Arrival) -> bool {
        self.0
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |word| {
                let waiting = matches!(phase_of(word), PHASE_PARKING | PHASE_PARKED);
                (seq_of(word) == seq && waiting).then_some(pack(seq, PHASE_RUNNABLE, arrival.code()))
            })
            .is_ok()
    }

    /// End whichever wait is current with `arrival`, if it is `Parking` or
    /// `Parked`. For wakers that hold the queue lock with the waiter linked.
    pub fn arrive_current(&self, arrival: Arrival) -> bool {
        self.0
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |word| {
                matches!(phase_of(word), PHASE_PARKING | PHASE_PARKED)
                    .then_some(pack(seq_of(word), PHASE_RUNNABLE, arrival.code()))
            })
            .is_ok()
    }

    /// Owner: commit wait `seq`, `Parking` to `Parked`. Fails with the end
    /// reason when an arrival came first.
    pub fn commit(&self, seq: u64) -> Result<(), Arrival> {
        match self.0.compare_exchange(
            pack(seq, PHASE_PARKING, 0),
            pack(seq, PHASE_PARKED, 0),
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => Ok(()),
            Err(word) => Err(self.ended_from(seq, word)),
        }
    }

    /// Owner, after its switch returned: the end reason if wait `seq` ended.
    /// If it did not (a late or spurious wake), the wait goes back to
    /// `Parking` so it can commit again.
    pub fn after_switch(&self, seq: u64) -> Option<Arrival> {
        match self.0.compare_exchange(
            pack(seq, PHASE_PARKED, 0),
            pack(seq, PHASE_PARKING, 0),
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => None,
            Err(word) => Some(self.ended_from(seq, word)),
        }
    }

    /// The end reason of wait `seq`, if it has ended.
    pub fn ended(&self, seq: u64) -> Option<Arrival> {
        let word = self.0.load(Ordering::Acquire);
        (seq_of(word) == seq && phase_of(word) == PHASE_RUNNABLE).then(|| self.ended_from(seq, word))
    }

    fn ended_from(&self, seq: u64, word: u64) -> Arrival {
        debug_assert!(seq_of(word) == seq && phase_of(word) == PHASE_RUNNABLE);
        Arrival::from_code(word & 0xf).unwrap_or(Arrival::Woken)
    }

    /// Owner: wait `seq` is over; no later arrival changes the word.
    pub fn finish(&self, seq: u64) {
        self.0.store(pack(seq, PHASE_IDLE, 0), Ordering::SeqCst);
    }
}

// ------------------------------------------------------------------ the wait

/// Which arrivals besides a wake and the deadline end a wait (§3.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaitMode {
    /// Any pending unblocked signal.
    Interruptible,
    /// Only a kill.
    Killable,
    /// Neither.
    Uninterruptible,
}

/// A pending signal the wait's mode admits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignalEnd {
    /// With the errno the caller returns (EINTR or ERESTARTSYS).
    Interrupted(i32),
    Killed,
}

/// How a wait ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaitEnd {
    /// The condition held at registration; the thread did not publish.
    Condition,
    Woken,
    Timeout,
    Interrupted(i32),
    Killed,
    /// No memory for the queue entry; the thread did not publish.
    NoMemory,
}

/// A wait was refused because the context may not park.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParkError {
    NotSleepable(ParkContext),
}

/// The points of a wait where the host model places arrivals.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Point {
    /// Published blocked, before the signal check.
    Published,
    /// After the signal check, before the deadline check.
    SignalChecked,
    /// After the deadline check, before the commit.
    DeadlineChecked,
    /// Committed, before the levels are released for the switch.
    Committed,
    /// Back from the switch, levels retaken, before reading the word.
    Returned,
    /// A wait that did not end published itself again.
    Republished,
}

/// What a wait needs from the scheduler and the CPU, beyond the queue's own
/// hooks.
pub trait ParkHooks: QueueHooks {
    /// The context classification (`classify` of the current inputs).
    fn context(&self) -> ParkContext;
    /// Record a violation by the current thread at `site`.
    fn receipt(&self, context: ParkContext, site: u64);
    /// The current thread's wait word.
    fn word(&self) -> Option<&WaitWord>;
    /// The CPU's preemption depth.
    fn depth(&self) -> u32;
    /// The current thread's baseline.
    fn baseline(&self) -> u32;
    /// Set the current thread's baseline (the `Braked` guard).
    fn set_baseline(&self, baseline: u32);
    /// Take one preemption level.
    fn raise(&self);
    /// Release one preemption level.
    fn lower(&self);
    fn irqs_enabled(&self) -> bool;
    /// Mask local interrupts (`false`) or unmask them (`true`).
    fn set_irqs(&self, enabled: bool);
    /// Monotonic time in nanoseconds.
    fn now_ns(&self) -> u64;
    /// A pending signal that `mode` admits.
    fn pending_signal(&self, mode: WaitMode) -> Option<SignalEnd>;
    /// Publish the current thread `tid` blocked again, with no queue lock
    /// (multi-queue waits and a wait that did not end). False when it may
    /// not sleep (a claimed kill).
    fn republish(&self, tid: u64, deadline_ns: Option<u64>) -> bool;
    /// Switch out while published blocked, with depth 0; return once
    /// runnable again (or spuriously).
    fn sleep(&self);
    /// A test seam: the model places arrivals here.
    fn point(&self, _point: Point) {}
}

/// The preemption and interrupt state a wait entered with. Restored on each
/// end (B1 requirement 9).
struct Entry {
    depth: u32,
    irqs: bool,
    /// Levels released for the switch: the baseline (§3.2).
    released: u32,
}

impl Entry {
    /// The wait publishes at the caller's baseline. E2.2 raised one more
    /// level around a baseline-0 caller's publication, so that an interrupt
    /// exit could not switch the published thread out as blocked before its
    /// commit. Since E2.3 the exit requeues an uncommitted wait instead
    /// ([`exit_switch`], §3.5), and that level is gone.
    fn take<H: ParkHooks>(hooks: &H) -> Self {
        Self { depth: hooks.depth(), irqs: hooks.irqs_enabled(), released: hooks.baseline() }
    }

    fn release_for_sleep<H: ParkHooks>(&self, hooks: &H) {
        for _ in 0..self.released {
            hooks.lower();
        }
    }

    fn retake_after_sleep<H: ParkHooks>(&self, hooks: &H) {
        for _ in 0..self.released {
            hooks.raise();
        }
    }

    fn restore<H: ParkHooks>(&self, hooks: &H) {
        debug_assert_eq!(hooks.depth(), self.depth);
        if hooks.irqs_enabled() != self.irqs {
            hooks.set_irqs(self.irqs);
        }
    }
}

/// Step 0: refuse a context that may not park, with a receipt.
fn admit<H: ParkHooks>(hooks: &H, site: u64) -> Result<(u64, &WaitWord), ParkError> {
    let context = hooks.context();
    if context != ParkContext::Sleepable {
        hooks.receipt(context, site);
        return Err(ParkError::NotSleepable(context));
    }
    match (hooks.current_tid(), hooks.word()) {
        (Some(tid), Some(word)) => Ok((tid, word)),
        _ => {
            let context = ParkContext::Atomic(AtomicReason::NoThread);
            hooks.receipt(context, site);
            Err(ParkError::NotSleepable(context))
        }
    }
}

const fn end_of(arrival: Arrival, errno: i32) -> WaitEnd {
    match arrival {
        Arrival::Woken => WaitEnd::Woken,
        Arrival::Timeout => WaitEnd::Timeout,
        Arrival::Interrupted => WaitEnd::Interrupted(errno),
        Arrival::Killed => WaitEnd::Killed,
    }
}

/// Steps 3 onward for a published wait: check, commit, switch, and repeat
/// until the word records the end.
fn commit_loop<H: ParkHooks>(
    hooks: &H,
    word: &WaitWord,
    tid: u64,
    seq: u64,
    deadline_ns: Option<u64>,
    mode: WaitMode,
    entry: &Entry,
) -> WaitEnd {
    let mut errno = 0;
    loop {
        // Published first, then the signal check (#1230): a signal sent after
        // this check finds the thread blocked and wakes it.
        match hooks.pending_signal(mode) {
            Some(SignalEnd::Interrupted(error)) => {
                if word.arrive(seq, Arrival::Interrupted) {
                    errno = error;
                }
            }
            Some(SignalEnd::Killed) => {
                word.arrive(seq, Arrival::Killed);
            }
            None => {}
        }
        hooks.point(Point::SignalChecked);
        if deadline_ns.is_some_and(|deadline| hooks.now_ns() >= deadline) {
            word.arrive(seq, Arrival::Timeout);
        }
        hooks.point(Point::DeadlineChecked);
        if let Err(arrival) = word.commit(seq) {
            return end_of(arrival, errno);
        }
        hooks.point(Point::Committed);
        entry.release_for_sleep(hooks);
        hooks.sleep();
        entry.retake_after_sleep(hooks);
        hooks.point(Point::Returned);
        if let Some(arrival) = word.after_switch(seq) {
            return end_of(arrival, errno);
        }
        // Runnable without an end of this wait: a wake meant for an earlier
        // wait, or a signal this mode does not admit. Sleep again.
        if !hooks.republish(tid, deadline_ns) {
            // Publication refused: a kill has claimed the thread. An arrival
            // that ended the wait meanwhile keeps its reason.
            word.arrive(seq, Arrival::Killed);
            return end_of(word.ended(seq).unwrap_or(Arrival::Killed), errno);
        }
        hooks.point(Point::Republished);
    }
}

/// Wait on `queue` until `done()` holds or the wait ends (design §3.3).
///
/// `done` is evaluated under the queue lock, atomically with registering and
/// publishing. Returns `Err(NotSleepable)` with a receipt from a context that
/// may not park, and otherwise how the wait ended. A wake is a hint: on
/// `Woken` the caller rechecks its condition. Returns with the caller's entry
/// depth and interrupt state.
pub fn wait<H: ParkHooks>(
    hooks: &H,
    site: u64,
    queue: &EventQueue,
    mut done: impl FnMut() -> bool,
    deadline_ns: Option<u64>,
    mode: WaitMode,
) -> Result<WaitEnd, ParkError> {
    let (tid, word) = admit(hooks, site)?;
    let entry = Entry::take(hooks);
    let seq = word.begin();
    let end = match queue.prepare_checked(hooks, deadline_ns, || !done()) {
        PrepareOutcome::Mismatch => WaitEnd::Condition,
        PrepareOutcome::OutOfMemory => WaitEnd::NoMemory,
        PrepareOutcome::PublishFailed => WaitEnd::Killed,
        PrepareOutcome::Queued => {
            hooks.point(Point::Published);
            commit_loop(hooks, word, tid, seq, deadline_ns, mode, &entry)
        }
    };
    // Step 4: unlink and return to running however the wait ended.
    queue.finish_for(hooks, tid);
    word.finish(seq);
    entry.restore(hooks);
    Ok(end)
}

/// Wait on several queues at once, with one sequence number (design §3.3,
/// "Waiting on several queues"). The thread registers on each queue, then
/// publishes once, then checks `done()`; a wake through any queue ends the
/// wait, and the end unlinks it from each one.
pub fn wait_any<H: ParkHooks>(
    hooks: &H,
    site: u64,
    queues: &[&EventQueue],
    mut done: impl FnMut() -> bool,
    deadline_ns: Option<u64>,
    mode: WaitMode,
) -> Result<WaitEnd, ParkError> {
    let (tid, word) = admit(hooks, site)?;
    let entry = Entry::take(hooks);
    let seq = word.begin();
    let mut added = 0;
    let end = 'end: {
        for queue in queues {
            if !queue.add(hooks, tid) {
                break 'end WaitEnd::NoMemory;
            }
            added += 1;
        }
        if !hooks.republish(tid, deadline_ns) {
            break 'end WaitEnd::Killed;
        }
        hooks.point(Point::Published);
        if done() {
            // A wake that already ended the wait keeps its reason.
            break 'end word.ended(seq).map_or(WaitEnd::Condition, |arrival| end_of(arrival, 0));
        }
        commit_loop(hooks, word, tid, seq, deadline_ns, mode, &entry)
    };
    for queue in &queues[..added] {
        queue.take(hooks, tid);
    }
    hooks.end_wait(tid);
    word.finish(seq);
    entry.restore(hooks);
    Ok(end)
}

// ------------------------------------------------------ the interrupt exit

/// The interrupted thread, as an interrupt exit that may switch sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExitThread {
    /// Running or ready.
    Runnable,
    /// Published blocked on I/O, the state a wait on the event queue
    /// publishes, with the phase of the thread's wait word.
    BlockedOnIo(Phase),
    /// Blocked in another state: on a timer, a signal, a child, or plain.
    Blocked,
    Terminated,
}

/// What an interrupt exit does with the thread it interrupted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExitSwitch {
    /// Return to it.
    Resume,
    /// Switch away and requeue it: it is runnable.
    Preempt,
    /// Switch away and requeue it: it is published blocked, but its wait has
    /// not committed (`Parking`), so it is runnable. The exit returns it to
    /// running first, so the switch requeues it. Its wait word stays
    /// `Parking`: a wake still ends the wait, and a wait that has not ended
    /// publishes again when its switch returns ([`wait`]).
    Requeue,
    /// Switch away and leave it off the run queues until something makes it
    /// runnable.
    SwitchOut,
}

/// The interrupt exit's decision about the interrupted thread, once the
/// exit's depth gate has admitted a switch (depth 0, or a return to user
/// mode). The ARM64 and the x86 exits both decide here (design §3.5).
///
/// Preemption does not commit a sleep (invariant I4): a thread whose wait is
/// published but not committed is runnable, and only its own commit takes it
/// off the run queues. A wait word in any other phase keeps the rule that
/// came before E2.3: `Parked` has committed, `Idle` is a wait not on
/// [`wait`], and an ended word's scheduler wake follows its arrival.
pub const fn exit_switch(thread: ExitThread, need_resched: bool) -> ExitSwitch {
    match thread {
        ExitThread::Runnable if need_resched => ExitSwitch::Preempt,
        ExitThread::BlockedOnIo(Phase::Parking) if need_resched => ExitSwitch::Requeue,
        ExitThread::Runnable | ExitThread::BlockedOnIo(Phase::Parking) => ExitSwitch::Resume,
        ExitThread::BlockedOnIo(_) | ExitThread::Blocked | ExitThread::Terminated => ExitSwitch::SwitchOut,
    }
}

/// Raise the depth and the baseline together by one, for a kernel thread
/// that keeps its CPU except while it sleeps, as a braked syscall does
/// (design §3.2). A park inside releases the level like a syscall's.
pub struct Braked<'a, H: ParkHooks> {
    hooks: &'a H,
}

impl<'a, H: ParkHooks> Braked<'a, H> {
    pub fn new(hooks: &'a H) -> Self {
        hooks.raise();
        hooks.set_baseline(hooks.baseline() + 1);
        Self { hooks }
    }
}

impl<H: ParkHooks> Drop for Braked<'_, H> {
    fn drop(&mut self) {
        self.hooks.set_baseline(self.hooks.baseline().saturating_sub(1));
        self.hooks.lower();
    }
}

// ---------------------------------------------------- lock-skipping wakers

/// The waiter's half of a lock-skipping wake (design §3.4): count itself in
/// `waiters`, then a full barrier, before it loads the condition.
pub fn announce(waiters: &AtomicU32) {
    waiters.fetch_add(1, Ordering::SeqCst);
    fence(Ordering::SeqCst);
}

/// The waiter leaves (`announce`'s inverse).
pub fn withdraw(waiters: &AtomicU32) {
    waiters.fetch_sub(1, Ordering::SeqCst);
}

/// The waker's half: after storing the condition, a full barrier, then
/// whether anyone announced. Store-buffering needs the barrier on both
/// sides, on x86-TSO as on Arm.
pub fn should_wake(waiters: &AtomicU32) -> bool {
    fence(Ordering::SeqCst);
    waiters.load(Ordering::SeqCst) != 0
}

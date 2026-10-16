//! x86_64: the processes whose interval or POSIX timers are armed, expired by
//! any CPU without a scheduler lock (#1649).
//!
//! The scheduler kept them in a list under its lock, and the pass that expired
//! them took the whole scheduler, every run-queue lock included, so a CPU
//! stopped in that pass stopped scheduling everywhere. Each armed group now
//! sits in a slot of a fixed table that holds a reference to it: a timer
//! syscall claims a free slot, any CPU's pass reads every slot, and a group
//! whose timers are all disarmed is dropped from the table, and its reference
//! released once no pass can still be reading it (`rq::Pin`). An expiry is
//! claimed by the timer's own lock, which the pass only tries, so two passes
//! never deliver one expiry twice. The threads a signal may go to are found
//! through the thread index (`rq::group_recipient`), and woken under their
//! owners' locks or held for them (`rq::notify_or_hold`).

use super::rq;
use crate::signal::IntervalTimers;
use crate::task::thread::CpuAccount;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, AtomicU8, Ordering};

const SLOTS: usize = 512;

/// Slot states: free; being filled or emptied; holding an armed group; and
/// holding a group dropped from the table whose reference a pass may still
/// read.
const FREE: u8 = 0;
const CLAIMED: u8 = 1;
const LIVE: u8 = 2;
const RETIRED: u8 = 3;

struct Group {
    state: AtomicU8,
    /// A reference to the group's timers (`Arc::into_raw`), and to the CPU
    /// account they count.
    timers: AtomicPtr<IntervalTimers>,
    account: AtomicPtr<CpuAccount>,
}

static GROUPS: [Group; SLOTS] = [const {
    Group {
        state: AtomicU8::new(FREE),
        timers: AtomicPtr::new(core::ptr::null_mut()),
        account: AtomicPtr::new(core::ptr::null_mut()),
    }
}; SLOTS];

/// Raised by every registration: a pass that computed the next check before
/// one finished asks for another pass rather than publish a check that misses
/// the newly armed timer.
static REGISTRATIONS: AtomicU64 = AtomicU64::new(0);

/// A group the table had no room for went to the scheduler's own list,
/// which the whole scheduler's pass expires (`Scheduler::wake_signal_timers`).
pub(super) static OVERFLOWED: AtomicBool = AtomicBool::new(false);

/// The monotonic time a pass started at, 0 when none runs: one CPU runs the
/// pass at a time, unless the one running it has not finished within
/// `PASS_TAKEOVER_NS`, as when its vCPU stopped mid-pass. A pass is safe to
/// run beside another: each expiry is claimed by its timer's lock.
static PASS_STARTED: AtomicU64 = AtomicU64::new(0);
const PASS_TAKEOVER_NS: u64 = 2_000_000;

/// Register `timers`, which count `account`, after a timer syscall or exec
/// changed them, and ask for a pass at the next tick, which publishes the
/// group's next check. False when the table is full: the caller registers
/// the group with the scheduler instead.
pub(super) fn register(timers: &Arc<IntervalTimers>, account: &Arc<CpuAccount>) -> bool {
    REGISTRATIONS.fetch_add(1, Ordering::AcqRel);
    let added = !timers.is_active() || listed(timers) || claim(timers, account);
    super::SIGNAL_TIMERS_NEXT.store(0, Ordering::Release);
    added
}

fn listed(timers: &Arc<IntervalTimers>) -> bool {
    let address = Arc::as_ptr(timers) as *mut IntervalTimers;
    GROUPS
        .iter()
        .any(|group| group.state.load(Ordering::Acquire) == LIVE && group.timers.load(Ordering::Acquire) == address)
}

/// Put `timers` in a free slot. Two registrations of one group can race to
/// two slots; a pass then visits it twice, which claims nothing twice.
fn claim(timers: &Arc<IntervalTimers>, account: &Arc<CpuAccount>) -> bool {
    for group in GROUPS.iter() {
        if group.state.load(Ordering::Relaxed) != FREE
            || group
                .state
                .compare_exchange(FREE, CLAIMED, Ordering::AcqRel, Ordering::Relaxed)
                .is_err()
        {
            continue;
        }
        group.timers.store(Arc::into_raw(timers.clone()) as *mut IntervalTimers, Ordering::Release);
        group.account.store(Arc::into_raw(account.clone()) as *mut CpuAccount, Ordering::Release);
        group.state.store(LIVE, Ordering::Release);
        return true;
    }
    false
}

/// Release the references of the groups dropped from the table, once no pin
/// is held: no pass can still read them.
fn release_retired() {
    for group in GROUPS.iter() {
        if group.state.load(Ordering::Acquire) != RETIRED || !rq::unpinned() {
            continue;
        }
        if group
            .state
            .compare_exchange(RETIRED, CLAIMED, Ordering::AcqRel, Ordering::Relaxed)
            .is_err()
        {
            continue;
        }
        let timers = group.timers.swap(core::ptr::null_mut(), Ordering::AcqRel);
        let account = group.account.swap(core::ptr::null_mut(), Ordering::AcqRel);
        // SAFETY: the slot held one reference to each, taken by `claim`, and
        // no pass reads them any more.
        unsafe {
            drop(Arc::from_raw(timers));
            drop(Arc::from_raw(account));
        }
        group.state.store(FREE, Ordering::Release);
    }
}

/// Expire every group's due timers and publish the next check
/// (`Scheduler::wake_signal_timers` and `publish_timer_deadline`). No
/// scheduler lock is held; interrupts are masked.
pub(super) fn expire() {
    let now = crate::signal::monotonic_nanos();
    let started = PASS_STARTED.load(Ordering::Acquire);
    let claimed = if started == 0 {
        PASS_STARTED.compare_exchange(0, now.max(1), Ordering::AcqRel, Ordering::Acquire).is_ok()
    } else {
        now.saturating_sub(started) > PASS_TAKEOVER_NS
            && PASS_STARTED
                .compare_exchange(started, now.max(1), Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
    };
    if !claimed {
        return;
    }
    super::SIGNAL_PASS_REQUESTED.store(false, Ordering::Release);
    let registrations = REGISTRATIONS.load(Ordering::Acquire);
    release_retired();
    crate::signal::timers::retry_job_control();
    let wall = crate::signal::monotonic_micros();
    let tick_ns = crate::time::timer::MS_PER_TICK * 1_000_000;
    let mut next = u64::MAX;
    {
        let pin = rq::Pin::new();
        for slot in GROUPS.iter() {
            if slot.state.load(Ordering::Acquire) != LIVE {
                continue;
            }
            // SAFETY: a live slot holds a reference to both, which is released
            // only after the slot is retired and no pin is held.
            let (timers, account) = unsafe {
                (
                    &*slot.timers.load(Ordering::Acquire),
                    &*slot.account.load(Ordering::Acquire),
                )
            };
            let group = timers as *const IntervalTimers as usize;
            // A process whose every thread has exited keeps its row until it
            // is reaped, but its timers stop.
            if !rq::group_has_live_thread(group) {
                timers.disarm_all();
            }
            if !timers.is_active() {
                let _ = slot.state.compare_exchange(LIVE, RETIRED, Ordering::AcqRel, Ordering::Relaxed);
                continue;
            }
            // A timer counting CPU time reads the account its threads are
            // charged to, so the threads running on any CPU are charged first.
            let counts_cpu =
                timers.virtual_timer.is_active() || timers.prof.is_active() || timers.posix.counts_cpu();
            if counts_cpu {
                rq::charge_group(group);
            }
            let (user_ns, system_ns) = account.split_ns();
            if expire_posix(&pin, timers, group, user_ns.saturating_add(system_ns)) {
                wake_posix_recipients(&pin, group);
            }
            let user = user_ns / 1000;
            let system = system_ns / 1000;
            let probe_real_deadline = timers.real.deadline_micros();
            let pending = [
                (timers.real.expire(wall), crate::signal::constants::SIGALRM),
                (timers.virtual_timer.expire_cpu(user), crate::signal::constants::SIGVTALRM),
                (timers.prof.expire_cpu(user.saturating_add(system)), crate::signal::constants::SIGPROF),
            ];
            for (expired, sig) in pending {
                if expired {
                    if sig == crate::signal::constants::SIGALRM {
                        let bit = crate::signal::constants::sig_mask(sig);
                        if let Some(tid) = rq::group_recipient(&pin, group, bit) {
                            crate::stall_probe::signal_due(tid, probe_real_deadline.map(|us| us.saturating_mul(1000)));
                        }
                    }
                    deliver(&pin, group, sig);
                }
            }
            if let Some(real) = timers.real.deadline_micros() {
                next = next.min(real.saturating_mul(1000));
            }
            if timers.virtual_timer.is_active() || timers.prof.is_active() {
                next = next.min(now.saturating_add(tick_ns));
            }
            next = next.min(timers.posix.next_check_ns(&crate::signal::timers::Now::read(0)));
        }
    }
    if OVERFLOWED.load(Ordering::Acquire) {
        next = next.min(now.saturating_add(tick_ns));
    }
    super::SIGNAL_TIMERS_NEXT.store(next, Ordering::Release);
    if REGISTRATIONS.load(Ordering::Acquire) != registrations {
        super::SIGNAL_TIMERS_NEXT.store(0, Ordering::Release);
    }
    let _ = PASS_STARTED.compare_exchange(now.max(1), 0, Ordering::AcqRel, Ordering::Relaxed);
}

/// Send interval-timer signal `sig` to the process whose timers are at
/// `group`: to the first thread that does not block it or waits for it, else
/// to the first, and wake it when it accepts the signal.
fn deliver(pin: &rq::Pin, group: usize, sig: u32) {
    let bit = crate::signal::constants::sig_mask(sig);
    let Some(tid) = rq::group_recipient(pin, group, bit) else {
        return;
    };
    let Some(thread) = pin.thread(tid) else {
        return;
    };
    let signals = &thread.signals;
    signals.timer_pending.fetch_or(bit, Ordering::Release);
    let eligible = (!signals.blocked.load(Ordering::Relaxed) | signals.wait_set.load(Ordering::Acquire)) & bit != 0;
    if eligible {
        rq::notify_or_hold(tid, &[rq::Notice::Signal, rq::Notice::ChildExit]);
    }
}

/// `Scheduler::expire_posix_timers`: expire the group's POSIX timers, given
/// its process's CPU time. Each newly generated signal goes to the thread
/// SIGEV_THREAD_ID names, or to a thread of the process that has it unblocked
/// or waits for it, else to any. True when a signal became due.
fn expire_posix<'a>(pin: &'a rq::Pin, timers: &IntervalTimers, group: usize, process_cpu: u64) -> bool {
    if !timers.posix.is_active() {
        return false;
    }
    let now = crate::signal::timers::Now::read(process_cpu);
    timers.posix.expire(&now, |thread, sig| {
        let tid = match thread {
            Some(tid) => {
                let mut member = None;
                rq::each_of_group(pin, group, |candidate, _| {
                    if candidate == tid {
                        member = Some(tid);
                    }
                });
                member
            }
            None => rq::group_recipient(pin, group, crate::signal::constants::sig_mask(sig)),
        }?;
        pin.thread(tid).map(|thread| &*thread.signals)
    })
}

/// `Scheduler::wake_posix_timer_recipients`: wake each thread of the group a
/// POSIX timer signal just became due for that has it unblocked or waits for
/// it.
fn wake_posix_recipients(pin: &rq::Pin, group: usize) {
    rq::each_of_group(pin, group, |tid, signals| {
        let due = signals.posix_pending.load(Ordering::Acquire);
        let accepted = (!signals.blocked.load(Ordering::Relaxed) | signals.wait_set.load(Ordering::Acquire)) & due;
        if accepted != 0 {
            rq::notify_or_hold(tid, &[rq::Notice::Signal, rq::Notice::ChildExit]);
        }
    });
}

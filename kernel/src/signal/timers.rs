//! POSIX per-process timers: timer_create, timer_settime, timer_gettime,
//! timer_getoverrun and timer_delete.
//!
//! A process's timers live beside its interval timers in `IntervalTimers`,
//! which every thread of the process shares and which the scheduler visits on
//! its tick while any timer in it is armed (`Scheduler::wake_signal_timers`).
//! On expiry the scheduler moves the deadline on by the interval. For a timer
//! that notifies by signal it then either marks the timer's signal due for one
//! recipient thread or, while that signal is still pending, counts the
//! expiries as overruns: a timer has at most one signal pending at a time.
//! The recipient's next signal check queues the due signal as an instance that
//! carries SI_TIMER, the timer's ID and its sigev_value, and that holds the
//! timer's `TimerSignal`; taking the instance for delivery reads the overrun
//! count off it and starts the count again from zero.

use super::types::ThreadSignals;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicI32, AtomicU64, AtomicUsize, Ordering};

pub const SIGEV_SIGNAL: i32 = 0;
pub const SIGEV_NONE: i32 = 1;
/// A C library runs the notification function on a thread of its own; to the
/// kernel it is a signal, as SIGEV_SIGNAL.
pub const SIGEV_THREAD: i32 = 2;
pub const SIGEV_THREAD_ID: i32 = 4;

/// si_code of a signal a POSIX timer's expiry sent.
pub const SI_TIMER: i32 = -2;

/// timer_settime's flag: it_value is an absolute time on the timer's clock.
pub const TIMER_ABSTIME: u64 = 1;

const NS_PER_SEC: u64 = 1_000_000_000;

/// Set in `TimerSignal::state` while the timer's signal is due or queued and
/// not yet delivered.
const QUEUED: u64 = 1 << 63;

/// Generation order of due signals, so signals due together are queued in the
/// order their timers expired.
static NEXT_DUE: AtomicU64 = AtomicU64::new(1);

/// What a timer and the signal it has pending share.
pub struct TimerSignal {
    /// The timer's ID, as timer_create returned it.
    pub id: i32,
    /// `QUEUED`, and in the low 32 bits the expiries since the pending signal
    /// was generated beyond the one it stands for.
    state: AtomicU64,
    /// The overrun count of the signal delivered last (timer_getoverrun).
    last_overrun: AtomicI32,
    /// The `ThreadSignals` address of the thread whose next signal check
    /// queues the due signal; 0 while none is due.
    due_for: AtomicUsize,
    /// When it became due, in `NEXT_DUE` order.
    due_seq: AtomicU64,
}

impl TimerSignal {
    fn new(id: i32) -> Self {
        Self {
            id,
            state: AtomicU64::new(0),
            last_overrun: AtomicI32::new(0),
            due_for: AtomicUsize::new(0),
            due_seq: AtomicU64::new(0),
        }
    }

    /// Record `expiries` expiries. Returns true when they generate a signal,
    /// none being pending; otherwise they count as overruns of the one that is.
    fn expire(&self, expiries: u64) -> bool {
        let old = self
            .state
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |state| {
                Some(if state & QUEUED != 0 {
                    QUEUED | (state & 0xffff_ffff).saturating_add(expiries).min(i32::MAX as u64)
                } else {
                    QUEUED | (expiries - 1).min(i32::MAX as u64)
                })
            })
            .unwrap_or(0);
        old & QUEUED == 0
    }

    /// The overrun count the pending signal would carry if delivered now.
    pub fn overrun(&self) -> i32 {
        (self.state.load(Ordering::Acquire) & 0xffff_ffff) as i32
    }

    /// The pending signal is being delivered: its overrun count, which
    /// timer_getoverrun reports from here on. A later expiry generates a new
    /// signal.
    pub fn deliver(&self) -> i32 {
        let overrun = (self.state.swap(0, Ordering::AcqRel) & 0xffff_ffff) as i32;
        self.last_overrun.store(overrun, Ordering::Release);
        overrun
    }

    /// The pending signal was discarded undelivered (ignored, or no thread
    /// could take it): a later expiry generates a new one.
    pub fn discard(&self) {
        self.due_for.store(0, Ordering::Release);
        self.state.store(0, Ordering::Release);
    }
}

/// How a timer tells of its expiry.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Notify {
    /// SIGEV_NONE: it does not; timer_gettime shows it.
    Nothing,
    /// SIGEV_SIGNAL: a signal to the process.
    Process,
    /// SIGEV_THREAD_ID: a signal to this thread of the process.
    Thread(u64),
}

/// The clock a timer's deadline is on.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Base {
    Monotonic,
    Realtime,
    ProcessCpu,
    ThreadCpu,
}

/// The clocks a timer may count, read once for a pass over a process's timers.
pub struct Now {
    pub monotonic: u64,
    pub realtime: u64,
    /// The process's CPU time, user and system.
    pub process_cpu: u64,
}

impl Now {
    /// CLOCK_MONOTONIC and CLOCK_REALTIME now, with the process CPU time given.
    pub fn read(process_cpu: u64) -> Self {
        let (secs, nanos) = crate::time::get_real_time_ns();
        let realtime = (secs.max(0) as u64).saturating_mul(NS_PER_SEC).saturating_add(nanos.max(0) as u64);
        Self { monotonic: super::types::monotonic_nanos(), realtime, process_cpu }
    }
}

struct PosixTimer {
    signal: Arc<TimerSignal>,
    /// The clock timer_create was given.
    clock: i32,
    /// CLOCK_THREAD_CPUTIME_ID: the creating thread, whose CPU time it counts.
    thread_clock: Option<Arc<ThreadSignals>>,
    notify: Notify,
    /// The signal it sends; 0 for SIGEV_NONE.
    signo: u32,
    value: u64,
    base: Base,
    /// The next expiry on `base`, in nanoseconds; 0 while disarmed.
    deadline: u64,
    interval: u64,
}

impl PosixTimer {
    fn now(&self, now: &Now) -> u64 {
        match self.base {
            Base::Monotonic => now.monotonic,
            Base::Realtime => now.realtime,
            Base::ProcessCpu => now.process_cpu,
            Base::ThreadCpu => self.thread_clock.as_ref().map_or(0, |thread| {
                let (user, system) = thread.cpu_split_ns();
                user.saturating_add(system)
            }),
        }
    }

    /// The time left and the interval, in nanoseconds, as timer_gettime
    /// reports them. An expiry the scheduler has not yet processed is
    /// reported as processed.
    fn setting(&self, now: &Now) -> (u64, u64) {
        if self.deadline == 0 {
            return (0, self.interval);
        }
        let current = self.now(now);
        let left = if current < self.deadline {
            self.deadline - current
        } else if self.interval != 0 {
            self.interval - (current - self.deadline) % self.interval
        } else {
            0
        };
        (left, self.interval)
    }
}

#[derive(Default)]
struct Table {
    next_id: i32,
    timers: Vec<PosixTimer>,
}

/// One process's POSIX timers. Syscalls lock the table; the scheduler only
/// ever tries it, and skips a pass it cannot take.
#[derive(Default)]
pub struct PosixTimers {
    table: spin::Mutex<Table>,
    /// Timers with a deadline, so an idle table costs the scheduler one load.
    armed: AtomicUsize,
}

/// The clock IDs timer_create accepts.
pub mod clock {
    pub const REALTIME: i32 = 0;
    pub const MONOTONIC: i32 = 1;
    pub const PROCESS_CPUTIME: i32 = 2;
    pub const THREAD_CPUTIME: i32 = 3;
    pub const BOOTTIME: i32 = 7;
}


impl PosixTimers {
    pub fn is_active(&self) -> bool {
        self.armed.load(Ordering::Acquire) != 0
    }

    fn recount(&self, table: &Table) {
        let armed = table.timers.iter().filter(|t| t.deadline != 0).count();
        self.armed.store(armed, Ordering::Release);
    }

    /// Create a timer on `clock`, telling of its expiry as `notify` with
    /// signal `signo` and `value`; its ID, or EAGAIN when the process already
    /// holds `limit` timers.
    pub fn create(
        &self,
        clock: i32,
        notify: Notify,
        signo: u32,
        value: Option<u64>,
        thread_clock: Option<Arc<ThreadSignals>>,
        limit: usize,
    ) -> Result<i32, u64> {
        let mut table = self.table.lock();
        if table.timers.len() >= limit {
            return Err(crate::syscall::errno::EAGAIN as u64);
        }
        let mut id = table.next_id;
        while table.timers.iter().any(|t| t.signal.id == id) {
            id = id.wrapping_add(1).max(0);
        }
        table.next_id = id.wrapping_add(1).max(0);
        table
            .timers
            .try_reserve(1)
            .map_err(|_| crate::syscall::errno::EAGAIN as u64)?;
        table.timers.push(PosixTimer {
            signal: Arc::new(TimerSignal::new(id)),
            clock,
            thread_clock,
            notify,
            signo: if notify == Notify::Nothing { 0 } else { signo },
            // With no sigevent, sigev_value is the timer's ID.
            value: value.unwrap_or(id as u32 as u64),
            base: Base::Monotonic,
            deadline: 0,
            interval: 0,
        });
        Ok(id)
    }

    /// Arm timer `id` to expire after `value` nanoseconds, or at `value` on
    /// its clock with TIMER_ABSTIME, and every `interval` after; a zero
    /// `value` disarms it. Returns the previous time left and interval.
    pub fn settime(&self, id: i32, flags: u64, interval: u64, value: u64, now: &Now) -> Result<(u64, u64), u64> {
        let mut table = self.table.lock();
        let timer = table
            .timers
            .iter_mut()
            .find(|t| t.signal.id == id)
            .ok_or(crate::syscall::errno::EINVAL as u64)?;
        let old = timer.setting(now);
        let absolute = flags & TIMER_ABSTIME != 0;
        // A relative CLOCK_REALTIME timer measures an interval, so setting
        // the clock does not move it: it runs on the monotonic clock, as on
        // Linux. An absolute one stays on CLOCK_REALTIME.
        timer.base = match timer.clock {
            clock::REALTIME if absolute => Base::Realtime,
            clock::PROCESS_CPUTIME => Base::ProcessCpu,
            clock::THREAD_CPUTIME => Base::ThreadCpu,
            _ => Base::Monotonic,
        };
        timer.interval = if value == 0 { 0 } else { interval };
        timer.deadline = if value == 0 {
            0
        } else if absolute {
            value
        } else {
            timer.now(now).saturating_add(value)
        };
        self.recount(&table);
        Ok(old)
    }

    /// Timer `id`'s time left and interval.
    pub fn gettime(&self, id: i32, now: &Now) -> Result<(u64, u64), u64> {
        let table = self.table.lock();
        let timer = table
            .timers
            .iter()
            .find(|t| t.signal.id == id)
            .ok_or(crate::syscall::errno::EINVAL as u64)?;
        Ok(timer.setting(now))
    }

    /// The overrun count of timer `id`'s last delivered signal.
    pub fn getoverrun(&self, id: i32) -> Result<i32, u64> {
        let table = self.table.lock();
        table
            .timers
            .iter()
            .find(|t| t.signal.id == id)
            .map(|t| t.signal.last_overrun.load(Ordering::Acquire))
            .ok_or(crate::syscall::errno::EINVAL as u64)
    }

    /// Delete timer `id`. A signal of it still pending is delivered.
    pub fn delete(&self, id: i32) -> Result<(), u64> {
        let mut table = self.table.lock();
        let at = table
            .timers
            .iter()
            .position(|t| t.signal.id == id)
            .ok_or(crate::syscall::errno::EINVAL as u64)?;
        let timer = table.timers.remove(at);
        timer.signal.due_for.store(0, Ordering::Release);
        self.recount(&table);
        Ok(())
    }

    /// Delete every timer: exec.
    pub fn clear(&self) {
        let mut table = self.table.lock();
        for timer in table.timers.iter() {
            timer.signal.discard();
        }
        table.timers = Vec::new();
        table.next_id = 0;
        self.recount(&table);
    }

    /// Whether any timer counts the process's CPU time, so the scheduler
    /// charges running threads before a pass.
    pub fn counts_cpu(&self) -> bool {
        self.is_active()
            && self.table.try_lock().is_some_and(|table| {
                table.timers.iter().any(|t| t.deadline != 0 && matches!(t.base, Base::ProcessCpu | Base::ThreadCpu))
            })
    }

    /// The scheduler's pass: expire every timer whose deadline `now` has
    /// reached. `pick` chooses the thread a newly generated signal goes to,
    /// given the thread SIGEV_THREAD_ID names and the signal. Returns true
    /// when a signal became due, so the scheduler wakes recipients that wait
    /// for it. Skips the pass when a syscall holds the table.
    pub fn expire<'a>(&self, now: &Now, mut pick: impl FnMut(Option<u64>, u32) -> Option<&'a ThreadSignals>) -> bool {
        if !self.is_active() {
            return false;
        }
        let Some(mut table) = self.table.try_lock() else {
            return false;
        };
        let mut due = false;
        for timer in table.timers.iter_mut() {
            if timer.deadline == 0 {
                continue;
            }
            let current = timer.now(now);
            if current < timer.deadline {
                continue;
            }
            let expiries = if timer.interval == 0 {
                timer.deadline = 0;
                1
            } else {
                let periods = (current - timer.deadline) / timer.interval + 1;
                timer.deadline = timer.deadline.saturating_add(periods.saturating_mul(timer.interval));
                periods
            };
            if timer.signo == 0 {
                continue;
            }
            let thread = match timer.notify {
                Notify::Thread(tid) => Some(tid),
                _ => None,
            };
            if !timer.signal.expire(expiries) {
                continue;
            }
            match pick(thread, timer.signo) {
                Some(recipient) => {
                    timer.signal.due_seq.store(NEXT_DUE.fetch_add(1, Ordering::Relaxed), Ordering::Relaxed);
                    timer.signal.due_for.store(recipient as *const ThreadSignals as usize, Ordering::Release);
                    recipient
                        .posix_pending
                        .fetch_or(super::constants::sig_mask(timer.signo), Ordering::Release);
                    due = true;
                }
                None => timer.signal.discard(),
            }
        }
        self.recount(&table);
        due
    }

    /// The signals of this process's timers due for the thread whose signal
    /// state is `signals`, oldest first, each as (signal, siginfo, whether
    /// process-directed, timer). Their due marks are cleared.
    pub fn take_due(&self, signals: &ThreadSignals) -> Vec<(u32, super::types::SigInfo, bool, Arc<TimerSignal>)> {
        let me = signals as *const ThreadSignals as usize;
        let table = self.table.lock();
        let mut due: Vec<_> = table
            .timers
            .iter()
            .filter(|t| t.signal.due_for.load(Ordering::Acquire) == me)
            .map(|t| {
                t.signal.due_for.store(0, Ordering::Release);
                let info = super::types::SigInfo::timer(t.signal.id, t.value);
                let process = !matches!(t.notify, Notify::Thread(_));
                (t.signal.due_seq.load(Ordering::Relaxed), t.signo, info, process, t.signal.clone())
            })
            .collect();
        drop(table);
        due.sort_unstable_by_key(|entry| entry.0);
        due.into_iter().map(|(_, sig, info, process, timer)| (sig, info, process, timer)).collect()
    }
}

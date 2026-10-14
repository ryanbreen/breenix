use core::sync::atomic::{AtomicI32, AtomicU64, AtomicUsize, Ordering};
use super::timers::{JOB, JOB_DUE, NEXT_DUE, NOT_SHOWN, QUEUED, REBIND};

/// What a timer and the signal it has pending share.
pub struct TimerSignal {
    /// The timer's ID, as timer_create returned it.
    pub id: i32,
    /// `QUEUED`, and in the low 32 bits the expiries since the pending signal
    /// was generated beyond the one it stands for.
    state: AtomicU64,
    /// The overrun count of the signal delivered last (timer_getoverrun).
    pub(super) last_overrun: AtomicI32,
    /// The `ThreadSignals` address of the thread whose next signal check
    /// queues the due signal, `REBIND` or `JOB`; 0 while none is due.
    pub(super) due_for: AtomicUsize,
    /// The pass that made it due, in `NEXT_DUE` order.
    pub(super) due_seq: AtomicU64,
    /// How long before that pass the timer expired, on its own clock: of
    /// signals one pass made due, the one with the most is the earliest.
    pub(super) due_late: AtomicU64,
    /// The overrun count the pending signal's siginfo was last read with
    /// (`overrun`), which delivery then reports, or `NOT_SHOWN`.
    shown: AtomicU64,
}

impl TimerSignal {
    pub(super) fn new(id: i32) -> Self {
        Self {
            id,
            state: AtomicU64::new(0),
            last_overrun: AtomicI32::new(0),
            due_for: AtomicUsize::new(0),
            due_seq: AtomicU64::new(0),
            due_late: AtomicU64::new(0),
            shown: AtomicU64::new(NOT_SHOWN),
        }
    }

    /// Where its due signal is queued among the others: by pass, then within
    /// a pass the earliest expiry first.
    pub(super) fn due_order(&self) -> (u64, core::cmp::Reverse<u64>) {
        (self.due_seq.load(Ordering::Relaxed), core::cmp::Reverse(self.due_late.load(Ordering::Relaxed)))
    }

    /// Clear the due mark, accounting for a due job-control signal.
    pub(super) fn clear_due(&self) {
        if self.due_for.swap(0, Ordering::AcqRel) == JOB {
            JOB_DUE.fetch_sub(1, Ordering::AcqRel);
        }
    }

    /// Record `expiries` expiries. Returns true when they generate a signal,
    /// none being pending; otherwise they count as overruns of the one that is.
    pub(super) fn expire(&self, expiries: u64) -> bool {
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

    /// The overrun count the pending signal carries if delivered now, for a
    /// siginfo copied out before the signal is taken. Delivery then reports
    /// this same count, so si_overrun and timer_getoverrun agree even when
    /// the timer expires again in between.
    pub fn overrun(&self) -> i32 {
        let overrun = self.state.load(Ordering::Acquire) & 0xffff_ffff;
        self.shown.store(overrun, Ordering::Release);
        overrun as i32
    }

    /// Consume the expiries represented by the siginfo already copied to the
    /// recipient. Expiries after that snapshot belong to a new notification,
    /// which the scheduler binds on its next pass, even if now disarmed.
    pub fn deliver(&self) -> i32 {
        let shown = self.shown.swap(NOT_SHOWN, Ordering::AcqRel);
        let old = self.state.fetch_update(Ordering::AcqRel, Ordering::Acquire, |state| {
            let counted = state & 0xffff_ffff;
            let remaining = if shown == NOT_SHOWN { 0 } else { counted.saturating_sub(shown) };
            Some(if remaining == 0 { 0 } else { QUEUED | (remaining - 1) })
        }).unwrap_or(0);
        let counted = old & 0xffff_ffff;
        let overrun = if shown == NOT_SHOWN { counted } else { shown.min(counted) } as i32;
        if shown != NOT_SHOWN && counted > shown {
            self.due_seq.store(NEXT_DUE.fetch_add(1, Ordering::Relaxed), Ordering::Relaxed);
            self.due_late.store(0, Ordering::Relaxed);
            self.due_for.store(REBIND, Ordering::Release);
        }
        self.last_overrun.store(overrun, Ordering::Release);
        overrun
    }

    /// The pending signal was discarded undelivered (ignored, or no thread
    /// could take it): a later expiry generates a new one.
    pub fn discard(&self) {
        self.clear_due();
        self.shown.store(NOT_SHOWN, Ordering::Release);
        self.state.store(0, Ordering::Release);
    }
}


//! Feeder progress sampled by a thread other than the feeder itself.
use core::sync::atomic::{AtomicU64, AtomicU8, Ordering};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub(super) enum Phase {
    Waiting,
    RootLookup,
    RootMount,
    RootSuperblock,
    RootInode,
    KcsGet,
    KcsSet,
    KcsReset,
}

impl Phase {
    fn from_byte(value: u8) -> Self {
        match value {
            n if n == Self::RootLookup as u8 => Self::RootLookup,
            n if n == Self::RootMount as u8 => Self::RootMount,
            n if n == Self::RootSuperblock as u8 => Self::RootSuperblock,
            n if n == Self::RootInode as u8 => Self::RootInode,
            n if n == Self::KcsGet as u8 => Self::KcsGet,
            n if n == Self::KcsSet as u8 => Self::KcsSet,
            n if n == Self::KcsReset as u8 => Self::KcsReset,
            _ => Self::Waiting,
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(super) struct Overdue {
    pub(super) phase: Phase,
    pub(super) age_ms: u64,
    pub(super) has_fed: bool,
}

pub(super) struct Progress {
    phase: AtomicU8,
    started_ms: AtomicU64,
    last_feed_ms: AtomicU64,
    last_report_ms: AtomicU64,
}

impl Progress {
    pub(super) const fn new() -> Self {
        Self {
            phase: AtomicU8::new(Phase::Waiting as u8),
            started_ms: AtomicU64::new(0),
            last_feed_ms: AtomicU64::new(0),
            last_report_ms: AtomicU64::new(0),
        }
    }

    pub(super) fn start(&self, now_ms: u64) {
        self.started_ms.store(now_ms, Ordering::Release);
    }

    pub(super) fn enter(&self, phase: Phase) {
        self.phase.store(phase as u8, Ordering::Release);
    }

    pub(super) fn fed(&self, now_ms: u64) {
        self.last_feed_ms.store(now_ms, Ordering::Release);
    }

    pub(super) fn overdue(&self, now_ms: u64) -> Option<Overdue> {
        // Report two missed 20 s rounds while there is still time to
        // capture the blockage before the 120 s hardware deadline.
        let fed = self.last_feed_ms.load(Ordering::Acquire);
        let since = if fed == 0 {
            self.started_ms.load(Ordering::Acquire)
        } else {
            fed
        };
        let last_report = self.last_report_ms.load(Ordering::Relaxed);
        let age_ms = now_ms.saturating_sub(since);
        if since == 0 || age_ms < 40_000 || now_ms.saturating_sub(last_report) < 20_000 {
            return None;
        }
        let phase = Phase::from_byte(self.phase.load(Ordering::Acquire));
        if self.last_feed_ms.load(Ordering::Acquire) != fed {
            return None;
        }
        self.last_report_ms.store(now_ms, Ordering::Relaxed);
        Some(Overdue {
            phase,
            age_ms,
            has_fed: fed != 0,
        })
    }
}

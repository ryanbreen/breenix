//! Atomic state for the console input idle clock.
use core::sync::atomic::{AtomicU64, Ordering};

const EXPIRED: u64 = u64::MAX;

pub(crate) struct InputClock {
    last: AtomicU64,
    interval: AtomicU64,
}

impl InputClock {
    pub const fn new() -> Self {
        Self { last: AtomicU64::new(0), interval: AtomicU64::new(0) }
    }

    pub fn arm(&self, now_ms: u64, minutes: u16) {
        self.last.store(now_ms, Ordering::Release);
        self.interval.store(u64::from(minutes) * 60_000, Ordering::Release);
    }

    pub fn minutes(&self) -> u64 {
        self.interval.load(Ordering::Acquire) / 60_000
    }

    pub fn input(&self, now_ms: u64) {
        // Monotonic across CPUs; once expiry wins, later input cannot undo it.
        self.last.fetch_max(now_ms, Ordering::AcqRel);
    }

    pub fn expire(&self, now_ms: u64) -> bool {
        let interval = self.interval.load(Ordering::Acquire);
        let last = self.last.load(Ordering::Acquire);
        interval != 0 && last != EXPIRED && now_ms.saturating_sub(last) >= interval
            && self.last.compare_exchange(last, EXPIRED, Ordering::AcqRel, Ordering::Acquire).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_defers_expiry_and_expiry_is_one_shot() {
        let clock = InputClock::new();
        assert!(!clock.expire(10_000_000));
        clock.arm(1000, 2);
        assert!(!clock.expire(120_999));
        clock.input(120_000);
        clock.input(119_000); // An older observation on another CPU cannot rewind it.
        assert!(!clock.expire(239_999));
        assert!(clock.expire(240_000));
        clock.input(300_000);
        assert!(!clock.expire(500_000));
    }

    #[test]
    fn default_idle_is_thirty_minutes() {
        let clock = InputClock::new();
        clock.arm(0, 30);
        assert_eq!(clock.minutes(), 30);
        assert!(!clock.expire(1_799_999));
        assert!(clock.expire(1_800_000));
    }
}

//! Diagnostic-branch per-open counters. Callers retain a slot until finish.
use core::sync::atomic::{AtomicU64, Ordering};

pub const METRICS: usize = 10;
#[derive(Clone, Copy)]
pub enum Metric {
    Namei,
    ReadGuard,
    WriteGuard,
    Create,
    Vfs,
    Mmio,
    Reclaim,
    Requests,
    Expired,
    ExpiredDone,
}

pub struct Counters {
    owner: AtomicU64,
    values: [AtomicU64; METRICS],
}
impl Counters {
    pub const fn new() -> Self {
        Self { owner: AtomicU64::new(0), values: [const { AtomicU64::new(0) }; METRICS] }
    }
    pub fn begin(&self, tid: u64) -> bool {
        if tid == 0 || self.owner.compare_exchange(0, tid, Ordering::Acquire, Ordering::Relaxed).is_err() {
            return false;
        }
        for value in &self.values { value.store(0, Ordering::Relaxed); }
        true
    }
    pub fn add(&self, tid: u64, metric: Metric, value: u64) {
        if tid != 0 && self.owner.load(Ordering::Relaxed) == tid {
            self.values[metric as usize].fetch_add(value, Ordering::Relaxed);
        }
    }
    pub fn finish(&self, tid: u64) -> Option<[u64; METRICS]> {
        if tid == 0 || self.owner.load(Ordering::Relaxed) != tid { return None; }
        let values = core::array::from_fn(|i| self.values[i].load(Ordering::Relaxed));
        self.owner.store(0, Ordering::Release);
        Some(values)
    }
}

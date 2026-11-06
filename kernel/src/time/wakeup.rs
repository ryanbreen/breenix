//! Shared finite fine wake requests, backed by LAPIC or ARM absolute-compare drivers.
use core::sync::atomic::{AtomicU64, Ordering};
use crate::arch_impl::TimerOps;
pub const POLL_NS: u64 = 250_000;
static REQUESTS: AtomicU64 = AtomicU64::new(0);
static IRQS: AtomicU64 = AtomicU64::new(0);
static LATE_MAX: AtomicU64 = AtomicU64::new(0);
pub fn arm(deadline: u64) -> bool {
    if !crate::arch_impl::current::Timer::request_wakeup(deadline) { return false; }
    REQUESTS.fetch_add(1, Ordering::Relaxed);
    true
}
pub fn record_wake(deadline: u64, now: u64) {
    IRQS.fetch_add(1, Ordering::Relaxed);
    LATE_MAX.fetch_max(now.saturating_sub(deadline), Ordering::Relaxed);
}
pub fn stats() -> [u64; 3] {
    [REQUESTS.load(Ordering::Relaxed), IRQS.load(Ordering::Relaxed), LATE_MAX.load(Ordering::Relaxed)]
}

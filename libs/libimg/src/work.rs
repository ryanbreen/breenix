//! Always-on image work accounting (contract §7).
use core::sync::atomic::{AtomicU64, Ordering};
static STEPS: AtomicU64 = AtomicU64::new(0);
pub fn reset() {
    STEPS.store(0, Ordering::Relaxed);
}
pub fn steps() -> u64 {
    STEPS.load(Ordering::Relaxed)
}
pub fn step() {
    STEPS.fetch_add(1, Ordering::Relaxed);
}

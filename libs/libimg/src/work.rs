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

/// Charge a fixed-length pass whose byte loop has no early return.
pub(crate) fn add(steps: usize) {
    STEPS.fetch_add(steps as u64, Ordering::Relaxed);
}

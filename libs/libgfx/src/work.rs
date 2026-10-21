//! The always-on step counter (docs/gui/breengel.md section 7).
//!
//! A step is one step of CPU geometry setup: clipping a primitive and building its
//! instances, one per primitive and one per visible glyph of a text run. Per-pixel work
//! runs on the device and is counted by `COUNTERS` (fragment area and work estimate).

use core::sync::atomic::{AtomicU64, Ordering::Relaxed};

static STEPS: AtomicU64 = AtomicU64::new(0);

/// Set the counter to zero.
pub fn reset() {
    STEPS.store(0, Relaxed);
}

/// Steps taken since the last `reset`.
pub fn steps() -> u64 {
    STEPS.load(Relaxed)
}

/// Count `n` steps.
#[inline]
pub(crate) fn add(n: u64) {
    STEPS.fetch_add(n, Relaxed);
}

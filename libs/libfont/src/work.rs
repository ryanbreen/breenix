//! The always-on step counter (docs/gui/breengel.md section 7).
//!
//! A step is one iteration of any loop in libfont, including iterations that reject or
//! skip, and one call of any recursive function. Loops whose iteration count is known
//! before they run add it in one call; the count is the same.

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

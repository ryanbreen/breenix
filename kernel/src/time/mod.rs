pub mod time;
pub mod timer;
pub mod rtc;

pub use time::Time;
pub use timer::{init, monotonic_clock, time_since_start, timer_interrupt};

use core::sync::atomic::{AtomicU64, Ordering};

static TIMER_TICKS: AtomicU64 = AtomicU64::new(0);

pub fn get_ticks() -> u64 {
    TIMER_TICKS.load(Ordering::Relaxed)
}

pub(crate) fn increment_ticks() {
    TIMER_TICKS.fetch_add(1, Ordering::Relaxed);
}
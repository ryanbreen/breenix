//! Record of a timed futex wait that did not arbitrate to `ETIMEDOUT` (#608).
//!
//! `clonevm_exec_test` asserts strictly that a 50 ms wait on a word nothing
//! ever wakes returns `-ETIMEDOUT`. When that assertion fails the test bails,
//! and until now the serial said only that it failed - not which of the two
//! possible arbitrations produced the wrong answer, nor what the clock and the
//! queue looked like at the moment of the decision. This module emits that
//! record, once per failed timed wait, from the futex syscall path only.
//!
//! It is deliberately lock-free: the raw serial writers below take no lock and
//! allocate nothing, so the record is safe on any path the futex wait can
//! reach. It is also budgeted, so a systemic failure degrades into a counter
//! rather than a serial storm that would itself hide the evidence.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::tracing::output::{raw_serial_dec, raw_serial_newline, raw_serial_str};

/// The grep anchor. Present exactly when a timed futex wait arbitrated to
/// something other than `ETIMEDOUT` after its deadline or without a waker.
pub const MARKER: &str = "FUTEX_TIMED_WAIT_NOT_ETIMEDOUT";

/// Records emitted to serial per boot before the path goes counter-only.
const EMISSION_BUDGET: u64 = 32;

/// Every failed timed wait this boot, emitted or not.
pub static NON_TIMEOUT_ARBITRATIONS: AtomicU64 = AtomicU64::new(0);

/// What the arbitration decided with, at the moment it decided.
pub struct TimedWaitRecord {
    pub thread_id: u64,
    /// True when this thread took itself off the wait queue, i.e. no waker
    /// dequeued it and it came out of the wait on its own.
    pub removed_by_me: bool,
    pub signal_pending: bool,
    /// The absolute monotonic deadline the caller asked for.
    pub user_deadline_ns: u64,
    /// The monotonic clock read that the deadline comparison used.
    pub now_ns: u64,
    /// What `wake_expired_timers` saw when it popped this thread's timer-heap
    /// entry: `Some(true)` still set, `Some(false)` already cleared - a no-op
    /// pop - and `None` if no entry for this wait was ever popped.
    pub timer_pop_wake_time_set: Option<bool>,
    /// The errno this wait is about to return, or 0 for a success return.
    pub errno: u64,
}

/// Emit one record. Called only on the failure arbitration.
pub fn record(record: &TimedWaitRecord) {
    let seen = NON_TIMEOUT_ARBITRATIONS.fetch_add(1, Ordering::Relaxed);
    if seen >= EMISSION_BUDGET {
        return;
    }

    // Kept to one line of short fields: every write here is lock-free, so a
    // shorter line is a smaller window for another writer to interleave.
    raw_serial_str(MARKER);
    raw_serial_str(" tid=");
    raw_serial_dec(record.thread_id);
    raw_serial_str(" removed_by_me=");
    raw_serial_str(bit(record.removed_by_me));
    raw_serial_str(" signal_pending=");
    raw_serial_str(bit(record.signal_pending));
    raw_serial_str(" deadline_ns=");
    raw_serial_dec(record.user_deadline_ns);
    raw_serial_str(" now_ns=");
    raw_serial_dec(record.now_ns);
    raw_serial_str(" timer_pop=");
    raw_serial_str(match record.timer_pop_wake_time_set {
        Some(true) => "wake_time_set",
        Some(false) => "wake_time_cleared",
        None => "never_popped",
    });
    raw_serial_str(" errno=");
    raw_serial_dec(record.errno);
    raw_serial_str(" seen=");
    raw_serial_dec(seen + 1);
    raw_serial_newline();
}

fn bit(value: bool) -> &'static str {
    if value {
        "1"
    } else {
        "0"
    }
}

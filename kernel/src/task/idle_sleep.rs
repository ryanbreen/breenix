//! One refusal, in one place: the CPU idle identity must never enter a
//! blocking primitive.
//!
//! # Why this exists
//!
//! Issue #761. The aarch64 testing-profile loader ran as CPU 0's boot
//! continuation, and the scheduler represents that continuation with CPU 0's
//! idle task. Two independent sleep-eligibility predicates -- `Completion`'s
//! and VirtIO block MMIO's -- accepted it: one asked only whether a thread ID
//! existed, the other only whether the preemption count looked like a syscall
//! bracket. `Completion` then marked the idle task `BlockedOnIO` and handed its
//! continuation to the scheduler. When the scheduler later dispatched that
//! identity, the aarch64 context-switch path deliberately reset it to the
//! canonical `idle_loop_arm64` entry rather than resuming the saved call, so
//! the loader stack was abandoned while it still owned the VirtIO request guard
//! and the ext2 read guard. The device completed the request and the ISR wake
//! landed on the idle TID, which is excluded from ready-queue insertion by
//! design. Completion delivery succeeded; continuation delivery could not.
//!
//! The identity, not the bracket, is what makes that unrecoverable, and each
//! predicate that grew its own approximation of "sleepable" got a different
//! answer. So the rule lives here once and every predicate and every blocking
//! primitive consults it, instead of each re-deriving it from `preempt_count`
//! or "some thread ID exists".
//!
//! # What refusal means
//!
//! Refusal is never a failure by itself: every caller has a non-sleeping path
//! (a bounded spin on the completion token, the ext2 spin fallback, the
//! waitqueue's `PublishFailed`). Refusing costs CPU; accepting costs the
//! continuation.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Times a `*_can_sleep` predicate or a blocking primitive refused the CPU idle
/// identity.
///
/// Gate-failing. A healthy boot never asks the idle task to block, because the
/// work that blocks runs in kernel threads, so this stays zero. Non-zero means
/// some caller is on the idle identity's stack reaching for a blocking
/// primitive -- the #761 shape -- and the refusal has kept it alive rather than
/// making it correct. The first refusal also prints `[IDLE_SLEEP_REFUSED:...]`
/// on serial so a gate script (and a human tailing the log) sees it without
/// having to read a counter out of memory.
pub static IDLE_SLEEP_REFUSED: AtomicU64 = AtomicU64::new(0);

/// Whether the one-shot serial marker has already been emitted this boot.
static IDLE_SLEEP_REFUSED_MARKED: AtomicBool = AtomicBool::new(false);

/// Read the running count of idle-identity refusals. See `IDLE_SLEEP_REFUSED`.
pub fn idle_sleep_refused() -> u64 {
    IDLE_SLEEP_REFUSED.load(Ordering::Relaxed)
}

/// The shared decision. `is_idle` is the caller's own reading of whether the
/// running identity is this CPU's idle task; `true` means "refuse".
///
/// Callers that already hold the scheduler lock read the identity themselves
/// and pass it here; callers that do not use `idle_identity_must_not_sleep()`
/// below, which reads it for them. Either way the counting, the marker and the
/// verdict are this one function's.
///
/// Lock-free and allocation-free: the marker goes out through the raw serial
/// writer, because the blocking primitives call this while holding the
/// scheduler lock with interrupts masked, where `serial_println!` would
/// deadlock against the logger lock.
#[inline]
pub(crate) fn refuse_idle_identity(is_idle: bool) -> bool {
    if !is_idle {
        return false;
    }
    let refusals = IDLE_SLEEP_REFUSED.fetch_add(1, Ordering::Relaxed) + 1;
    if !IDLE_SLEEP_REFUSED_MARKED.swap(true, Ordering::Relaxed) {
        crate::tracing::output::raw_serial_str("[IDLE_SLEEP_REFUSED:first:count=");
        crate::tracing::output::raw_serial_dec(refusals);
        crate::tracing::output::raw_serial_str("]\r\n");
    }
    true
}

/// The shared predicate for callers that do not hold the scheduler lock:
/// `true` means the current identity may not hand its continuation to the
/// scheduler.
///
/// An unknown identity (the scheduler lock was busy, so `is_current_idle_thread`
/// declined to block on it) is refused too, and deliberately not counted: the
/// counter means "the idle task was observed reaching for a blocking
/// primitive", and an unreadable identity is not that observation. Refusing on
/// unknown is the safe direction -- the spin fallback is recoverable, an
/// abandoned continuation is not.
#[inline]
pub(crate) fn idle_identity_must_not_sleep() -> bool {
    match crate::task::scheduler::is_current_idle_thread() {
        Some(true) => refuse_idle_identity(true),
        Some(false) => false,
        None => true,
    }
}

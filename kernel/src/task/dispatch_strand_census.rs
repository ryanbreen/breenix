//! #775 replacement for the serial-record x86 strand census.
//!
//! The old host-side census reconstructed three facts from formatted records
//! written in the context-switch path: whether a TID had ever had a blocked
//! kernel context saved, whether its most recent save/restore event was a
//! restore, and whether it later exited.  Keep exactly those facts in a fixed
//! atomic ledger so the hot path adds no lock, allocation, or formatting.

use core::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};

const LEDGER_CAPACITY: usize = 4096;
const EVER_SAVED: u8 = 1 << 0;
const LAST_EVENT_RESTORED: u8 = 1 << 1;
const EXITED: u8 = 1 << 2;

static LEDGER: [AtomicU8; LEDGER_CAPACITY] = [const { AtomicU8::new(0) }; LEDGER_CAPACITY];
static OVERFLOW_EVENTS: AtomicU64 = AtomicU64::new(0);
static REPORTED: AtomicBool = AtomicBool::new(false);

#[inline(always)]
fn slot(tid: u64) -> Option<&'static AtomicU8> {
    let index = usize::try_from(tid).ok()?;
    LEDGER.get(index)
}

/// Record the event named by the former "Saved kernel context" record.
#[inline(always)]
pub(crate) fn note_save(tid: u64) {
    let Some(state) = slot(tid) else {
        OVERFLOW_EVENTS.fetch_add(1, Ordering::Relaxed);
        return;
    };
    let _ = state.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |old| {
        Some((old | EVER_SAVED) & !LAST_EVENT_RESTORED)
    });
}

/// Record the event named by the former "Restored kernel context" record.
#[inline(always)]
pub(crate) fn note_restore(tid: u64) {
    let Some(state) = slot(tid) else {
        OVERFLOW_EVENTS.fetch_add(1, Ordering::Relaxed);
        return;
    };
    state.fetch_or(LAST_EVENT_RESTORED, Ordering::Relaxed);
}

/// Exclude a thread exactly as the former process-exit serial record did.
#[inline(always)]
pub(crate) fn note_exit(tid: u64) {
    let Some(state) = slot(tid) else {
        OVERFLOW_EVENTS.fetch_add(1, Ordering::Relaxed);
        return;
    };
    state.fetch_or(EXITED, Ordering::Relaxed);
}

/// Emit the host gate's compact source after the userspace test battery ends.
///
/// This is deliberately the only formatted operation in this module.  Its
/// caller is the final-userspace-exit path, outside interrupt/context-switch
/// context and after `note_exit` has recorded that last process.
pub(crate) fn report_once() {
    if REPORTED
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }

    let mut threads_saved_blocked = 0u64;
    let mut stranded = 0u64;
    for state in &LEDGER {
        let state = state.load(Ordering::Relaxed);
        if state & EVER_SAVED == 0 {
            continue;
        }
        threads_saved_blocked += 1;
        if state & (EXITED | LAST_EVENT_RESTORED) == 0 {
            stranded += 1;
        }
    }

    crate::serial_println!(
        "[DISPATCH_STRAND_CENSUS:threads_saved_blocked={}:stranded={}:overflow={}]",
        threads_saved_blocked,
        stranded,
        OVERFLOW_EVENTS.load(Ordering::Relaxed),
    );
}

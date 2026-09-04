//! #775 replacement for the serial-record x86 strand census.
//!
//! The old host-side census reconstructed three facts from formatted records
//! written in the context-switch path: whether a TID had ever had a blocked
//! kernel context saved, whether its most recent save/restore event was a
//! restore, and whether it later exited.  Keep exactly those facts in a fixed
//! atomic ledger so the hot path adds no lock, allocation, or formatting.  The
//! scheduler's idle loop and the loopback pump emit rate-limited snapshots with
//! interrupts enabled, where the COM2 kernel-log lock is legal.

use core::{
    fmt,
    sync::atomic::{AtomicU64, AtomicU8, Ordering},
};

const LEDGER_CAPACITY: usize = 4096;
const STRANDED_TID_CAPACITY: usize = 16;
const HEARTBEAT_INTERVAL_NS: u64 = 1_000_000_000;
const EVER_SAVED: u8 = 1 << 0;
const LAST_EVENT_RESTORED: u8 = 1 << 1;
const EXITED: u8 = 1 << 2;

static LEDGER: [AtomicU8; LEDGER_CAPACITY] = [const { AtomicU8::new(0) }; LEDGER_CAPACITY];
static OVERFLOW_EVENTS: AtomicU64 = AtomicU64::new(0);
static LAST_HEARTBEAT_NS: AtomicU64 = AtomicU64::new(0);
static SNAPSHOT_SEQ: AtomicU64 = AtomicU64::new(0);

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
    let _ = state.fetch_update(Ordering::Release, Ordering::Relaxed, |old| {
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
    state.fetch_or(LAST_EVENT_RESTORED, Ordering::Release);
}

/// Exclude a thread exactly as the former process-exit serial record did.
#[inline(always)]
pub(crate) fn note_exit(tid: u64) {
    let Some(state) = slot(tid) else {
        OVERFLOW_EVENTS.fetch_add(1, Ordering::Relaxed);
        return;
    };
    state.fetch_or(EXITED, Ordering::Release);
}

struct TidList<'a>(&'a [u64]);

impl fmt::Display for TidList<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.is_empty() {
            return formatter.write_str("-");
        }

        for (index, tid) in self.0.iter().enumerate() {
            if index != 0 {
                formatter.write_str(",")?;
            }
            write!(formatter, "{tid}")?;
        }
        Ok(())
    }
}

/// Emit one compact host-gate snapshot on the kernel-log channel (COM2).
///
/// The callers are ordinary thread contexts with interrupts enabled, never
/// interrupt or context-switch paths: `interrupts::context_switch::idle_loop`,
/// `net::loopback_pump::loopback_pump_fn`, and the syscall-context completion
/// site in `syscall::handlers`. Acquire loads pair with the release RMWs in the
/// three recorders, so a snapshot observes published ledger events across CPUs.
/// claim-lint:ok: the three call sites are pinned by
/// tests/dispatch_strand_census_structure.rs.
///
/// The snapshot goes to COM2 because that is the channel the three removed
/// `log::info!`/`log::debug!` dispatch records used, and because COM1 is the
/// interactive user console (kernel/src/serial.rs). Every in-repo consumer is
/// handed the kernel serial capture.
/// claim-lint:ok: the 3 in-repo call sites are enumerated and pinned by
/// tests/dispatch_strand_census_structure.rs.
///
/// Fields, in emission order: `seq` (1-based, unique within a boot, strictly
/// increasing), `tick` (the raw PIT tick counter), `ms` (milliseconds on the
/// monotonic clock the rate limiter reads), `saved`, `stranded`, `tids`,
/// `tid_overflow`, `ledger_overflow`.
/// claim-lint:ok: #775 ruling R125 fixes the permitted emission call sites.
pub(crate) fn report_snapshot() {
    let mut threads_saved_blocked = 0u64;
    let mut stranded = 0u64;
    let mut stranded_tids = [0u64; STRANDED_TID_CAPACITY];
    let mut stranded_tid_count = 0usize;
    for (tid, state) in LEDGER.iter().enumerate() {
        let state = state.load(Ordering::Acquire);
        if state & EVER_SAVED == 0 {
            continue;
        }
        threads_saved_blocked += 1;
        if state & (EXITED | LAST_EVENT_RESTORED) == 0 {
            stranded += 1;
            if stranded_tid_count < stranded_tids.len() {
                stranded_tids[stranded_tid_count] = tid as u64;
                stranded_tid_count += 1;
            }
        }
    }
    let tid_overflow = stranded.saturating_sub(stranded_tid_count as u64);
    let seq = SNAPSHOT_SEQ.fetch_add(1, Ordering::Release) + 1;

    crate::log_serial_println!(
        "[DISPATCH_STRAND_CENSUS:seq={}:tick={}:ms={}:saved={}:stranded={}:tids={}:tid_overflow={}:ledger_overflow={}]",
        seq,
        crate::time::get_ticks(),
        monotonic_now_ns() / 1_000_000,
        threads_saved_blocked,
        stranded,
        TidList(&stranded_tids[..stranded_tid_count]),
        tid_overflow,
        OVERFLOW_EVENTS.load(Ordering::Acquire),
    );
}

fn monotonic_now_ns() -> u64 {
    let (seconds, nanos) = crate::time::get_monotonic_time_ns();
    seconds.saturating_mul(1_000_000_000).saturating_add(nanos)
}

/// Emit at most one census snapshot per second from existing housekeeping.
pub(crate) fn report_heartbeat_if_due() {
    // idle_loop and loopback_pump_fn both call after their halt returns. Keep
    // this check at the emission boundary so serial locking cannot silently
    // move into an interrupts-disabled context.
    if !crate::arch_interrupts_enabled() {
        return;
    }

    let now = monotonic_now_ns();
    let last = LAST_HEARTBEAT_NS.load(Ordering::Acquire);
    if last != 0 && now.saturating_sub(last) < HEARTBEAT_INTERVAL_NS {
        return;
    }
    if LAST_HEARTBEAT_NS
        .compare_exchange(last, now.max(1), Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
    {
        report_snapshot();
    }
}

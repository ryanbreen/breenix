//! #775 replacement for the serial-record x86 strand census.
//!
//! The old host-side census reconstructed three facts from formatted records
//! written in the context-switch path: whether a TID had ever had a blocked
//! kernel context saved, whether its most recent save/restore event was a
//! restore, and whether it later exited.  Keep exactly those facts in a fixed
//! atomic ledger so the hot path adds no lock, allocation, or formatting.  The
//! scheduler's idle loop, the loopback pump and the `kstrandd` census kthread
//! emit rate-limited snapshots with interrupts enabled, where the COM2
//! kernel-log lock is legal.

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

/// The dispatch-path facts that used to be published by a `log::*!` call in
/// `interrupts::context_switch` and now have no other publication at all.
///
/// PR-1 of the critical-path logging drain deletes 16 `log::*!` calls from
/// `kernel/src/interrupts/context_switch.rs`. Six of them stood beside a
/// `trace_dispatch_abandon(DispatchAbandonSite::…)` that already counts the
/// same arm, so deleting them drops no fact. The other ten had no counter of
/// any kind, and each one gets a variant here: one relaxed `fetch_add` on the
/// dispatch path, read back and printed from ordinary thread context by
/// `report_snapshot()` below, which is the emission boundary that already
/// refuses to run with interrupts disabled.
///
/// This is a SIBLING of `tracing::providers::sched::DispatchAbandonSite`, not
/// an extension of it, and deliberately so: four of the ten arms
/// (`SignalPendingBlocked`, `SignalContextBlocked`, `SignalDeliveredBlocked`,
/// `SignalDeliverableUser`) do NOT abandon the dispatch, so folding them into
/// that enum would add them to the `DISPATCH_SWITCH_IDLE_REDIRECT` aggregate
/// whose ten contributing arms that counter's own documentation enumerates.
///
/// The discriminants index `DISPATCH_LOG_FACTS`, and the order here is the
/// order the fields appear in the census line.
/// claim-lint:ok: the 16/6/10 split is the per-site table in
/// docs/planning/green-program/gates/CRITICAL-PATH-DEBT-PR1-2026-09-06.md and
/// is pinned site by site by tests/dispatch_fact_census_structure.rs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(usize)]
pub(crate) enum DispatchLogFact {
    /// `save_current_thread_context_with_guard`: the process was found but
    /// carries no `main_thread`, so no context was saved.
    SaveNoMainThread = 0,
    /// `save_current_thread_context_with_guard`: no process in the manager
    /// owns the thread being saved.
    SaveProcessNotFound = 1,
    /// `save_current_thread_context_with_guard`: the guard was taken but the
    /// manager option it wraps is unset.
    SaveManagerNone = 2,
    /// `switch_to_thread`, blocked-in-syscall arm: the thread has deliverable
    /// signals and a saved userspace context, so delivery runs on that context.
    SignalPendingBlocked = 3,
    /// `switch_to_thread`, blocked-in-syscall arm: the saved userspace context
    /// was installed into the interrupt frame for that delivery.
    SignalContextBlocked = 4,
    /// `switch_to_thread`, blocked-in-syscall arm: delivery returned
    /// `Delivered`.
    SignalDeliveredBlocked = 5,
    /// `setup_idle_return`: the idle thread has no kernel stack, so the
    /// per-CPU fallback was used for the return frame.
    IdleStackMissing = 6,
    /// `setup_kernel_thread_return`: the scheduler has no thread info for the
    /// thread being dispatched, so no context was restored.
    KernelThreadInfoMissing = 7,
    /// `restore_userspace_thread_context`: the userspace thread has no kernel
    /// stack, so `TSS.RSP0` was left as it stood.
    UserKernelStackMissing = 8,
    /// `restore_userspace_thread_context`: the process has deliverable signals
    /// at the point the userspace context has been restored.
    SignalDeliverableUser = 9,
}

/// The number of `DispatchLogFact` variants. A variant added without widening
/// this does not compile (`DISPATCH_LOG_FACTS[fact as usize]` is a constant
/// index into a fixed array, and `FACT_FIELD_NAMES` below is the same length).
pub(crate) const DISPATCH_LOG_FACT_COUNT: usize = 10;

/// One whole-machine relaxed counter per fact. Relaxed and unsynchronised on
/// purpose: these are counts read once a second from thread context, not
/// values another CPU acts on, so the increment is a single `lock xadd` with
/// no ordering constraint and no lock, allocation or formatting -- the 3
/// shapes the dispatch path admits.
/// claim-lint:ok: the 3 admitted shapes are the Tier-2 constraint list in
/// CLAUDE.md, and this function is pinned to a single relaxed `fetch_add` by
/// tests/dispatch_fact_census_structure.rs.
static DISPATCH_LOG_FACTS: [AtomicU64; DISPATCH_LOG_FACT_COUNT] =
    [const { AtomicU64::new(0) }; DISPATCH_LOG_FACT_COUNT];

/// The census-line field name of each fact, in discriminant order.
const FACT_FIELD_NAMES: [&str; DISPATCH_LOG_FACT_COUNT] = [
    "save_no_thread",
    "save_no_proc",
    "save_no_pm",
    "sig_pending_blocked",
    "sig_ctx_blocked",
    "sig_delivered_blocked",
    "idle_no_stack",
    "kthread_no_info",
    "user_no_kstack",
    "sig_deliverable_user",
];

/// Record one occurrence of `fact`. Called from the dispatch path with
/// interrupts disabled and, at several sites, with `PROCESS_MANAGER` held.
#[inline(always)]
pub(crate) fn note_fact(fact: DispatchLogFact) {
    DISPATCH_LOG_FACTS[fact as usize].fetch_add(1, Ordering::Relaxed);
}

/// The ten totals, in the order the census line prints them.
pub(crate) fn fact_counts() -> [u64; DISPATCH_LOG_FACT_COUNT] {
    let mut out = [0u64; DISPATCH_LOG_FACT_COUNT];
    for (index, slot) in DISPATCH_LOG_FACTS.iter().enumerate() {
        out[index] = slot.load(Ordering::Relaxed);
    }
    out
}

/// The ten `name=value` fields, rendered for the census line.
struct FactFields([u64; DISPATCH_LOG_FACT_COUNT]);

impl fmt::Display for FactFields {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (name, value) in FACT_FIELD_NAMES.iter().zip(self.0.iter()) {
            write!(formatter, ":{name}={value}")?;
        }
        Ok(())
    }
}

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
/// `net::loopback_pump::loopback_pump_fn`, `census_thread_fn` in this module
/// (the `kstrandd` kthread), and the syscall-context completion site in
/// `syscall::handlers`. Acquire loads pair with the release RMWs in the three
/// recorders, so a snapshot observes published ledger events across CPUs.
/// claim-lint:ok: the four call sites are pinned by
/// tests/dispatch_strand_census_structure.rs.
///
/// The snapshot goes to COM2 because that is the channel the three removed
/// `log::info!`/`log::debug!` dispatch records used, and because COM1 is the
/// interactive user console (kernel/src/serial.rs). Every in-repo consumer is
/// handed the kernel serial capture.
/// claim-lint:ok: the 4 in-repo call sites are enumerated and pinned by
/// tests/dispatch_strand_census_structure.rs.
///
/// Fields, in emission order: `seq` (1-based, unique within a boot, strictly
/// increasing), `tick` (the raw PIT tick counter), `ms` (milliseconds on the
/// monotonic clock the rate limiter reads), `saved`, `stranded`, `tids`,
/// `tid_overflow`, `ledger_overflow`, then the ten `DispatchLogFact` totals in
/// discriminant order (`FACT_FIELD_NAMES`).
///
/// The ten fact fields are APPENDED, so every consumer that reads the first
/// eight by name keeps working on a capture recorded before this PR;
/// `scripts/x86-strand-census.sh`'s shape check accepts the eight-field and
/// the eighteen-field forms alike, which is what lets the committed #775
/// round-4 captures under
/// `docs/planning/green-program/sockets/serials/775/` still be replayed by
/// tests/x86_gate_verdict_test.rs.
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
        "[DISPATCH_STRAND_CENSUS:seq={}:tick={}:ms={}:saved={}:stranded={}:tids={}:tid_overflow={}:ledger_overflow={}{}]",
        seq,
        crate::time::get_ticks(),
        monotonic_now_ns() / 1_000_000,
        threads_saved_blocked,
        stranded,
        TidList(&stranded_tids[..stranded_tid_count]),
        tid_overflow,
        OVERFLOW_EVENTS.load(Ordering::Acquire),
        FactFields(fact_counts()),
    );
}

fn monotonic_now_ns() -> u64 {
    let (seconds, nanos) = crate::time::get_monotonic_time_ns();
    seconds.saturating_mul(1_000_000_000).saturating_add(nanos)
}

/// Emit one snapshot NOW, bypassing the one-per-second limiter.
///
/// `boot_tests` only, and called only from
/// `test_framework::registry::run_x86_dispatch_fact_oracle`, which needs the
/// two bracketing census lines its verdict is read against to be adjacent in
/// the capture rather than up to a second apart. It shares the emission
/// boundary's contract by construction -- the oracle runs on the boot thread
/// with interrupts enabled -- and it does not disturb the limiter, so the
/// 1 Hz cadence the gate measures is unchanged.
#[cfg(feature = "boot_tests")]
pub(crate) fn force_snapshot() {
    report_snapshot();
}

/// Emit at most one census snapshot per second from existing housekeeping.
pub(crate) fn report_heartbeat_if_due() {
    // idle_loop, loopback_pump_fn and census_thread_fn all call from ordinary
    // thread context after a halt returns. Keep this check at the emission
    // boundary so serial locking cannot silently move into an
    // interrupts-disabled context.
    // claim-lint:ok: the 3 call sites are pinned at 1 each by
    // tests/dispatch_strand_census_structure.rs.
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

/// The `kstrandd` census kthread: sleep, then offer a snapshot to the shared
/// limiter.
///
/// This is the THIRD emission context, and the only one that does not need
/// some other subsystem to act first: `idle_loop` runs only when the CPU has
/// nothing else to dispatch, and `loopback_pump_fn` runs only when loopback
/// traffic wakes it. Round 3's review measured the consequence on the
/// zero-feature production profile: 2 of 6 boots published no post-init
/// snapshot at all, because that profile's single idle dispatch landed inside
/// the shared 1-second limiter's window (finding R3-5).
///
/// The cadence is rate-LIMITED, not periodic, and this thread does not cover a
/// busy boot. `sleep_one_interval()` blocks on the scheduler timer, and a
/// snapshot is published only once this thread is DISPATCHED after that wake --
/// the wake-to-dispatch latency #766 measures (x86 `sleep_until` overrun,
/// p90 2592 ms, max 10318 ms over 324 trials; see
/// docs/planning/green-program/sockets/693-RCA-2026-09-02.md). Under a
/// saturated CPU the hole is longer than that. On the two committed round-4
/// gate captures this thread was alive and publishing at 1 Hz and then
/// emitted no snapshot for 19939 ms (boot1, seq=5 at 4789 ms to seq=6 at
/// 24728 ms) and 17888 ms (boot2, seq=5 at 4840 ms to seq=6 at 22728 ms),
/// across the userspace-process creation burst. The other two contexts did
/// not fill either hole, and the capture is what says so rather than an
/// argument about them: the 3 emitters share ONE limiter, so a hole is an
/// absence of snapshots from the 3 of them together. Neither of the other two
/// published in
/// that window, which is what being demand-driven predicts -- the CPU had
/// runnable work across the burst, so `idle_loop` did not run, and
/// `loopback_pump_fn` runs only on loopback traffic.
/// claim-lint:ok: both holes are re-derivable from the `ms=` fields of
/// docs/planning/green-program/sockets/serials/775/round4/gate-green/
/// boot{1,2}/serial_kernel.txt, and each boot's own gate.txt line 8 prints
/// its largest gap.
///
/// The emission goes through `report_heartbeat_if_due()`, the same rate-limited
/// path the other two contexts call, so adding this thread cannot double the
/// serial volume: the three contexts share one `AtomicU64` compare-exchange.
fn census_thread_fn() {
    loop {
        sleep_one_interval();
        report_heartbeat_if_due();
    }
}

/// Block on the scheduler's timer heap for one heartbeat interval.
///
/// `kloopbackd` next door sleeps on `Scheduler::block_current()`, which has no
/// timer wake at all: it stays blocked until loopback traffic unblocks it. The
/// periodic form of that same block/yield/halt shape is
/// `Scheduler::block_current_for_timer()`, the primitive `sys_nanosleep` and
/// `test_framework::registry::sleep_current_thread_ms` use, and this is that
/// sequence verbatim: publish the block, set `need_resched`, then halt until
/// the timer wake has moved the thread out of `BlockedOnTimer`.
/// claim-lint:ok: the shape is `sleep_current_thread_ms` in
/// kernel/src/test_framework/registry.rs.
///
/// Round 2's dedicated-kthread attempt is why the shape is spelled out here: it
/// drove the scheduler's timer-expiry sweep by hand from thread context inside
/// `with_scheduler` and re-entered the scheduler before ever departing the CPU,
/// and 4 of 4 boots page-faulted. This body does neither: the timer heap is
/// swept by the timer interrupt, as it is for the other sleepers.
/// claim-lint:ok: the 4 boots and the fault signature are recorded in the
/// round-4 notes and in
/// docs/planning/green-program/sockets/775-CENSUS-EQUIVALENCE-2026-09-04.md.
fn sleep_one_interval() {
    use super::thread::ThreadState;

    let Some(tid) = super::scheduler::current_thread_id() else {
        // No scheduler identity: halt once rather than spin, and let the next
        // iteration try again.
        crate::arch_halt_with_interrupts();
        return;
    };
    let wake_at = monotonic_now_ns().saturating_add(HEARTBEAT_INTERVAL_NS);
    super::scheduler::with_scheduler(|scheduler| {
        if scheduler.get_thread(tid).is_some() {
            scheduler.block_current_for_timer(wake_at);
        }
    });
    super::scheduler::yield_current();

    loop {
        crate::arch_halt_with_interrupts();
        let still_blocked = super::scheduler::with_scheduler(|scheduler| {
            scheduler
                .get_thread(tid)
                .is_some_and(|thread| thread.state == ThreadState::BlockedOnTimer)
        })
        .unwrap_or(false);
        if !still_blocked {
            break;
        }
    }
}

/// Start `kstrandd`. Called from `main.rs` immediately after
/// `net::init_loopback_pump()`, on the unconditional init path, so the thread
/// exists in the zero-feature production profile and not only under `testing`.
pub(crate) fn start_census_kthread() {
    let outcome = super::kthread::kthread_run(census_thread_fn, "kstrandd");
    if outcome.is_err() {
        log::error!("kstrandd did not start: the strand census has no timer emitter");
    }
}

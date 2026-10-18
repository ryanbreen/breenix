//! #775 replacement for the serial-record x86 strand census.
//!
//! The old host-side census reconstructed three facts from formatted records
//! written in the context-switch path: whether a TID had ever had a blocked
//! kernel context saved, whether its most recent save/restore event was a
//! restore, and whether it later exited.  Keep exactly those facts in a fixed
//! atomic ledger so the hot path adds no lock, allocation, or formatting.  The
//! loopback pump and the `kstrandd` census kthread queue rate-limited snapshots
//! with interrupts enabled. `kstrandd` drains COM2 one FIFO burst per timer wake,
//! sleeping between bursts so UART time does not occupy a processor. Completion
//! snapshots keep the normal COM2 log path.

use core::{
    fmt,
    sync::atomic::{AtomicU64, AtomicU8, Ordering},
};

const LEDGER_CAPACITY: usize = 4096;
const STRANDED_TID_CAPACITY: usize = 16;
const HEARTBEAT_INTERVAL_NS: u64 = 1_000_000_000;
// A requested deadline, rounded by the 200 Hz x86 timer and dispatch latency.
const DRAIN_RETRY_INTERVAL_NS: u64 = 1_000_000;
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
/// PR-1 of the critical-path logging drain deleted 16 `log::*!` calls from
/// `kernel/src/interrupts/context_switch.rs`. Six of them stood beside a
/// `trace_dispatch_abandon(DispatchAbandonSite::…)` that already counts the
/// same arm, so deleting them dropped no fact. The other ten had no counter
/// of any kind, and each got a variant here: one relaxed `fetch_add` on the
/// dispatch path, read back and printed from ordinary thread context by
/// `report_snapshot()` below, which is the emission boundary that already
/// refuses to run with interrupts disabled. Five of those arms went with the
/// dispatch's process-manager dependency (#1635): the context saves no longer
/// look a thread up in the process manager, and the blocked-in-syscall
/// signal-delivery arm, which no x86 wait armed, is gone.
///
/// This is a SIBLING of `tracing::providers::sched::DispatchAbandonSite`, not
/// an extension of it, and deliberately so: `SignalDeliverableUser` does NOT
/// abandon the dispatch, so folding it into that enum would add it to the
/// `DISPATCH_SWITCH_IDLE_REDIRECT` aggregate whose contributing arms that
/// counter's own documentation enumerates.
///
/// The discriminants index `DISPATCH_LOG_FACTS`, and the order here is the
/// order the fields appear in the census line.
/// claim-lint:ok: the per-site table is pinned site by site by
/// tests/dispatch_fact_census_structure.rs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(usize)]
pub(crate) enum DispatchLogFact {
    /// `save_current_thread_context`: the scheduler has no row for the user
    /// thread being saved, so no context was saved.
    SaveNoThread = 0,
    /// `setup_idle_return`: the idle thread has no kernel stack, so the
    /// per-CPU fallback was used for the return frame.
    IdleStackMissing = 1,
    /// `setup_kernel_thread_return`: the scheduler has no thread info for the
    /// thread being dispatched, so no context was restored.
    KernelThreadInfoMissing = 2,
    /// `restore_userspace_thread_context`: the userspace thread has no kernel
    /// stack, so `TSS.RSP0` was left as it stood.
    UserKernelStackMissing = 3,
    /// `deliver_signals_on_user_return`, for a thread a switch dispatched: the
    /// process has deliverable signals at the point the userspace context has
    /// been restored.
    SignalDeliverableUser = 4,
}

/// The number of `DispatchLogFact` variants. A variant added without widening
/// this does not compile (`DISPATCH_LOG_FACTS[fact as usize]` is a constant
/// index into a fixed array, and `FACT_FIELD_NAMES` below is the same length).
pub(crate) const DISPATCH_LOG_FACT_COUNT: usize = 5;

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
    "idle_no_stack",
    "kthread_no_info",
    "user_no_kstack",
    "sig_deliverable_user",
];

/// Record one occurrence of `fact`. Called from the dispatch path with
/// interrupts disabled and, at one site, with `PROCESS_MANAGER` held.
#[inline(always)]
pub(crate) fn note_fact(fact: DispatchLogFact) {
    DISPATCH_LOG_FACTS[fact as usize].fetch_add(1, Ordering::Relaxed);
}

/// The five totals, in the order the census line prints them.
pub(crate) fn fact_counts() -> [u64; DISPATCH_LOG_FACT_COUNT] {
    let mut out = [0u64; DISPATCH_LOG_FACT_COUNT];
    for (index, slot) in DISPATCH_LOG_FACTS.iter().enumerate() {
        out[index] = slot.load(Ordering::Relaxed);
    }
    out
}

/// The five `name=value` fields, rendered for the census line.
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
/// interrupt or context-switch paths: `net::loopback_pump::loopback_pump_fn`,
/// `census_thread_fn` in this module (the `kstrandd` kthread), and the
/// syscall-context completion site in `syscall::handlers` (plus the explicit
/// boot-test snapshot when that feature is enabled). The idle loop does
/// not call it: see `interrupts::context_switch::idle_loop`. Acquire loads pair
/// with the release RMWs in the three recorders, so a snapshot observes
/// published ledger events across CPUs.
/// claim-lint:ok: the three call sites are pinned by
/// tests/dispatch_strand_census_structure.rs.
///
/// Periodic snapshots only enqueue their formatted record. `kstrandd` drains
/// one FIFO burst per wake and blocks between bursts. Completion and explicit
/// boot-test snapshots use the normal log path, which drains synchronously when
/// the caller may wait; otherwise it leaves delivery to idle housekeeping.
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
/// `tid_overflow`, `ledger_overflow`, then the five `DispatchLogFact` totals in
/// discriminant order (`FACT_FIELD_NAMES`), then `waiting`.
///
/// `stranded` and `waiting` split the threads whose saved kernel context was
/// not restored and that did not exit: `waiting` holds the ones still parked in
/// a wait (`parked_in_wait`), `stranded` and `tids` the rest. Before #1885 they
/// were one count, and a production boot listed its blocked init and getty as
/// stranded for the whole session.
///
/// The fact fields are APPENDED, so every consumer that reads the first
/// eight by name keeps working on a capture recorded before them;
/// `scripts/x86-strand-census.sh`'s shape check accepts the eight-field form
/// and any number of appended fact fields alike, which is what lets the committed #775
/// round-4 captures under
/// `docs/planning/green-program/sockets/serials/775/` still be replayed by
/// tests/x86_gate_verdict_test.rs.
/// claim-lint:ok: #775 ruling R125 fixes the permitted emission call sites.
pub(crate) fn report_snapshot() {
    snapshot(false);
}

/// Whether a thread whose saved kernel context has not been restored is
/// waiting on purpose rather than stranded: blocked with no deadline, or with a
/// deadline less than one census interval behind `now_ns`.
///
/// A thread parked in a blocking syscall stays saved and unrestored for as long
/// as it waits, so the ledger alone lists init in waitpid and getty reading the
/// console for a whole production session (#1885). Everything else the ledger
/// lists stays a strand: a Ready or Running thread whose context was not put
/// back, a terminated thread or one without a scheduler row that left no exit
/// record, and a timed wait whose deadline passed a census interval ago, which
/// the per-tick timer sweep would have ended (a lost timer wake).
fn parked_in_wait(tid: u64, now_ns: u64) -> bool {
    match super::scheduler::thread_wait(tid) {
        Some((state, deadline)) if state.is_blocked() => {
            deadline.is_none_or(|wake_ns| now_ns.saturating_sub(wake_ns) < HEARTBEAT_INTERVAL_NS)
        }
        _ => false,
    }
}

fn snapshot(deferred: bool) {
    let mut threads_saved_blocked = 0u64;
    let mut stranded = 0u64;
    let mut waiting = 0u64;
    let mut stranded_tids = [0u64; STRANDED_TID_CAPACITY];
    let mut stranded_tid_count = 0usize;
    let now_ns = monotonic_now_ns();
    for (tid, slot) in LEDGER.iter().enumerate() {
        let state = slot.load(Ordering::Acquire);
        if state & EVER_SAVED == 0 {
            continue;
        }
        threads_saved_blocked += 1;
        if state & (EXITED | LAST_EVENT_RESTORED) == 0 {
            if parked_in_wait(tid as u64, now_ns) {
                waiting += 1;
                continue;
            }
            // Restored or exited after the first read: the thread ran, and the
            // state just read (Running, or no row) is the run, not a strand.
            if slot.load(Ordering::Acquire) & (EXITED | LAST_EVENT_RESTORED) != 0 {
                continue;
            }
            stranded += 1;
            if stranded_tid_count < stranded_tids.len() {
                stranded_tids[stranded_tid_count] = tid as u64;
                stranded_tid_count += 1;
            }
        }
    }
    let tid_overflow = stranded.saturating_sub(stranded_tid_count as u64);
    let seq = SNAPSHOT_SEQ.fetch_add(1, Ordering::Release) + 1;

    let emit = |args| {
        if deferred {
            crate::serial::enqueue_log(args);
        } else {
            crate::serial::_log_print(args);
        }
    };
    emit(format_args!(
        concat!("[DISPATCH_STRAND_CENSUS:seq={}:tick={}:ms={}:saved={}:stranded={}:tids={}:tid_overflow={}:ledger_overflow={}{}:waiting={}]", "\n"),
        seq,
        crate::time::get_ticks(),
        now_ns / 1_000_000,
        threads_saved_blocked,
        stranded,
        TidList(&stranded_tids[..stranded_tid_count]),
        tid_overflow,
        OVERFLOW_EVENTS.load(Ordering::Acquire),
        FactFields(fact_counts()),
        waiting,
    ));
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
    // loopback_pump_fn and census_thread_fn both call from ordinary thread
    // context after a halt returns. Keep this check at the emission boundary
    // so serial locking cannot silently move into an interrupts-disabled
    // context.
    // claim-lint:ok: the 2 call sites are pinned at 1 each by
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
        snapshot(true);
    }
}

/// The `kstrandd` census kthread offers a snapshot to the shared limiter and
/// writes at most one ready FIFO burst before blocking on the scheduler timer.
/// While COM2 has pending bytes it requests a 1 ms deadline; the 200 Hz x86
/// timer services that in roughly 5–10 ms before dispatch latency. One burst
/// per tick supplies at most 3.2 KB/s with a 16-byte FIFO, or 200 B/s without
/// one, below the 115200-baud UART wire rate. Idle drainers and ordinary log
/// writers also service the ring; this thread does not spin for UART readiness.
///
/// This is the one emission context that does not need some other subsystem
/// to act first: `loopback_pump_fn` runs only when loopback traffic wakes it.
/// The idle loop emitted too until it was found holding an idle CPU, the CPU
/// placement queues woken threads on, with interrupts masked for the ~60 ms
/// the line takes to write (see `interrupts::context_switch::idle_loop`); a
/// CPU running this thread is busy to placement instead. Round 3's review measured the consequence on the
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
/// across the userspace-process creation burst. The two other contexts that
/// emitted then, the idle loop and `loopback_pump_fn`, did not fill either
/// hole, and the capture is what says so rather than an argument about them:
/// the emitters share ONE limiter, so a hole is an absence of snapshots from
/// all of them together. Neither of the other two published in that window,
/// which is what being demand-driven predicts -- the CPU had runnable work
/// across the burst, so the idle loop did not run, and `loopback_pump_fn`
/// runs only on loopback traffic.
/// claim-lint:ok: both holes are re-derivable from the `ms=` fields of
/// docs/planning/green-program/sockets/serials/775/round4/gate-green/
/// boot{1,2}/serial_kernel.txt, and each boot's own gate.txt line 8 prints
/// its largest gap.
///
/// The emission goes through `report_heartbeat_if_due()`, the same rate-limited
/// path `loopback_pump_fn` calls, so the two cannot double the serial volume:
/// they share one `AtomicU64` compare-exchange.
fn census_thread_fn() {
    loop {
        report_heartbeat_if_due();
        let pending = crate::serial::drain_log_once();
        sleep_one_interval(if pending {
            DRAIN_RETRY_INTERVAL_NS
        } else {
            HEARTBEAT_INTERVAL_NS
        });
    }
}

/// Block on the scheduler's timer heap for the heartbeat or UART-drain interval.
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
fn sleep_one_interval(interval_ns: u64) {
    use super::thread::ThreadState;

    let Some(tid) = super::scheduler::current_thread_id() else {
        // No scheduler identity: halt once rather than spin, and let the next
        // iteration try again.
        crate::arch_halt_with_interrupts();
        return;
    };
    let wake_at = monotonic_now_ns().saturating_add(interval_ns);
    super::scheduler::with_current(|scheduler| scheduler.block_current_for_timer(wake_at));
    super::scheduler::yield_current();

    loop {
        crate::arch_halt_with_interrupts();
        if super::scheduler::thread_state(tid) != Some(ThreadState::BlockedOnTimer) {
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

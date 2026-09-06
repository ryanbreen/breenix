//! `BXCAP`: a bounded, lock-free failure-trace capture.
//!
//! This module owns the `BXCAP v1` wire format and the emitter that writes
//! it. It is PR-3 of
//! `docs/planning/green-program/failure-capture/PLAN-2026-09-05.md`, whose
//! section 4 defines the schema and whose section 6 defines this PR's scope.
//!
//! # What a capture is
//!
//! One capture is a bracketed block of one-line, `TOKEN key=value` records
//! written to the raw serial primitive:
//!
//! ```text
//! [BXCAP:BEGIN v=1 seq=0 edge=SELFTEST cpu=0 ts=... tsfreq=... uptime_ms=... arch=aarch64]
//! [BXCAP:EDGE kind=SELFTEST a0=0x0 a1=0x0]
//! [BXCAP:CPU cpu=0 preempt=1 nr=0 hardirq=1 softirq=0 uptime_ms=3000 q=exact]
//! [BXCAP:EV cpu=0 i=0 ts=... type=0x0100 n=TIMER_TICK p=16 f=0x0]
//! [BXCAP:CNT TIMER_TICK_TOTAL=3001]
//! [BXCAP:RING cpu=0 writes=... dropped=0 kept=... span_us=... enabled=1]
//! [BXCAP:THR cpu=0 cur=1 rq=0 threads=14 blocked=6 q=exact]
//! [BXCAP:END v=1 seq=0 edge=SELFTEST verdict=complete records=... bytes=... truncated=0 sections_skipped=0x0]
//! ```
//!
//! `BEGIN` without a matching `END` is the definition of a truncated
//! capture, and that is the distinction a gate needs: it separates "no
//! capture was emitted" from "a capture was emitted and something cut it
//! off". No code in this PR consumes that distinction; PR-5 of the plan
//! does.
//!
//! # The constraints this path holds itself to
//!
//! No lock, no allocation, no formatting machinery, no page-table walk, no
//! I/O beyond the raw serial primitive, and a bounded output size. The one
//! piece of scheduler state a capture wants is read through
//! `try_liveness_snapshot`, which is non-blocking; a refusal is reported and
//! the capture continues. `kernel/src/capture/` is on
//! `scripts/check-critical-path-violations.sh`'s critical-file list with an
//! extra capture-scoped denylist, and `tests/capture_path_lock_free_structure.rs`
//! pins the same shape from the source side.
//!
//! # The re-entrancy latch is one-way, and that is a trade-off
//!
//! `IN_CAPTURE` is set on entry to `emit()` and cleared on the way out. A
//! fault taken INSIDE a capture re-enters `emit()` on the same CPU, and that
//! re-entry emits one `[BXCAP:NOTE reentrant]` line and returns rather than
//! recursing -- but it does NOT clear the latch, and the outer frame is the
//! one place that does. Should the inner fault carry the CPU away instead of
//! returning, that CPU is left latched for the rest of the boot and emits no
//! further capture.
//!
//! That is deliberate for the alternative it rules out -- a latch the inner
//! call cleared would let a fault loop inside a capture recurse until the
//! stack ran out, which is the failure the latch exists to prevent -- and it
//! costs a capture only in a boot that has already taken two faults on one
//! CPU. The one edge this PR wires is the self-test, which fires once per
//! boot, so the latch is set and cleared once. It stops being free when PR-4
//! and PR-7 wire panic, fault and soft-lockup edges, where a first terminal
//! edge that faults mid-capture would silence the second one on that CPU. Whether
//! those edges want a latch that survives the CPU (this one), a depth
//! counter, or a reset at a known-good point is a decision for the PR that
//! wires them; this module states the behaviour rather than leaving it to be
//! discovered.
//!
//! PR-4 wired the panic and fault edges and KEPT the one-way latch, without
//! changing a byte of this file's logic. The reasoning is the trade-off
//! above read in the direction those edges actually take: a terminal edge is
//! already the end of the boot, so the second capture the latch would cost
//! is a capture of a machine that was not going to run again, while the
//! recursion a self-clearing latch would allow is a fault loop inside the
//! emitter that destroys the FIRST capture's bytes as well. A depth counter
//! was considered and not taken: it buys the second capture only in the case
//! where the inner fault RETURNS, which the outer frame already handles by
//! clearing the latch itself. See
//! docs/planning/green-program/failure-capture/PR-4-2026-09-05.md.
//!
//! # What is wired in this PR
//!
//! One edge: `Edge::SelfTest`, behind `--features capture_selftest`, fired
//! once per boot from the timer-tick provider. It exists to give a later PR
//! a deterministic capture to test against instead of waiting for a rare
//! fault. The terminal edges -- panic on both arches, the aarch64 fatal
//! postmortem's section 7, the soft-lockup dump -- are PR-4 and PR-7 and are
//! NOT wired here.

pub mod record;
pub mod sections;

#[cfg(feature = "capture_selftest")]
pub mod selftest;

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use record::{Writer, BXCAP_VERSION};
use sections::{
    section_bit, SECTIONS_ALL, SECTION_CNT, SECTION_CPU, SECTION_EDGE, SECTION_EV, SECTION_RING,
    SECTION_THR,
};

/// Maximum CPUs the re-entrancy latch covers. Matches the trace substrate's
/// `MAX_CPUS`, which is what indexes the ring this capture reads.
const CAPTURE_MAX_CPUS: usize = crate::tracing::MAX_CPUS;

/// Globally monotonic capture sequence number, so two captures in one boot
/// order themselves and a nested one is visible as an interleaved `seq`.
static CAPTURE_SEQ: AtomicU64 = AtomicU64::new(0);

/// Per-CPU re-entrancy latch. A fault taken inside a capture re-enters
/// `emit` on the same CPU; that re-entry emits one `[BXCAP:NOTE reentrant]`
/// line and returns rather than recursing. The outer `emit()` returning is
/// what clears it, so a CPU whose capture does not get that far stays
/// latched for the rest of the boot -- see the module comment for why that
/// is the side to err on here, and for what PR-4 and PR-7 inherit.
static IN_CAPTURE: [AtomicBool; CAPTURE_MAX_CPUS] =
    [const { AtomicBool::new(false) }; CAPTURE_MAX_CPUS];

/// What fired a capture. The `as_str()` value is the `edge=` field, and a
/// scorer greps for it, so these spellings are part of the wire format.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Edge {
    /// The deterministic self-test edge (`--features capture_selftest`).
    SelfTest,
    /// A kernel panic. Wired by PR-4, not here.
    Panic,
    /// A terminal CPU exception. Wired by PR-4, not here.
    Fault,
    /// The soft-lockup detector. Wired by PR-7, not here.
    Lockup,
}

impl Edge {
    pub const fn as_str(self) -> &'static str {
        match self {
            Edge::SelfTest => "SELFTEST",
            Edge::Panic => "PANIC",
            Edge::Fault => "FAULT",
            Edge::Lockup => "LOCKUP",
        }
    }
}

const ARCH_NAME: &str = if cfg!(target_arch = "aarch64") {
    "aarch64"
} else if cfg!(target_arch = "x86_64") {
    "x86_64"
} else {
    "unknown"
};

#[inline(always)]
fn current_cpu_id() -> usize {
    #[cfg(target_arch = "x86_64")]
    {
        use crate::arch_impl::current::percpu::X86PerCpu;
        use crate::arch_impl::PerCpuOps;
        <X86PerCpu as PerCpuOps>::cpu_id() as usize
    }

    #[cfg(target_arch = "aarch64")]
    {
        use crate::arch_impl::current::percpu::Aarch64PerCpu;
        use crate::arch_impl::PerCpuOps;
        // Through the trait on purpose: both arches also carry an inherent
        // `cpu_id`, and going through `PerCpuOps` keeps this one call site
        // reading the same abstraction on both.
        <Aarch64PerCpu as PerCpuOps>::cpu_id() as usize
    }
}

/// Emit one complete capture.
///
/// `arg0`/`arg1` are two opaque edge-supplied words carried on the `EDGE`
/// record; what they mean is the edge's business, not this module's.
///
/// claim-lint:ok: the trigger word is inside the attribute name
/// `#[inline(never)]`, not a claim; the cost claim itself is stated as
/// instruction shape and disclaimed as unmeasured in
/// docs/planning/green-program/failure-capture/PR-3-2026-09-05.md section 8.
/// `#[cold]` and `#[inline(never)]`: a capture is a failure-edge dump, not
/// something a hot path calls, and keeping it out of line keeps its stack
/// frame and its constants out of the callers that merely reference it.
#[cold]
#[inline(never)]
pub fn emit(edge: Edge, arg0: u64, arg1: u64) {
    let cpu_id = current_cpu_id();
    let seq = CAPTURE_SEQ.fetch_add(1, Ordering::Relaxed);

    if cpu_id >= CAPTURE_MAX_CPUS {
        return;
    }
    if IN_CAPTURE[cpu_id].swap(true, Ordering::AcqRel) {
        // Re-entered on this CPU, which means something faulted inside a
        // capture. One line, no recursion, no budget: the outer capture's
        // BEGIN is already on the wire and its missing END will say the
        // rest was lost.
        let mut nested = Writer::new();
        nested.set_budgeted(false);
        if nested.open("NOTE") {
            nested.text(" reentrant seq=");
            nested.dec(seq);
            // Unbudgeted, and the verdict has no reader here: this line
            // either lands whole or the CPU is too far gone to care.
            let _ = nested.close();
        }
        return;
    }

    let mut writer = Writer::new();

    writer.set_budgeted(false);
    if writer.open("BEGIN") {
        writer.kv_dec("v", BXCAP_VERSION);
        writer.kv_dec("seq", seq);
        writer.kv_text("edge", edge.as_str());
        writer.kv_dec("cpu", cpu_id as u64);
        writer.kv_dec("ts", crate::tracing::trace_timestamp());
        writer.kv_dec("tsfreq", crate::tracing::timestamp_frequency_hz());
        writer.kv_dec("uptime_ms", crate::time::timer::get_monotonic_time());
        writer.kv_text("arch", ARCH_NAME);
        // The bracket lines are written with the budget suspended, so their
        // verdict carries no information a reader does not already have from
        // the line itself being on the wire.
        let _ = writer.close();
    }
    writer.set_budgeted(true);

    // A section returns `true` only when its own records went onto the wire
    // whole -- `Writer::close()`'s verdict, propagated. A section the budget
    // cut mid-record returns `false` and keeps its bit in `sections_skipped=`
    // below, so that field does not claim a fragment was a complete section.
    let mut completed: u64 = 0;
    if sections::edge(&mut writer, edge.as_str(), arg0, arg1) {
        completed |= section_bit(SECTION_EDGE);
    }
    if sections::cpu(&mut writer) {
        completed |= section_bit(SECTION_CPU);
    }

    // The one non-blocking scheduler read, taken here rather than at the
    // `THR` section below. See `sections::sample_threads` for why: the rows
    // stay last on the wire, but the lock is asked for before the bulk
    // sections spend milliseconds emitting.
    let sampled_threads = sections::sample_threads(cpu_id);

    if sections::events(&mut writer, cpu_id) {
        completed |= section_bit(SECTION_EV);
    }
    if sections::counters(&mut writer) {
        completed |= section_bit(SECTION_CNT);
    }
    if sections::ring(&mut writer, cpu_id) {
        completed |= section_bit(SECTION_RING);
    }
    if sections::threads(&mut writer, sampled_threads) {
        completed |= section_bit(SECTION_THR);
    }

    let sections_skipped = SECTIONS_ALL & !completed;

    writer.set_budgeted(false);
    writer.close_dangling_record();

    // Read after `close_dangling_record` and before `END` is written, so
    // `records=` and `bytes=` describe exactly the records that precede the
    // `END` line -- `BEGIN` included, `END` itself not. A record the budget
    // cut mid-line was opened but not closed, so it is not counted, which
    // is what lets a reader check `records=` against the well-formed
    // `[BXCAP:...]` lines it can actually parse.
    let truncated = writer.truncated();
    let records = writer.records();
    let bytes = writer.bytes();

    if writer.open("END") {
        writer.kv_dec("v", BXCAP_VERSION);
        writer.kv_dec("seq", seq);
        writer.kv_text("edge", edge.as_str());
        writer.kv_text(
            "verdict",
            if sections_skipped == 0 && !truncated {
                "complete"
            } else {
                "partial"
            },
        );
        writer.kv_dec("records", records);
        writer.kv_dec("bytes", bytes);
        writer.kv_dec("truncated", truncated as u64);
        writer.kv_hex("sections_skipped", sections_skipped);
        let _ = writer.close();
    }

    IN_CAPTURE[cpu_id].store(false, Ordering::Release);
}

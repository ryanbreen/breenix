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
//! capture, and that is the distinction a gate needs: it separates "the
//! kernel never captured" from "the kernel captured and something cut it
//! off". Nothing in this PR consumes that distinction; PR-5 of the plan
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
//! # What is wired in this PR
//!
//! One edge: `Edge::SelfTest`, behind `--features capture_selftest`, fired
//! once per boot from the timer-tick provider. It exists so every later PR
//! has a deterministic capture to test against instead of waiting for a rare
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
/// line and returns rather than recursing.
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
/// `#[cold]` and `#[inline(never)]`: this is off every hot path by
/// construction, and keeping it out of line keeps its stack frame and its
/// constants out of the callers that merely reference it.
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
            nested.close();
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
        writer.close();
    }
    writer.set_budgeted(true);

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
    // cut mid-line was opened but never closed, so it is not counted, which
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
        writer.close();
    }

    IN_CAPTURE[cpu_id].store(false, Ordering::Release);
}

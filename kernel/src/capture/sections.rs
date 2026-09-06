//! The `BXCAP v1` sections, in emission order.
//!
//! Order is "cheapest and most lock-free first": `EDGE` and `CPU` read only
//! per-CPU registers and statics, `EV`/`CNT`/`RING` read the lock-free trace
//! substrate, and `THR` -- the one section that wants the scheduler lock --
//! goes last and does not wait for it.
//!
//! A section returns `bool`: `true` when it emitted its content, `false`
//! when it refused or the budget stopped it. A `false` leaves that section's
//! bit clear in the capture's `completed` word, which is what `END`'s
//! `sections_skipped=` reports.
//!
//! "The budget stopped it" has two shapes, and a section has to report both.
//! `Writer::open()` refuses a record that starts with the budget already
//! spent, and each loop here checks that. A record that starts with a few
//! bytes left is cut MID-WRITE instead, and the one thing that says so is
//! `Writer::close()`'s return value -- so each `close()` below is either
//! returned or branched on. Ignoring one would report a fragment as a
//! completed section, which is exactly the claim `sections_skipped=` exists
//! to make truthfully.
//!
//! # THR is the inverse of the soft-lockup dump's old failure arm
//!
//! `arch_impl/aarch64/timer_interrupt.rs`'s `dump_lockup_state` USED TO print
//! `HELD (possible deadlock)` and then skip the trace-buffer dump -- which
//! needs no scheduler lock at all -- so it went quiet in exactly the case it
//! exists to report. `THR` here is the inverse: it is emitted LAST, it uses
//! the non-blocking `try_liveness_snapshot`, and a refusal costs only the
//! `THR` rows. Everything above it has already been emitted by then, and the
//! refusal itself is reported as `[BXCAP:NOTE sched_lock_held]`.
//!
//! PR-7 repaired that arm by deleting it: `dump_lockup_state` now calls
//! `capture::emit(Edge::Lockup, ...)` and has no scheduler branch of its own,
//! so a held scheduler lock during a soft lockup produces `EDGE`/`CPU`/`EV`/
//! `CNT`/`RING` and that NOTE rather than one line. What the repair does not
//! give back is the per-thread detail the old acquired-lock arm printed --
//! ELR/x30/SP, the inline-schedule stack scan, ready-queue membership and
//! per-thread flags. `THR`'s rows are per-CPU aggregates, and that regression
//! is recorded in
//! docs/planning/green-program/failure-capture/PR-7-2026-09-06.md rather than
//! described as equivalent.

use core::sync::atomic::Ordering;

use super::record::Writer;
use crate::tracing::{event_type_name, timestamp_to_nanos, TRACE_BUFFERS, TRACE_BUFFER_SIZE};

/// Section identifiers. The bit index is the section's position in
/// `sections_skipped=`, which is a bitmap over exactly these.
pub const SECTION_EDGE: u32 = 0;
pub const SECTION_CPU: u32 = 1;
pub const SECTION_EV: u32 = 2;
pub const SECTION_CNT: u32 = 3;
pub const SECTION_RING: u32 = 4;
pub const SECTION_THR: u32 = 5;

/// The 6 section bits set. `sections_skipped = SECTIONS_ALL & !completed`.
pub const SECTIONS_ALL: u64 = (1 << 6) - 1;

/// Newest ring events emitted, per capture. Caps the `EV` section
/// independently of the byte budget so a long ring tail cannot starve the
/// sections that follow it.
pub const BXCAP_MAX_EVENTS: usize = 32;

/// Nonzero counters emitted, per capture, in registration order. Same
/// argument as `BXCAP_MAX_EVENTS`: the tree registers 196 counters and a
/// capture that spent its whole budget on them would carry no `RING` or
/// `THR` rows.
pub const BXCAP_MAX_COUNTERS: usize = 32;

/// Per-CPU scheduler rows emitted by `THR`. Matches the fixed-size arrays
/// `SchedulerLivenessSnapshot` carries.
pub const BXCAP_MAX_THR_ROWS: usize = 8;

#[inline(always)]
pub fn section_bit(section: u32) -> u64 {
    1u64 << section
}

/// `[BXCAP:EDGE ...]` -- what fired this capture, plus the two opaque
/// edge-supplied words the caller passed.
pub fn edge(writer: &mut Writer, kind: &str, arg0: u64, arg1: u64) -> bool {
    if !writer.open("EDGE") {
        return false;
    }
    writer.kv_text("kind", kind);
    writer.kv_hex("a0", arg0);
    writer.kv_hex("a1", arg1);
    writer.close()
}

/// `[BXCAP:CPU ...]` -- the capturing CPU's own per-CPU state.
///
/// One row, for this CPU only. A peer CPU's per-CPU block is reachable only
/// through its own base register, so a row for it would be a guess; the
/// scheduler-side view of the other CPUs is what `THR` carries. The 6 fields
/// here are register or atomic reads, hence `q=exact`.
pub fn cpu(writer: &mut Writer) -> bool {
    use crate::arch_impl::current::percpu as hal_percpu;
    use crate::arch_impl::PerCpuOps;

    #[cfg(target_arch = "x86_64")]
    type Cpu = hal_percpu::X86PerCpu;
    #[cfg(target_arch = "aarch64")]
    type Cpu = hal_percpu::Aarch64PerCpu;

    if !writer.open("CPU") {
        return false;
    }
    writer.kv_dec("cpu", Cpu::cpu_id());
    writer.kv_dec("preempt", Cpu::preempt_count() as u64);
    writer.kv_dec("nr", Cpu::need_resched() as u64);
    writer.kv_dec("hardirq", Cpu::in_hardirq() as u64);
    writer.kv_dec("softirq", Cpu::in_softirq() as u64);
    writer.kv_dec("uptime_ms", crate::time::timer::get_monotonic_time());
    writer.kv_text("q", "exact");
    writer.close()
}

/// Borrow the capturing CPU's own ring buffer.
///
/// # Safety
///
/// Read-only access to a `static mut` whose single writer is this CPU, from
/// this CPU. A peer CPU writing its OWN buffer cannot touch this one, and a
/// concurrent write to this CPU's buffer would require this CPU to be
/// somewhere else, which it is not.
unsafe fn own_buffer(cpu_id: usize) -> Option<&'static crate::tracing::TraceCpuBuffer> {
    let buffers = core::ptr::addr_of!(TRACE_BUFFERS);
    if cpu_id >= (*buffers).len() {
        return None;
    }
    Some(&(*buffers)[cpu_id])
}

/// `[BXCAP:EV ...]` -- the newest `BXCAP_MAX_EVENTS` entries of the
/// capturing CPU's ring, oldest first, decoded to their event-type names.
pub fn events(writer: &mut Writer, cpu_id: usize) -> bool {
    let buffer = match unsafe { own_buffer(cpu_id) } {
        Some(buffer) => buffer,
        None => return false,
    };
    let live = core::cmp::min(buffer.write_index(), TRACE_BUFFER_SIZE);
    let skip = live.saturating_sub(BXCAP_MAX_EVENTS);
    let mut emitted = 0usize;
    for (index, event) in buffer.iter_events().enumerate() {
        if index < skip {
            continue;
        }
        if !writer.open("EV") {
            return false;
        }
        writer.kv_dec("cpu", cpu_id as u64);
        writer.kv_dec("i", index as u64);
        writer.kv_dec("ts", event.timestamp);
        writer.kv_hex("type", event.event_type as u64);
        writer.kv_text("n", event_type_name(event.event_type));
        writer.kv_dec("p", event.payload as u64);
        writer.kv_hex("f", event.flags as u64);
        if !writer.close() {
            return false;
        }
        emitted += 1;
        if emitted >= BXCAP_MAX_EVENTS {
            break;
        }
    }
    true
}

/// `[BXCAP:CNT <NAME>=<total>]` -- nonzero counters, registration order,
/// capped at `BXCAP_MAX_COUNTERS`.
///
/// Deliberately NOT `snapshot_all_counters()`: that returns
/// `[CounterSnapshot; 224]` by value, which is roughly 19 KiB of stack on a
/// path that may be a fault handler running on an already-deep stack.
/// Iterating the registry reads one aggregate at a time into a register.
pub fn counters(writer: &mut Writer) -> bool {
    let mut emitted = 0usize;
    for counter in crate::tracing::counter::list_counters() {
        let total = counter.aggregate();
        if total == 0 {
            continue;
        }
        if !writer.open("CNT") {
            return false;
        }
        writer.kv_dec(counter.name, total);
        if !writer.close() {
            return false;
        }
        emitted += 1;
        if emitted >= BXCAP_MAX_COUNTERS {
            break;
        }
    }
    true
}

/// `[BXCAP:RING ...]` -- the capturing CPU's ring accounting, so evidence
/// loss in the `EV` section above is visible rather than silent.
///
/// `dropped` is the overwrite count, `kept` the live entries, and `span_us`
/// the wall-clock distance between the oldest and newest live entries. A
/// short `span_us` next to a large `dropped` says the `EV` tail covers less
/// of the boot than a reader might assume.
pub fn ring(writer: &mut Writer, cpu_id: usize) -> bool {
    let buffer = match unsafe { own_buffer(cpu_id) } {
        Some(buffer) => buffer,
        None => return false,
    };
    let writes = buffer.write_index() as u64;
    let dropped = buffer.dropped_count();
    let kept = core::cmp::min(buffer.write_index(), TRACE_BUFFER_SIZE) as u64;

    let mut oldest: u64 = 0;
    let mut newest: u64 = 0;
    for event in buffer.iter_events() {
        if oldest == 0 {
            oldest = event.timestamp;
        }
        newest = event.timestamp;
    }
    let span_us = timestamp_to_nanos(newest.saturating_sub(oldest)) / 1_000;

    if !writer.open("RING") {
        return false;
    }
    writer.kv_dec("cpu", cpu_id as u64);
    writer.kv_dec("writes", writes);
    writer.kv_dec("dropped", dropped);
    writer.kv_dec("kept", kept);
    writer.kv_dec("span_us", span_us);
    writer.kv_dec("enabled", crate::tracing::TRACE_ENABLED.load(Ordering::Relaxed));
    writer.close()
}

/// Ask the scheduler for a fixed-size liveness snapshot, without waiting.
///
/// `try_liveness_snapshot` masks local interrupts for its guard's whole
/// window and acquires the scheduler lock with `try_lock`, so it returns
/// `None` rather than blocking when the lock is busy. It fills fixed-size
/// arrays and performs no allocation, which is why the capture calls it and
/// not `try_dump_state` -- that one builds two `alloc` vectors while holding
/// the guard, which is the shape a capture path must not reach.
/// claim-lint:ok: the flagged word is Rust's `Option::None`, not a claim;
/// the allocation-free property is pinned by
/// tests/capture_path_lock_free_structure.rs and by the `try_dump_state`
/// entry in scripts/check-critical-path-violations.sh.
///
/// SAMPLED EARLY, EMITTED LAST, on purpose. The rows appear at the end of
/// the capture so a fault inside the bulk `EV`/`CNT` sections cannot cost
/// the cheap ones, but the lock is asked for before those sections spend
/// milliseconds on the wire -- asking afterwards loses the race far more
/// often for no benefit, because the value being read is a snapshot either
/// way. The `THR` rows therefore describe the scheduler at the moment
/// `[BXCAP:CPU]`'s `uptime_ms` was taken, not at the moment they are
/// printed.
pub fn sample_threads(cpu_id: usize) -> Option<crate::task::scheduler::SchedulerLivenessSnapshot> {
    crate::task::scheduler::try_liveness_snapshot(cpu_id)
}

/// `[BXCAP:THR ...]` -- per-CPU scheduler state, or a refusal.
///
/// A `None` from `sample_threads` is not an error path to be silent about:
/// it is emitted as `[BXCAP:NOTE sched_lock_held]`, the section's bit stays
/// clear in `sections_skipped=`, and the capture continues to its `END`.
/// claim-lint:ok: the flagged word is Rust's `Option::None`, not a claim;
/// the refusal-is-stated behaviour is pinned by
/// tests/capture_path_lock_free_structure.rs and shown on a real boot in
/// docs/planning/green-program/failure-capture/serials/pr3/aarch64-selftest-sched-lock-held.txt
pub fn threads(
    writer: &mut Writer,
    sampled: Option<crate::task::scheduler::SchedulerLivenessSnapshot>,
) -> bool {
    let snapshot = match sampled {
        Some(snapshot) => snapshot,
        None => {
            if writer.open("NOTE") {
                writer.text(" sched_lock_held");
                // The one deliberate discard in this file. The arm returns
                // `false` whatever the budget did to the note, because the
                // section it is refusing for was not emitted either way.
                let _ = writer.close();
            }
            return false;
        }
    };
    for index in 0..BXCAP_MAX_THR_ROWS {
        if !writer.open("THR") {
            return false;
        }
        writer.kv_dec("cpu", index as u64);
        writer.kv_dec("cur", snapshot.per_cpu_current[index]);
        writer.kv_dec("rq", snapshot.per_cpu_ready_len[index]);
        writer.kv_dec("threads", snapshot.total_threads);
        writer.kv_dec("blocked", snapshot.blocked_count);
        writer.kv_text("q", "exact");
        if !writer.close() {
            return false;
        }
    }
    true
}

//! Interrupt trace provider.
//!
//! This provider traces interrupt entry, exit, and specific interrupt events
//! like timer ticks.
//!
//! # Event Types
//!
//! - `IRQ_ENTRY` (0x0100): Interrupt entry, payload = interrupt vector
//! - `IRQ_EXIT` (0x0101): Interrupt exit, payload = interrupt vector
//! - `TIMER_TICK` (0x0102): Timer tick, payload = tick count (low 32 bits).
//!   Recorded once per `TICK_SAMPLE` ticks (see below); `TIMER_TICK_TOTAL`
//!   keeps counting on each tick regardless.
//! - `HEARTBEAT_MARKER` (0x0103): Heartbeat marker, payload = diagnostic value
//!
//! # Usage
//!
//! ```rust,ignore
//! use kernel::tracing::providers::irq::{IRQ_PROVIDER, TIMER_TICK};
//! use kernel::trace_event;
//!
//! // Enable timer tracing only
//! IRQ_PROVIDER.enable_probe(2); // TIMER_TICK
//!
//! // In timer handler:
//! trace_event!(IRQ_PROVIDER, TIMER_TICK, tick_count as u32);
//! ```

use crate::tracing::provider::{register_provider, TraceProvider};
use crate::tracing::providers::counters::{IRQ_TOTAL, TIMER_TICK_TOTAL};
use core::sync::atomic::AtomicU64;

/// Provider ID for interrupt events (0x01xx range).
pub const PROVIDER_ID: u8 = 0x01;

/// Interrupt trace provider.
///
/// GDB: `print IRQ_PROVIDER`
#[no_mangle]
pub static IRQ_PROVIDER: TraceProvider = TraceProvider {
    name: "irq",
    id: PROVIDER_ID,
    enabled: AtomicU64::new(0),
};

// =============================================================================
// Probe Definitions
// =============================================================================

/// Probe ID for interrupt entry.
pub const PROBE_ENTRY: u8 = 0x00;

/// Probe ID for interrupt exit.
pub const PROBE_EXIT: u8 = 0x01;

/// Probe ID for timer tick.
pub const PROBE_TIMER_TICK: u8 = 0x02;

/// Probe ID for heartbeat marker.
pub const PROBE_HEARTBEAT_MARKER: u8 = 0x03;

/// Event type for interrupt entry.
/// Payload: interrupt vector number.
pub const IRQ_ENTRY: u16 = ((PROVIDER_ID as u16) << 8) | (PROBE_ENTRY as u16);

/// Event type for interrupt exit.
/// Payload: interrupt vector number.
pub const IRQ_EXIT: u16 = ((PROVIDER_ID as u16) << 8) | (PROBE_EXIT as u16);

/// Event type for timer tick.
/// Payload: tick count (low 32 bits).
pub const TIMER_TICK: u16 = ((PROVIDER_ID as u16) << 8) | (PROBE_TIMER_TICK as u16);

/// Event type for heartbeat marker.
/// Payload: uptime in seconds or diagnostic value.
pub const HEARTBEAT_MARKER: u16 = ((PROVIDER_ID as u16) << 8) | (PROBE_HEARTBEAT_MARKER as u16);

// =============================================================================
// Ring Depth Sampling (failure-trace-capture PR-2)
// =============================================================================

/// A `TIMER_TICK` ring EVENT is recorded once per `TICK_SAMPLE` ticks.
/// `TIMER_TICK_TOTAL` -- and the readers that depend on it: the soft-lockup
/// detector, per-CPU idle-tick accounting, `/proc/stat` -- keeps incrementing
/// on each tick regardless, so this trades ring density for ring depth and
/// loses no counted tick. The tick number is already the payload recorded on
/// a sampled write, so no information is lost, only the spacing between
/// samples.
///
/// Must be a power of two: the guard below tests `tick_count & (TICK_SAMPLE
/// - 1) == 0`, not `tick_count % TICK_SAMPLE == 0` -- a non-power-of-two
/// divisor makes the compiler lower that test to its magic-number
/// division-by-constant sequence (measured for the `= 20` this constant
/// originally shipped as: a materialized 64-bit reciprocal constant, `mul`,
/// `ror` and `cmp` in a release aarch64 `boot_tests` disassembly of
/// `timer_interrupt_handler`, PR-2 fix round 2026-09-05). A bitwise AND
/// against an immediate is one instruction; the const assertion below
/// enforces the power-of-two invariant so a future edit picking a
/// non-power-of-two value fails to build instead of silently
/// reintroducing the division.
///
/// claim-lint:ok: measured on this branch -- PR-2's original round
/// (docs/planning/green-program/failure-capture/PR-2-2026-09-05.md, 3 boots
/// per configuration at `TICK_SAMPLE = 20`) and this file's fix round (same
/// doc, section 9, at `TICK_SAMPLE = 16`).
/// The shared per-CPU ring also carries other providers' traffic (scheduler,
/// syscalls, ...), and once userspace bring-up starts that other traffic can
/// dominate the ring's overall turnover regardless of this constant --
/// observed at up to several hundred thousand non-timer writes within a few
/// seconds in 2 of 2 configurations tried past that point. Before that
/// point, at `boot_tests`' `RING_SPAN` self-check below, this constant is
/// what stands between an early fatal dump's `TIMER_TICK` history reaching
/// back a checkpoint-and-arch-specific short span (unsampled) or several
/// times that (sampled) -- both bounded by the same 1024-entry ring, so the
/// sampling factor does not appear as an equal span factor once the ring is
/// shared. The exact measured spans, which differ by checkpoint and by
/// architecture, are recorded in
/// docs/planning/green-program/failure-capture/PR-2-2026-09-05.md section 9
/// (fix round, current) and section 2.2 (original calibration, at the prior
/// `TICK_SAMPLE = 20` value, superseded by section 9's numbers but kept for
/// the record of how the checkpoint was originally chosen). See also the
/// open question on this constant's exact value in section 8 (Q1) -- still
/// open; 16 is the nearest power of two to the original 20, not a new,
/// independently re-derived target.
pub const TICK_SAMPLE: u64 = 16;

const _: () = assert!(
    TICK_SAMPLE.is_power_of_two(),
    "TICK_SAMPLE must be a power of two so the sampling guard can test it \
     with a single bitwise AND instead of a runtime division"
);

// =============================================================================
// Initialization
// =============================================================================

/// Initialize the interrupt provider.
///
/// Registers the provider with the global registry.
pub fn init() {
    register_provider(&IRQ_PROVIDER);
}

// =============================================================================
// Inline Tracing Functions
// =============================================================================

/// Trace interrupt entry (inline for minimal overhead).
///
/// Also increments the IRQ_TOTAL counter (single atomic add, always runs).
///
/// # Parameters
///
/// - `vector`: The interrupt vector number
#[inline(always)]
#[allow(dead_code)]
pub fn trace_irq_entry(vector: u8) {
    // Always increment the counter (single atomic add, ~3 cycles)
    IRQ_TOTAL.increment();

    // Only record trace event if tracing is enabled
    if IRQ_PROVIDER.is_enabled() && crate::tracing::is_enabled() {
        crate::tracing::record_event(IRQ_ENTRY, 0, vector as u32);
    }
}

/// Trace interrupt exit (inline for minimal overhead).
///
/// # Parameters
///
/// - `vector`: The interrupt vector number
#[inline(always)]
#[allow(dead_code)]
pub fn trace_irq_exit(vector: u8) {
    if IRQ_PROVIDER.is_enabled() && crate::tracing::is_enabled() {
        crate::tracing::record_event(IRQ_EXIT, 0, vector as u32);
    }
}

/// Trace timer tick (inline for minimal overhead).
///
/// Also increments the TIMER_TICK_TOTAL counter (single atomic add, runs on
/// each call). Records a ring EVENT once per `TICK_SAMPLE` ticks -- see the
/// constant's doc comment above for why that loses no counted tick and no
/// information.
///
/// # Parameters
///
/// - `tick_count`: The current tick count
#[inline(always)]
#[allow(dead_code)]
pub fn trace_timer_tick(tick_count: u64) {
    // Unconditionally increment the counter (single atomic add, ~3 cycles)
    TIMER_TICK_TOTAL.increment();

    // Only record a ring event if tracing is enabled AND this is a sampled
    // tick. tick_count is already the payload, so sampling costs no
    // information, only density between recorded points.
    if IRQ_PROVIDER.is_enabled()
        && crate::tracing::is_enabled()
        && tick_count & (TICK_SAMPLE - 1) == 0
    {
        crate::tracing::record_event(TIMER_TICK, 0, tick_count as u32);
    }

    #[cfg(feature = "boot_tests")]
    ring_span_self_check::observe(tick_count);

    // The BXCAP self-test edge (failure-capture PR-3). Behind
    // `capture_selftest`, which no gate builds with, so a gated or shipped
    // kernel does not compile this line at all.
    #[cfg(feature = "capture_selftest")]
    crate::capture::selftest::observe(tick_count);

    // The BXCAP `edge=PANIC` oracle (failure-capture PR-4). Behind
    // `capture_panic_oracle`, which no gate builds with, so a gated or
    // shipped kernel does not compile this line at all.
    #[cfg(feature = "capture_panic_oracle")]
    crate::capture_oracle::observe(tick_count);
}

/// Trace heartbeat marker (inline for minimal overhead).
///
/// Records a periodic heartbeat in the trace buffer as a replacement
/// for serial output in the timer interrupt.
///
/// # Parameters
///
/// - `payload`: Diagnostic value (e.g., uptime in seconds or xHCI completion code)
#[inline(always)]
#[allow(dead_code)]
pub fn trace_heartbeat_marker(payload: u32) {
    if IRQ_PROVIDER.is_enabled() && crate::tracing::is_enabled() {
        crate::tracing::record_event(HEARTBEAT_MARKER, 0, payload);
    }
}

// =============================================================================
// Boot-test self-check: ring span oracle (failure-trace-capture PR-2)
// =============================================================================

/// Measures whether the sampling above actually buys `TIMER_TICK` ring
/// depth, from inside the boot itself rather than from arithmetic on the
/// constants.
///
/// claim-lint:ok: measured on this branch,
/// docs/planning/green-program/failure-capture/PR-2-2026-09-05.md.
/// `span_ms` below is computed over the `TIMER_TICK`-typed entries currently
/// live in CPU 0's ring specifically, not the ring's raw oldest-to-newest
/// span. The ring is shared with other providers (scheduler, syscalls,
/// ...), and under `boot_tests`' own concurrent oracle machinery that other
/// traffic can dominate the ring's overall turnover -- measured on this
/// branch at up to several hundred thousand non-timer writes in the first
/// few seconds of a boot. Sampling `TIMER_TICK` cannot do anything about
/// that other traffic's own write rate; what it controls, and what this
/// measures directly, is how far back `TIMER_TICK`'s own history still
/// reaches despite it. `writes`/`dropped` stay ring-wide (total turnover,
/// for context); only `span_ms` is TIMER_TICK-specific.
///
/// This runs inside `trace_timer_tick`, which both timer interrupt handlers
/// (the Tier-1 `kernel/src/interrupts/timer.rs` on x86 and the critical-path
/// `kernel/src/arch_impl/aarch64/timer_interrupt.rs`) call directly and
/// unconditionally on each tick, so it is held to the same rules as the ISR
/// that reaches it even though `boot_tests` does not ship: the one-shot
/// latch check is a single relaxed load, and the reporting path that fires
/// exactly once uses only the lock-free `raw_serial_*` writers
/// `tracing::output`'s own panic-safe dump functions use -- no lock, no
/// allocation, no `format!`.
///
/// See docs/planning/green-program/failure-capture/PR-2-2026-09-05.md.
#[cfg(feature = "boot_tests")]
mod ring_span_self_check {
    use core::sync::atomic::{AtomicBool, Ordering};

    use super::TIMER_TICK;
    use crate::tracing::core::TRACE_BUFFERS;
    use crate::tracing::output::{raw_serial_dec, raw_serial_newline, raw_serial_str};
    use crate::tracing::timestamp::timestamp_to_nanos;

    /// Wall-clock point (`tick_count * MS_PER_TICK`, arch-neutral: 1 ms/tick
    /// on aarch64, `PIT_HZ`-derived on x86) at which the one-shot measurement
    /// fires. Late enough that a meaningful number of ticks have flowed
    /// through `trace_timer_tick`, early enough to land before userspace
    /// bring-up traffic starts (see the PR-2 round doc, sections 2.2 and 9).
    ///
    /// This does NOT need to be tuned so the unsampled baseline wraps CPU
    /// 0's ring by this point (an earlier version of this comment, and of
    /// `report()` below, claimed exactly that -- see PR-2-2026-09-05.md
    /// section 9 for why it was wrong on both counts: x86's 5x-slower PIT
    /// tick rate meant this checkpoint's nominal tick count did not come
    /// close to the ring's capacity there, and aarch64's own wrap timing
    /// was itself close enough to this checkpoint, under normal boot-to-boot
    /// host jitter, to sometimes go either way -- a real, measured ~8%
    /// false-pass rate against a `TICK_SAMPLE = 1` mutation across the two
    /// combined sample sets in that section). `report()`'s `ticks_total` /
    /// `tick_events` ratio is what actually proves the guard is gating (see
    /// its own doc comment), and does not depend on ring-wrap timing at
    /// all, so this checkpoint only needs "enough ticks have happened for
    /// the ratio to be meaningful" -- true within the first handful of
    /// samples, well before 1 s on either architecture.
    const CHECK_AT_MS: u64 = 1_000;

    static CHECKED: AtomicBool = AtomicBool::new(false);

    /// Called on each tick; returns immediately once latched. The
    /// comparison and the latch load are the only per-tick cost this adds.
    #[inline(always)]
    pub(super) fn observe(tick_count: u64) {
        if CHECKED.load(Ordering::Relaxed) {
            return;
        }
        let elapsed_ms = tick_count.saturating_mul(crate::time::timer::MS_PER_TICK);
        if elapsed_ms < CHECK_AT_MS {
            return;
        }
        if CHECKED.swap(true, Ordering::AcqRel) {
            return;
        }
        report();
    }

    /// Reads CPU 0's ring with the same raw-pointer idiom
    /// `tracing::output::dump_buffer` uses, and prints
    /// `[RING_SPAN:cpu=0:span_ms=<N>:writes=<W>:dropped=<D>]`. `span_ms` is
    /// the delta between the oldest and newest live `TIMER_TICK` entries --
    /// see the module doc comment for why it is filtered to that type.
    fn report() {
        // SAFETY: read-only access to CPU 0's own ring buffer, from CPU 0's
        // own timer-tick path with no concurrent writer of this slot (single
        // writer per per-CPU buffer, and the current tick's own write, if
        // any, already landed above before this function is reached).
        let buffer = unsafe {
            let buffers_ptr = core::ptr::addr_of!(TRACE_BUFFERS);
            &(*buffers_ptr)[0]
        };

        let writes = buffer.write_index() as u64;
        let dropped = buffer.dropped_count();

        let mut oldest = None;
        let mut newest = None;
        let mut tick_events: u64 = 0;
        for event in buffer.iter_events().filter(|event| event.event_type == TIMER_TICK) {
            if oldest.is_none() {
                oldest = Some(event);
            }
            newest = Some(event);
            tick_events += 1;
        }
        let span_ms = match (oldest, newest) {
            (Some(oldest), Some(newest)) => {
                let delta = newest.timestamp.saturating_sub(oldest.timestamp);
                timestamp_to_nanos(delta) / 1_000_000
            }
            _ => 0,
        };

        // `ticks_total` is TIMER_TICK_TOTAL's own aggregate -- incremented
        // unconditionally on each tick, with no sampling applied (see
        // `trace_timer_tick` above) -- and `tick_events` is the count of
        // TIMER_TICK-typed entries the ring is currently holding, counted in
        // the same pass as `oldest`/`newest` above rather than a second
        // traversal. Their ratio is a wall-clock-jitter-immune signal of
        // whether the sampling guard is gating: a working guard holds
        // `tick_events` near `ticks_total / TICK_SAMPLE` regardless of
        // when in the boot this fires or whether the ring has wrapped,
        // where `span_ms` alone (both quantities are timing measurements
        // taken at the same instant, and can coincidentally agree even when
        // the guard is not gating -- see PR-2-2026-09-05.md section 9,
        // the aarch64-mutation-flaky finding) does not share. A guard that
        // has been deleted (recording each tick unconditionally) makes this
        // ratio collapse to ~1 regardless of timing.
        let ticks_total = crate::tracing::providers::counters::TIMER_TICK_TOTAL.aggregate();

        raw_serial_str("[RING_SPAN:cpu=0:span_ms=");
        raw_serial_dec(span_ms);
        raw_serial_str(":writes=");
        raw_serial_dec(writes);
        raw_serial_str(":dropped=");
        raw_serial_dec(dropped);
        raw_serial_str(":ticks_total=");
        raw_serial_dec(ticks_total);
        raw_serial_str(":tick_events=");
        raw_serial_dec(tick_events);
        raw_serial_str("]");
        raw_serial_newline();
    }
}

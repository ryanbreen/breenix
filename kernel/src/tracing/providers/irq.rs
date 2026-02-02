//! Interrupt trace provider.
//!
//! This provider traces interrupt entry, exit, and specific interrupt events
//! like timer ticks.
//!
//! # Event Types
//!
//! - `IRQ_ENTRY` (0x0100): Interrupt entry, payload = interrupt vector
//! - `IRQ_EXIT` (0x0101): Interrupt exit, payload = interrupt vector
//! - `TIMER_TICK` (0x0102): Timer tick, payload = tick count (low 32 bits)
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

/// Event type for interrupt entry.
/// Payload: interrupt vector number.
pub const IRQ_ENTRY: u16 = ((PROVIDER_ID as u16) << 8) | (PROBE_ENTRY as u16);

/// Event type for interrupt exit.
/// Payload: interrupt vector number.
pub const IRQ_EXIT: u16 = ((PROVIDER_ID as u16) << 8) | (PROBE_EXIT as u16);

/// Event type for timer tick.
/// Payload: tick count (low 32 bits).
pub const TIMER_TICK: u16 = ((PROVIDER_ID as u16) << 8) | (PROBE_TIMER_TICK as u16);

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
/// Also increments the TIMER_TICK_TOTAL counter (single atomic add, always runs).
///
/// # Parameters
///
/// - `tick_count`: The current tick count
#[inline(always)]
#[allow(dead_code)]
pub fn trace_timer_tick(tick_count: u64) {
    // Always increment the counter (single atomic add, ~3 cycles)
    TIMER_TICK_TOTAL.increment();

    // Only record trace event if tracing is enabled
    if IRQ_PROVIDER.is_enabled() && crate::tracing::is_enabled() {
        crate::tracing::record_event(TIMER_TICK, 0, tick_count as u32);
    }
}

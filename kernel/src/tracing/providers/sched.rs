//! Scheduler trace provider.
//!
//! This provider traces scheduler and context switch events.
//! Events capture thread IDs and scheduling decisions.
//!
//! # Event Types
//!
//! - `CTX_SWITCH_ENTRY` (0x0001): Context switch beginning, payload = packed(old_tid, new_tid)
//! - `CTX_SWITCH_EXIT` (0x0002): Context switch complete, payload = new_tid
//! - `SCHED_PICK` (0x0200): Scheduler picked a thread, payload = thread_id
//! - `SCHED_RESCHED` (0x0201): Reschedule requested, payload = 0
//!
//! # Usage
//!
//! ```rust,ignore
//! use kernel::tracing::providers::sched::{SCHED_PROVIDER, CTX_SWITCH_ENTRY};
//! use kernel::trace_event_2;
//!
//! // Enable context switch tracing
//! SCHED_PROVIDER.enable_probe(0); // CTX_SWITCH_ENTRY
//!
//! // In context switch code:
//! trace_event_2!(SCHED_PROVIDER, CTX_SWITCH_ENTRY, old_tid as u16, new_tid as u16);
//! ```

use crate::tracing::provider::{register_provider, TraceProvider};
use crate::tracing::providers::counters::CTX_SWITCH_TOTAL;
use core::sync::atomic::AtomicU64;

/// Provider ID for scheduler events.
/// Uses 0x00 for context switch events (0x00xx) and 0x02 for scheduler events (0x02xx).
/// For simplicity, we use a single provider with both ranges.
pub const PROVIDER_ID: u8 = 0x00;

/// Scheduler trace provider.
///
/// GDB: `print SCHED_PROVIDER`
#[no_mangle]
pub static SCHED_PROVIDER: TraceProvider = TraceProvider {
    name: "sched",
    id: PROVIDER_ID,
    enabled: AtomicU64::new(0),
};

// =============================================================================
// Context Switch Probes (0x00xx range)
// =============================================================================

/// Probe ID for context switch entry.
pub const PROBE_CTX_SWITCH_ENTRY: u8 = 0x01;

/// Probe ID for context switch exit.
pub const PROBE_CTX_SWITCH_EXIT: u8 = 0x02;

/// Probe ID for switch to userspace.
pub const PROBE_CTX_SWITCH_TO_USER: u8 = 0x03;

/// Probe ID for switch to kernel.
pub const PROBE_CTX_SWITCH_TO_KERNEL: u8 = 0x04;

/// Probe ID for switch to idle.
pub const PROBE_CTX_SWITCH_TO_IDLE: u8 = 0x05;

/// Event type for context switch entry.
/// Payload: packed(old_tid[15:0], new_tid[15:0]).
pub const CTX_SWITCH_ENTRY: u16 = ((PROVIDER_ID as u16) << 8) | (PROBE_CTX_SWITCH_ENTRY as u16);

/// Event type for context switch exit.
/// Payload: new_tid.
pub const CTX_SWITCH_EXIT: u16 = ((PROVIDER_ID as u16) << 8) | (PROBE_CTX_SWITCH_EXIT as u16);

/// Event type for switch to userspace.
/// Payload: thread_id.
pub const CTX_SWITCH_TO_USER: u16 = ((PROVIDER_ID as u16) << 8) | (PROBE_CTX_SWITCH_TO_USER as u16);

/// Event type for switch to kernel.
/// Payload: thread_id.
pub const CTX_SWITCH_TO_KERNEL: u16 =
    ((PROVIDER_ID as u16) << 8) | (PROBE_CTX_SWITCH_TO_KERNEL as u16);

/// Event type for switch to idle.
/// Payload: 0.
pub const CTX_SWITCH_TO_IDLE: u16 = ((PROVIDER_ID as u16) << 8) | (PROBE_CTX_SWITCH_TO_IDLE as u16);

// =============================================================================
// Scheduler Probes (using upper probe IDs to avoid collision)
// =============================================================================

/// Probe ID for scheduler pick.
pub const PROBE_SCHED_PICK: u8 = 0x10;

/// Probe ID for reschedule request.
pub const PROBE_SCHED_RESCHED: u8 = 0x11;

/// Event type for scheduler picking a thread.
/// Payload: thread_id.
pub const SCHED_PICK: u16 = ((PROVIDER_ID as u16) << 8) | (PROBE_SCHED_PICK as u16);

/// Event type for reschedule request.
/// Payload: 0.
pub const SCHED_RESCHED: u16 = ((PROVIDER_ID as u16) << 8) | (PROBE_SCHED_RESCHED as u16);

// =============================================================================
// Initialization
// =============================================================================

/// Initialize the scheduler provider.
///
/// Registers the provider with the global registry.
pub fn init() {
    register_provider(&SCHED_PROVIDER);
}

// =============================================================================
// Inline Tracing Functions
// =============================================================================

/// Trace context switch entry (inline for minimal overhead).
///
/// Also increments the CTX_SWITCH_TOTAL counter (single atomic add, always runs).
///
/// # Parameters
///
/// - `old_tid`: Thread ID of the thread being switched from
/// - `new_tid`: Thread ID of the thread being switched to
#[inline(always)]
#[allow(dead_code)]
pub fn trace_ctx_switch(old_tid: u64, new_tid: u64) {
    // Always increment the counter (single atomic add, ~3 cycles)
    CTX_SWITCH_TOTAL.increment();

    // Only record trace event if tracing is enabled
    if SCHED_PROVIDER.is_enabled() && crate::tracing::is_enabled() {
        crate::tracing::record_event_2(CTX_SWITCH_ENTRY, old_tid as u16, new_tid as u16);
    }
}

/// Trace switch to idle (inline for minimal overhead).
#[inline(always)]
#[allow(dead_code)]
pub fn trace_switch_to_idle() {
    if SCHED_PROVIDER.is_enabled() && crate::tracing::is_enabled() {
        crate::tracing::record_event(CTX_SWITCH_TO_IDLE, 0, 0);
    }
}

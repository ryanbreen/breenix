//! Syscall trace provider.
//!
//! This provider traces system call entry and exit events.
//! Each event captures the syscall number (entry) or return value (exit).
//!
//! # Event Types
//!
//! - `SYSCALL_ENTRY` (0x0300): Syscall entry, payload = syscall number
//! - `SYSCALL_EXIT` (0x0301): Syscall exit, payload = return value (low 32 bits)
//!
//! # Usage
//!
//! ```rust,ignore
//! use kernel::tracing::providers::syscall::{SYSCALL_PROVIDER, SYSCALL_ENTRY, SYSCALL_EXIT};
//! use kernel::trace_event;
//!
//! // Enable syscall tracing
//! SYSCALL_PROVIDER.enable_all();
//!
//! // In syscall handler:
//! trace_event!(SYSCALL_PROVIDER, SYSCALL_ENTRY, syscall_nr as u32);
//! // ... handle syscall ...
//! trace_event!(SYSCALL_PROVIDER, SYSCALL_EXIT, result as u32);
//! ```

use crate::tracing::provider::{register_provider, TraceProvider};
use crate::tracing::providers::counters::SYSCALL_TOTAL;
use core::sync::atomic::AtomicU64;

/// Provider ID for syscall events (0x03xx range).
pub const PROVIDER_ID: u8 = 0x03;

/// Syscall trace provider.
///
/// GDB: `print SYSCALL_PROVIDER`
#[no_mangle]
pub static SYSCALL_PROVIDER: TraceProvider = TraceProvider {
    name: "syscall",
    id: PROVIDER_ID,
    enabled: AtomicU64::new(0),
};

// =============================================================================
// Probe Definitions
// =============================================================================

/// Probe ID for syscall entry.
pub const PROBE_ENTRY: u8 = 0x00;

/// Probe ID for syscall exit.
pub const PROBE_EXIT: u8 = 0x01;

/// Event type for syscall entry.
/// Payload: syscall number (32-bit).
pub const SYSCALL_ENTRY: u16 = ((PROVIDER_ID as u16) << 8) | (PROBE_ENTRY as u16);

/// Event type for syscall exit.
/// Payload: return value (low 32 bits, can be negative for errors).
pub const SYSCALL_EXIT: u16 = ((PROVIDER_ID as u16) << 8) | (PROBE_EXIT as u16);

// =============================================================================
// Initialization
// =============================================================================

/// Initialize the syscall provider.
///
/// Registers the provider with the global registry.
pub fn init() {
    register_provider(&SYSCALL_PROVIDER);
}

// =============================================================================
// Inline Tracing Functions
// =============================================================================

/// Trace syscall entry (inline for minimal overhead).
///
/// This is a convenience function that combines the enable check and record.
/// Use this instead of the macro if you need to avoid macro expansion issues.
///
/// Also increments the SYSCALL_TOTAL counter (single atomic add, always runs).
///
/// # Parameters
///
/// - `syscall_nr`: The syscall number
#[inline(always)]
#[allow(dead_code)]
pub fn trace_entry(syscall_nr: u64) {
    // Always increment the counter (single atomic add, ~3 cycles)
    SYSCALL_TOTAL.increment();

    // Only record trace event if tracing is enabled
    if SYSCALL_PROVIDER.is_enabled() && crate::tracing::is_enabled() {
        crate::tracing::record_event(SYSCALL_ENTRY, 0, syscall_nr as u32);
    }
}

/// Trace syscall exit (inline for minimal overhead).
///
/// # Parameters
///
/// - `result`: The syscall return value
#[inline(always)]
#[allow(dead_code)]
pub fn trace_exit(result: i64) {
    if SYSCALL_PROVIDER.is_enabled() && crate::tracing::is_enabled() {
        crate::tracing::record_event(SYSCALL_EXIT, 0, result as u32);
    }
}

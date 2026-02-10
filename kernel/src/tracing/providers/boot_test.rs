//! Boot test trace provider.
//!
//! Traces boot test lifecycle events for the BTRT system.
//!
//! # Event Types
//!
//! - `TEST_REGISTER` (0x0700): Test registered, payload = test_id
//! - `TEST_START` (0x0701): Test started, payload = test_id
//! - `TEST_PASS` (0x0702): Test passed, payload = packed(test_id, duration_ms)
//! - `TEST_FAIL` (0x0703): Test failed, payload = packed(test_id, error_code)
//! - `TEST_SKIP` (0x0704): Test skipped, payload = test_id
//! - `TEST_TIMEOUT` (0x0705): Test timed out, payload = test_id
//!
//! # Usage
//!
//! ```rust,ignore
//! use kernel::tracing::providers::boot_test;
//!
//! boot_test::trace_test_pass(42);
//! ```

use crate::tracing::provider::{register_provider, TraceProvider};
use crate::tracing::providers::counters::{
    BOOT_TEST_FAIL_TOTAL, BOOT_TEST_PASS_TOTAL, BOOT_TEST_SKIP_TOTAL, BOOT_TEST_TOTAL,
};
use core::sync::atomic::AtomicU64;

/// Provider ID for boot test events (0x07xx range).
pub const PROVIDER_ID: u8 = 0x07;

/// Boot test trace provider.
///
/// GDB: `print BOOT_TEST_PROVIDER`
#[no_mangle]
pub static BOOT_TEST_PROVIDER: TraceProvider = TraceProvider {
    name: "boot_test",
    id: PROVIDER_ID,
    enabled: AtomicU64::new(0),
};

// =============================================================================
// Probe Definitions
// =============================================================================

/// Probe ID for test registration.
pub const PROBE_TEST_REGISTER: u8 = 0x00;

/// Probe ID for test start.
pub const PROBE_TEST_START: u8 = 0x01;

/// Probe ID for test pass.
pub const PROBE_TEST_PASS: u8 = 0x02;

/// Probe ID for test fail.
pub const PROBE_TEST_FAIL: u8 = 0x03;

/// Probe ID for test skip.
pub const PROBE_TEST_SKIP: u8 = 0x04;

/// Probe ID for test timeout.
pub const PROBE_TEST_TIMEOUT: u8 = 0x05;

/// Event type for test registration.
pub const TEST_REGISTER: u16 = ((PROVIDER_ID as u16) << 8) | (PROBE_TEST_REGISTER as u16);

/// Event type for test start.
pub const TEST_START: u16 = ((PROVIDER_ID as u16) << 8) | (PROBE_TEST_START as u16);

/// Event type for test pass.
pub const TEST_PASS: u16 = ((PROVIDER_ID as u16) << 8) | (PROBE_TEST_PASS as u16);

/// Event type for test fail.
pub const TEST_FAIL: u16 = ((PROVIDER_ID as u16) << 8) | (PROBE_TEST_FAIL as u16);

/// Event type for test skip.
pub const TEST_SKIP: u16 = ((PROVIDER_ID as u16) << 8) | (PROBE_TEST_SKIP as u16);

/// Event type for test timeout.
pub const TEST_TIMEOUT: u16 = ((PROVIDER_ID as u16) << 8) | (PROBE_TEST_TIMEOUT as u16);

// =============================================================================
// Initialization
// =============================================================================

/// Initialize the boot test provider.
///
/// Registers the provider with the global registry.
pub fn init() {
    register_provider(&BOOT_TEST_PROVIDER);
}

// =============================================================================
// Inline Tracing Functions
// =============================================================================

/// Trace test start (inline for minimal overhead).
///
/// Also increments the BOOT_TEST_TOTAL counter.
#[inline(always)]
#[allow(dead_code)]
pub fn trace_test_start(test_id: u16) {
    BOOT_TEST_TOTAL.increment();

    if BOOT_TEST_PROVIDER.is_enabled() && crate::tracing::is_enabled() {
        crate::tracing::record_event(TEST_START, 0, test_id as u32);
    }
}

/// Trace test pass (inline for minimal overhead).
///
/// Also increments the BOOT_TEST_PASS_TOTAL counter.
#[inline(always)]
#[allow(dead_code)]
pub fn trace_test_pass(test_id: u16) {
    BOOT_TEST_PASS_TOTAL.increment();

    if BOOT_TEST_PROVIDER.is_enabled() && crate::tracing::is_enabled() {
        crate::tracing::record_event(TEST_PASS, 0, test_id as u32);
    }
}

/// Trace test fail (inline for minimal overhead).
///
/// Also increments the BOOT_TEST_FAIL_TOTAL counter.
#[inline(always)]
#[allow(dead_code)]
pub fn trace_test_fail(test_id: u16, error_code: u8) {
    BOOT_TEST_FAIL_TOTAL.increment();

    if BOOT_TEST_PROVIDER.is_enabled() && crate::tracing::is_enabled() {
        crate::tracing::record_event_2(TEST_FAIL, test_id, error_code as u16);
    }
}

/// Trace test skip (inline for minimal overhead).
///
/// Also increments the BOOT_TEST_SKIP_TOTAL counter.
#[inline(always)]
#[allow(dead_code)]
pub fn trace_test_skip(test_id: u16) {
    BOOT_TEST_SKIP_TOTAL.increment();

    if BOOT_TEST_PROVIDER.is_enabled() && crate::tracing::is_enabled() {
        crate::tracing::record_event(TEST_SKIP, 0, test_id as u32);
    }
}

/// Trace test timeout (inline for minimal overhead).
#[inline(always)]
#[allow(dead_code)]
pub fn trace_test_timeout(test_id: u16) {
    BOOT_TEST_FAIL_TOTAL.increment();

    if BOOT_TEST_PROVIDER.is_enabled() && crate::tracing::is_enabled() {
        crate::tracing::record_event(TEST_TIMEOUT, 0, test_id as u32);
    }
}

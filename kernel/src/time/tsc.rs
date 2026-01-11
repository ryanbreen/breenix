//! TSC (Time Stamp Counter) support for nanosecond-precision timing.
//!
//! This module provides a thin wrapper over the architecture-specific
//! timer implementation in the HAL. All actual TSC operations are
//! delegated to `crate::arch_impl::x86_64::timer`.

use crate::arch_impl::current::timer as hal_timer;

/// Read the Time Stamp Counter using RDTSC instruction.
///
/// Returns a 64-bit cycle count. On modern CPUs with invariant TSC,
/// this increments at a constant rate regardless of CPU frequency scaling.
#[inline(always)]
pub fn read_tsc() -> u64 {
    hal_timer::rdtsc()
}

/// Read TSC with serialization using LFENCE + RDTSC.
///
/// LFENCE ensures all previous loads have completed before reading the TSC,
/// providing more accurate timing for benchmarking. This is more portable
/// than RDTSCP which isn't available on all CPU models.
#[inline(always)]
pub fn read_tsc_serialized() -> u64 {
    hal_timer::rdtsc_serialized()
}

/// Calibrate TSC frequency using the PIT as a reference.
///
/// This function uses the PIT's known frequency to measure how many
/// TSC cycles occur in a fixed time period, allowing us to calculate
/// the TSC frequency.
///
/// Must be called after PIT is initialized but before interrupts are enabled.
pub fn calibrate() {
    hal_timer::calibrate()
}

/// Check if TSC has been calibrated
#[inline]
pub fn is_calibrated() -> bool {
    hal_timer::is_calibrated()
}

/// Get TSC frequency in Hz
#[inline]
pub fn frequency_hz() -> u64 {
    hal_timer::frequency_hz()
}

/// Get the current time in nanoseconds since TSC base was established.
///
/// This provides nanosecond-precision monotonic time based on TSC.
/// Returns None if TSC hasn't been calibrated yet.
#[inline]
pub fn nanoseconds_since_base() -> Option<u64> {
    hal_timer::nanoseconds_since_base()
}

/// Get high-resolution monotonic time as (seconds, nanoseconds).
///
/// Returns None if TSC hasn't been calibrated.
#[inline]
pub fn monotonic_time() -> Option<(u64, u64)> {
    hal_timer::monotonic_time()
}

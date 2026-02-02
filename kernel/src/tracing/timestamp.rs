//! Architecture-specific timestamp sources for tracing.
//!
//! This module provides a unified timestamp interface that uses:
//! - x86-64: RDTSC (Time Stamp Counter) - cycle-accurate, ~1ns precision
//! - ARM64: CNTVCT_EL0 (Virtual Counter) - cycle-accurate, ~10-40ns precision
//!
//! # Design Notes
//!
//! We use raw cycle counts rather than converted nanoseconds for several reasons:
//!
//! 1. **Performance**: Conversion requires division which is expensive in hot paths
//! 2. **Precision**: No loss of precision from integer division
//! 3. **Simplicity**: The raw value is sufficient for ordering and delta calculations
//! 4. **Post-processing**: Analysis tools can convert to nanoseconds offline
//!
//! # GDB Usage
//!
//! To convert timestamps to approximate nanoseconds in GDB:
//! - x86-64: `timestamp_ns = timestamp * 1000000000 / TSC_FREQUENCY_HZ`
//! - ARM64: `timestamp_ns = timestamp * 1000000000 / CNTFRQ_EL0`

/// Read the current timestamp from the CPU's high-resolution timer.
///
/// # Returns
///
/// A 64-bit timestamp value representing CPU cycles since some epoch.
/// The value is monotonically increasing on a single CPU.
///
/// # Architecture Details
///
/// - **x86-64**: Uses RDTSC instruction. The TSC increments at a constant rate
///   on modern CPUs with "invariant TSC" (most CPUs since ~2008).
///
/// - **ARM64**: Uses CNTVCT_EL0 (Virtual Counter). This is always accessible
///   from EL0 and provides a system-wide monotonic counter.
///
/// # Performance
///
/// - x86-64: ~20-25 cycles (RDTSC is very fast)
/// - ARM64: ~5-10 cycles (MRS is fast)
///
/// # Safety
///
/// This function is safe to call from any context, including:
/// - Interrupt handlers (no locks, no allocations)
/// - Context switch code
/// - Syscall entry/exit
#[inline(always)]
#[allow(dead_code)] // Used by record_event when tracing is integrated
pub fn trace_timestamp() -> u64 {
    #[cfg(target_arch = "x86_64")]
    {
        // Use RDTSC directly for minimal overhead
        // We don't need serialization (LFENCE) for tracing purposes
        // because small reordering is acceptable
        let low: u32;
        let high: u32;
        unsafe {
            core::arch::asm!(
                "rdtsc",
                out("eax") low,
                out("edx") high,
                options(nostack, nomem, preserves_flags)
            );
        }
        ((high as u64) << 32) | (low as u64)
    }

    #[cfg(target_arch = "aarch64")]
    {
        // Read the virtual counter (CNTVCT_EL0)
        // This is always accessible from any exception level
        let val: u64;
        unsafe {
            core::arch::asm!(
                "mrs {}, cntvct_el0",
                out(reg) val,
                options(nomem, nostack)
            );
        }
        val
    }

    // Fallback for other architectures (should not be reached)
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        0
    }
}

/// Get the frequency of the timestamp counter in Hz.
///
/// This is useful for converting raw timestamps to nanoseconds:
/// `nanoseconds = timestamp * 1_000_000_000 / frequency_hz()`
///
/// # Returns
///
/// The timestamp counter frequency in Hz, or 0 if unknown/uncalibrated.
///
/// # Architecture Details
///
/// - **x86-64**: Returns the TSC frequency (calibrated at boot via PIT).
///   Returns 0 if TSC hasn't been calibrated yet.
///
/// - **ARM64**: Returns CNTFRQ_EL0 (set by firmware, always available).
///   Typical values: 24 MHz (QEMU), 19.2 MHz (RPi4), 1 GHz (some platforms).
#[inline]
#[allow(dead_code)]
pub fn timestamp_frequency_hz() -> u64 {
    #[cfg(target_arch = "x86_64")]
    {
        use crate::arch_impl::current::timer as hal_timer;
        hal_timer::frequency_hz()
    }

    #[cfg(target_arch = "aarch64")]
    {
        // Read CNTFRQ_EL0 directly
        let freq: u64;
        unsafe {
            core::arch::asm!(
                "mrs {}, cntfrq_el0",
                out(reg) freq,
                options(nomem, nostack)
            );
        }
        freq
    }

    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        0
    }
}

/// Convert a timestamp delta to nanoseconds.
///
/// # Parameters
///
/// - `delta`: The difference between two timestamps (in cycles)
///
/// # Returns
///
/// The time difference in nanoseconds, or 0 if the frequency is unknown.
///
/// # Note
///
/// This function performs a 128-bit multiplication to avoid overflow.
/// For hot paths, prefer storing raw timestamps and converting offline.
#[inline]
#[allow(dead_code)]
pub fn timestamp_to_nanos(delta: u64) -> u64 {
    let freq = timestamp_frequency_hz();
    if freq == 0 {
        return 0;
    }

    // delta * 1_000_000_000 / freq
    // Use 128-bit arithmetic to avoid overflow
    let nanos = (delta as u128 * 1_000_000_000) / freq as u128;
    nanos as u64
}

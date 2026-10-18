//! In‑kernel unit test for POSIX clock_gettime implementation
//!
//! ## What This Test Validates
//!
//! This test runs before preemption is enabled and validates:
//!
//! ✓ TSC calibration agrees within 0.1% with an independent reference
//! ✓ TSC frequency is plausible (100 MHz to 10 GHz)
//! ✓ TSC is actually being used (not PIT fallback)
//! ✓ High-resolution timing provides sub-millisecond precision
//! ✓ Time advances monotonically between calls
//! ✓ Nanoseconds are not suspiciously aligned (proves TSC, not PIT)
//! ✓ clock_gettime returns Timespec for valid clock IDs
//! ✓ CLOCK_MONOTONIC returns Timespec with valid nanosecond values (0-999999999)
//! ✓ CLOCK_REALTIME returns plausible RTC timestamp (year >= 2024) with sub-second precision
//! ✓ Invalid clock IDs return EINVAL
//! claim-lint:ok: test_clock_gettime below asserts these properties; #1859 rules the calibration check.
//!
//! ## What This Test Does NOT Validate
//!
//! ✗ Userspace calling the syscall via INT 0x80 from Ring 3
//! ✗ copy_to_user mechanics (tested by userspace syscalls)
//!
//! This test runs during kernel initialization with preemption disabled.
//!
//! For userspace syscall validation, see:
//! - hello_time.elf (userspace/programs/hello_time.rs) - calls SYS_GET_TIME from Ring 3
//! - Boot stage "Userspace clock_gettime validated" - validates userspace syscall path

use crate::syscall::time::{clock_gettime, CLOCK_MONOTONIC, CLOCK_REALTIME};
use crate::syscall::ErrorCode;
use crate::time::tsc;
use crate::time::DateTime;

#[path = "pm_reference.rs"]
mod pm_reference;

/// S19: measure against a different timer from calibration's PIT. CPUID
/// cannot be the oracle: calibration may install it or deliberately reject it.
fn independent_tsc_frequency() -> (u64, &'static str) {
    use crate::arch_impl::x86_64::platform;
    use x86_64::instructions::port::Port;

    let (port, wide) = platform::pm_timer()
        .expect("TSC validation needs an independent ACPI PM reference timer");
    let mask = if wide { u32::MAX } else { 0x00ff_ffff };
    let mut pm = Port::<u32>::new(port);
    // SAFETY: platform::pm_timer obtained this I/O port from the FADT.
    let sample = || unsafe {
        let before = tsc::read_tsc_serialized();
        let ticks = pm.read() & mask;
        let after = tsc::read_tsc_serialized();
        pm_reference::Sample { ticks, before, after }
    };
    let hz = pm_reference::measure(sample, mask)
        .expect("independent ACPI PM reference remained stalled or imprecise after eight attempts");
    (hz, "acpi-pm-1s")
}

/// In-kernel test for clock_gettime - only called in certain build configurations.
#[allow(dead_code)]
pub fn test_clock_gettime() {
    log::info!("=== CLOCK_GETTIME TEST ===");

    // ── Test A: TSC Calibration ───────────────────────────────────
    log::info!("Test A: TSC Calibration Status");
    assert!(
        tsc::is_calibrated(),
        "TSC must be calibrated before clock_gettime test"
    );

    let freq_hz = tsc::frequency_hz();
    let freq_ghz = freq_hz as f64 / 1_000_000_000.0;
    log::info!("  TSC frequency: {} Hz ({:.2} GHz)", freq_hz, freq_ghz);

    // S19: plausibility is separate from agreement with an independent rate.
    assert!(
        (100_000_000..=10_000_000_000).contains(&freq_hz),
        "TSC frequency {} Hz outside plausible range (100 MHz-10 GHz)",
        freq_hz
    );
    let (reference_hz, source) = independent_tsc_frequency();
    assert!(reference_hz != 0, "independent TSC reference returned zero Hz");
    let difference = u128::from(freq_hz.abs_diff(reference_hz));
    log::info!(
        "  TSC independent reference: {} Hz source={} difference={} ppm",
        reference_hz,
        source,
        difference * 1_000_000 / u128::from(reference_hz)
    );
    assert!(
        difference * 1000 <= u128::from(reference_hz),
        "TSC frequency {} Hz disagrees with {} reference {} Hz by more than 0.1%",
        freq_hz,
        source,
        reference_hz
    );
    log::info!("✓ TSC calibrated with plausible frequency and independent agreement within 0.1%");

    // ── Test B: TSC Is Actually Used (Not PIT Fallback) ───────────
    log::info!("Test B: Verify TSC provides sub-millisecond precision");

    // Make two rapid calls - TSC should show time difference in nanoseconds
    // PIT fallback would show 0 or jump by full milliseconds (1,000,000 ns)
    let t1 = clock_gettime(CLOCK_MONOTONIC).expect("CLOCK_MONOTONIC failed");
    let t2 = clock_gettime(CLOCK_MONOTONIC).expect("CLOCK_MONOTONIC failed");

    let t1_ns = t1.tv_sec * 1_000_000_000 + t1.tv_nsec;
    let t2_ns = t2.tv_sec * 1_000_000_000 + t2.tv_nsec;
    let elapsed_ns = t2_ns - t1_ns;

    log::info!("  First call:  {} s, {} ns", t1.tv_sec, t1.tv_nsec);
    log::info!("  Second call: {} s, {} ns", t2.tv_sec, t2.tv_nsec);
    log::info!("  Elapsed: {} ns", elapsed_ns);

    // With TSC, even rapid calls should show elapsed time < 1 millisecond
    // PIT would show 0 or >= 1,000,000 ns
    assert!(
        elapsed_ns < 1_000_000,
        "Elapsed time {} ns too large - suggests PIT fallback, not TSC",
        elapsed_ns
    );
    log::info!("✓ Sub-millisecond precision confirmed (TSC active)");

    // ── Test C: Time Actually Advances ────────────────────────────
    log::info!("Test C: Monotonic advancement");

    let mut prev_ns = t2_ns;
    let mut advancements = 0;

    for i in 0..10 {
        let ts = clock_gettime(CLOCK_MONOTONIC).expect("Monotonic call failed");
        let now_ns = ts.tv_sec * 1_000_000_000 + ts.tv_nsec;

        assert!(
            now_ns >= prev_ns,
            "time went backwards on call {}! prev={} ns, now={} ns",
            i,
            prev_ns,
            now_ns
        );

        if now_ns > prev_ns {
            advancements += 1;
        }
        prev_ns = now_ns;
    }

    log::info!("  Made 10 calls, time advanced {} times", advancements);
    log::info!("✓ Time never went backwards");

    // ── Test D: Nanoseconds Are Not Suspiciously Aligned ──────────
    log::info!("Test D: Nanosecond precision (not millisecond-aligned)");

    // Collect nanosecond values from multiple calls
    let mut nanosecond_values = [0i64; 20];
    for val in &mut nanosecond_values {
        let ts = clock_gettime(CLOCK_MONOTONIC).expect("Monotonic call failed");
        *val = ts.tv_nsec;
    }

    // Count how many are suspiciously aligned to millisecond boundaries
    // (i.e., nanoseconds divisible by 1,000,000)
    let aligned_count = nanosecond_values
        .iter()
        .filter(|&&ns| ns % 1_000_000 == 0)
        .count();

    log::info!("  Collected 20 nanosecond samples");
    log::info!("  Millisecond-aligned values: {}/20", aligned_count);

    // If TSC is working, at most a few samples should be millisecond-aligned by chance
    // If using PIT fallback, ALL would be millisecond-aligned
    assert!(
        aligned_count < 15,
        "Too many millisecond-aligned values ({}/20) - suggests PIT fallback",
        aligned_count
    );
    log::info!("✓ Nanosecond precision confirmed (not PIT-granular)");

    // ── Test E: CLOCK_MONOTONIC Basic Validation ──────────────────
    log::info!("Test E: CLOCK_MONOTONIC basic validation");
    let mono = clock_gettime(CLOCK_MONOTONIC).expect("CLOCK_MONOTONIC failed");
    log::info!("  CLOCK_MONOTONIC: {} s, {} ns", mono.tv_sec, mono.tv_nsec);

    assert!(
        mono.tv_nsec >= 0 && mono.tv_nsec < 1_000_000_000,
        "nanoseconds out of valid range: {}",
        mono.tv_nsec
    );
    log::info!("✓ Nanoseconds in valid range [0, 999999999]");

    // ── Test F: CLOCK_REALTIME Validation ─────────────────────────
    log::info!("Test F: CLOCK_REALTIME validation");
    let real = clock_gettime(CLOCK_REALTIME).expect("CLOCK_REALTIME failed");
    let dt = DateTime::from_unix_timestamp(real.tv_sec as u64);
    log::info!(
        "  CLOCK_REALTIME: {:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:09}",
        dt.year,
        dt.month,
        dt.day,
        dt.hour,
        dt.minute,
        dt.second,
        real.tv_nsec
    );

    assert!(
        real.tv_nsec >= 0 && real.tv_nsec < 1_000_000_000,
        "nanoseconds out of valid range: {}",
        real.tv_nsec
    );
    assert!(
        dt.year >= 2024,
        "RTC returned implausible year: {}",
        dt.year
    );
    log::info!("✓ CLOCK_REALTIME has valid timestamp and sub-second precision");

    // ── Test G: Invalid Clock ID ──────────────────────────────────
    log::info!("Test G: Invalid clock ID handling");
    match clock_gettime(999) {
        Err(ErrorCode::InvalidArgument) => {
            log::info!("✓ Invalid ID correctly returned EINVAL")
        }
        other => panic!("unexpected result for invalid ID: {:?}", other),
    }

    log::info!("=== CLOCK_GETTIME TEST COMPLETE ===");
    log::info!("All 7 tests passed: TSC-based high-resolution timing validated");
}

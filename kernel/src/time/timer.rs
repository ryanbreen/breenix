//! Core tick-backed timer facilities.
//!
//! One tick is `MS_PER_TICK` milliseconds: on x86_64 the PIT below is
//! programmed at `PIT_HZ` = 200, so 5 ms; on aarch64 the interrupt that writes
//! `TICKS` is programmed at 1000 Hz, so 1 ms. Millisecond resolution is
//! therefore the tick period, not 1 ms on both.
//!
//! The PIT provides a fallback timer for systems where TSC is unavailable
//! or as a reference during TSC calibration. For high-precision timing,
//! use the TSC module directly.

use core::sync::atomic::{AtomicU64, Ordering};
#[cfg(target_arch = "x86_64")]
use x86_64::instructions::port::Port;

#[cfg(target_arch = "x86_64")]
const PIT_INPUT_FREQ_HZ: u32 = 1_193_182;
#[cfg(target_arch = "x86_64")]
const PIT_HZ: u32 = 200; // 200 Hz ⇒ 5 ms per tick
#[cfg(target_arch = "x86_64")]
const PIT_COMMAND_PORT: u16 = 0x43;
#[cfg(target_arch = "x86_64")]
const PIT_CHANNEL0_PORT: u16 = 0x40;

/// Milliseconds of elapsed time represented by one `TICKS` increment.
///
/// x86_64 programs the PIT at `PIT_HZ` above, so one tick is `1000 / PIT_HZ`
/// milliseconds there. On aarch64 the only writer of `TICKS` is CPU 0's arm of
/// `arch_impl::aarch64::timer_interrupt::timer_interrupt_handler`, whose timer
/// is programmed at `TARGET_TIMER_HZ` = 1000 Hz, so one tick is 1 ms there.
///
/// Public because the tick-to-millisecond relationship is what
/// `time_test::test_timer_resolution()` scores, and that check has to read the
/// same number the conversion uses rather than restating it.
#[cfg(target_arch = "x86_64")]
pub const MS_PER_TICK: u64 = 1_000 / PIT_HZ as u64;
#[cfg(not(target_arch = "x86_64"))]
pub const MS_PER_TICK: u64 = 1;

// A PIT_HZ that does not divide 1000 exactly would make MS_PER_TICK a
// truncated integer and put a rounding error into the milliseconds this
// module reports. Refuse to build in that case rather than round.
//
// Scope, so this is not read as more than it is: the assert covers the
// nominal 1000 / PIT_HZ division only. The divisor programmed into the PIT
// below truncates too -- 1_193_182 / 200 = 5965, so the hardware rate is
// 200.0305 Hz and one tick is 4.99924 ms, not 5 -- and that ~0.015% residual
// is outside what this assert can see. It also makes MS_PER_TICK == 0
// unrepresentable: `a % b == 0` with `b > a` requires `a == 0`, so a rate
// above 1000 Hz is rejected here rather than silently flooring to 0.
#[cfg(target_arch = "x86_64")]
const _: () = assert!(
    1_000 % PIT_HZ as u64 == 0,
    "PIT_HZ must divide 1000 exactly for MS_PER_TICK to be an exact factor"
);

/// Global monotonic tick counter: one increment per timer interrupt, worth
/// `MS_PER_TICK` milliseconds of elapsed time.
static TICKS: AtomicU64 = AtomicU64::new(0);

/// Counter for cursor blink timing (toggles every ~100 ticks = 500ms at 200Hz)
/// Only used when interactive feature is enabled.
#[cfg(feature = "interactive")]
static CURSOR_BLINK_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Cursor blink interval in ticks (100 ticks * 5ms = 500ms at 200Hz)
#[cfg(feature = "interactive")]
const CURSOR_BLINK_INTERVAL: u64 = 100;

/// Program the PIT to generate periodic interrupts at `PIT_HZ`.
#[cfg(target_arch = "x86_64")]
pub fn init() {
    let divisor: u16 = (PIT_INPUT_FREQ_HZ / PIT_HZ) as u16;
    unsafe {
        let mut cmd: Port<u8> = Port::new(PIT_COMMAND_PORT);
        let mut ch0: Port<u8> = Port::new(PIT_CHANNEL0_PORT);

        // Counter 0, lobyte/hibyte, mode 3 (square wave), binary
        cmd.write(0x36);

        // Divisor LSB then MSB
        ch0.write((divisor & 0xFF) as u8);
        ch0.write((divisor >> 8) as u8);
    }

    log::info!(
        "Timer initialized at {} Hz ({}ms per tick)",
        PIT_HZ,
        1000 / PIT_HZ
    );

    // Initialize RTC for wall clock time
    super::rtc::init();
}

#[cfg(not(target_arch = "x86_64"))]
pub fn init() {
    super::rtc::init();
}

/// Invoked from the CPU-side interrupt stub every 5 ms (at 200 Hz).
#[inline]
pub fn timer_interrupt() {
    TICKS.fetch_add(1, Ordering::Relaxed);

    // Cursor blink handling for interactive mode
    // This is kept minimal - just an atomic increment and comparison
    #[cfg(feature = "interactive")]
    {
        let count = CURSOR_BLINK_COUNTER.fetch_add(1, Ordering::Relaxed);
        if count >= CURSOR_BLINK_INTERVAL {
            CURSOR_BLINK_COUNTER.store(0, Ordering::Relaxed);
            // Toggle cursor - uses try_lock so won't block
            crate::logger::toggle_cursor_blink();
        }
    }
}

/// Raw tick counter.
#[inline]
pub fn get_ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

/// Milliseconds since the kernel was initialized, at `MS_PER_TICK` resolution.
///
/// For finer resolution, use `get_monotonic_time_ns()`, which reads the TSC
/// when it is calibrated. Monotonic, and does not wrap earlier than ~584
/// million years.
///
/// #767: the returned value used to be the raw tick count, which is
/// milliseconds only where a tick is 1 ms. It is scaled here, at the one
/// producer, rather than at the call sites that read it as milliseconds.
#[inline]
pub fn get_monotonic_time() -> u64 {
    get_ticks().saturating_mul(MS_PER_TICK)
}

/// Nanoseconds since the kernel was initialized (TSC-based, nanosecond resolution).
///
/// Falls back to the tick counter if the TSC is not calibrated, in which case
/// the resolution is `MS_PER_TICK` milliseconds rather than nanoseconds.
/// Returns (seconds, nanoseconds) tuple for POSIX timespec compatibility.
#[inline]
pub fn get_monotonic_time_ns() -> (u64, u64) {
    // Try TSC first for nanosecond precision
    if let Some((secs, nanos)) = super::tsc::monotonic_time() {
        return (secs, nanos);
    }

    // Fallback to the tick counter (MS_PER_TICK resolution)
    let ms = get_monotonic_time();
    (ms / 1000, (ms % 1000) * 1_000_000)
}

/// Validate that the PIT hardware is configured and counting
/// Returns (is_counting, count1, count2, description)
#[cfg(target_arch = "x86_64")]
#[allow(dead_code)] // Used in kernel_main_continue (conditionally compiled)
pub fn validate_pit_counting() -> (bool, u16, u16, &'static str) {
    unsafe {
        let mut ch0: Port<u8> = Port::new(PIT_CHANNEL0_PORT);
        let mut cmd: Port<u8> = Port::new(PIT_COMMAND_PORT);

        // Latch counter 0
        cmd.write(0x00);

        // Read low byte then high byte
        let low1 = ch0.read() as u16;
        let high1 = ch0.read() as u16;
        let count1 = (high1 << 8) | low1;

        // Wait a tiny bit (execute some instructions)
        for _ in 0..100 {
            core::hint::spin_loop();
        }

        // Latch counter 0 again
        cmd.write(0x00);

        // Read low byte then high byte
        let low2 = ch0.read() as u16;
        let high2 = ch0.read() as u16;
        let count2 = (high2 << 8) | low2;

        // The counter should be counting down, so count2 should be less than count1
        // (unless it wrapped, which is unlikely in such a short time)
        if count1 == 0 && count2 == 0 {
            return (
                false,
                count1,
                count2,
                "Counter reads as zero (not initialized?)",
            );
        }

        if count1 == count2 {
            return (false, count1, count2, "Counter not changing (not counting)");
        }

        // Counter is counting down, so we expect count2 < count1 (or wrapped)
        if count2 < count1 || count1 < 100 {
            return (true, count1, count2, "Counter is actively counting down");
        }

        // If count2 > count1, it might have wrapped or be counting wrong
        (true, count1, count2, "Counter changed (possibly wrapped)")
    }
}

#[cfg(not(target_arch = "x86_64"))]
#[allow(dead_code)] // Used in kernel_main_continue (conditionally compiled)
pub fn validate_pit_counting() -> (bool, u16, u16, &'static str) {
    (false, 0, 0, "PIT not supported on this architecture")
}

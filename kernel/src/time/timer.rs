//! Core PIT-backed timer facilities (10 Hz, 100 ms resolution).
//! NOTE: Temporarily reduced from 100 Hz to 10 Hz for debugging userspace execution.
//! The serial logging overhead causes timer interrupts to fire before userspace can execute.

use core::sync::atomic::{AtomicU64, Ordering};
use x86_64::instructions::port::Port;

const PIT_INPUT_FREQ_HZ: u32 = 1_193_182;
const PIT_HZ: u32 = 10; // 10 Hz ⇒ 100 ms per tick (reduced to allow userspace to execute)
const PIT_COMMAND_PORT: u16 = 0x43;
const PIT_CHANNEL0_PORT: u16 = 0x40;

/// Global monotonic tick counter (1 tick == 100 ms at 10 Hz).
static TICKS: AtomicU64 = AtomicU64::new(0);

/// Program the PIT to generate periodic interrupts at `PIT_HZ`.
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

    log::info!("Timer initialized at {} Hz ({}ms per tick)", PIT_HZ, 1000 / PIT_HZ);

    // Initialize RTC for wall clock time
    super::rtc::init();
}

/// Invoked from the CPU-side interrupt stub every 100 ms (at 10 Hz).
#[inline]
pub fn timer_interrupt() {
    TICKS.fetch_add(1, Ordering::Relaxed);
    // If the scheduler needs a tick hook, call it here.
    // crate::sched::timer_tick();
}

/// Raw tick counter.
#[inline]
pub fn get_ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

/// Milliseconds since the kernel was initialized.
///
/// Guaranteed monotonic and never wraps earlier than ~584 million years.
#[inline]
pub fn get_monotonic_time() -> u64 {
    // Convert ticks to milliseconds: 10 Hz = 100 ms per tick
    get_ticks() * (1000 / PIT_HZ as u64)
}

/// Validate that the PIT hardware is configured and counting
/// Returns (is_counting, count1, count2, description)
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
            return (false, count1, count2, "Counter reads as zero (not initialized?)");
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

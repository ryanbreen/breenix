//! Core PIT-backed timer facilities (1 kHz, 1 ms resolution).

use core::sync::atomic::{AtomicU64, Ordering};
use x86_64::instructions::port::Port;

const PIT_INPUT_FREQ_HZ: u32 = 1_193_182;
const PIT_HZ: u32 = 1000; // 1 kHz ⇒ 1 ms per tick
const PIT_COMMAND_PORT: u16 = 0x43;
const PIT_CHANNEL0_PORT: u16 = 0x40;

/// Global monotonic tick counter (1 tick == 1 ms).
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

    log::info!("Timer initialized at 1000 Hz (1ms per tick)");

    // Initialize RTC for wall clock time
    super::rtc::init();
}

/// Invoked from the CPU-side interrupt stub every 1 ms.
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
    get_ticks()
}

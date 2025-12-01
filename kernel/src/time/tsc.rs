//! TSC (Time Stamp Counter) support for nanosecond-precision timing.
//!
//! The TSC is a 64-bit register that counts CPU cycles. On modern x86_64
//! processors, it runs at a constant rate (invariant TSC) making it ideal
//! for high-resolution timing.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// TSC frequency in Hz (cycles per second), calibrated at boot
static TSC_FREQUENCY_HZ: AtomicU64 = AtomicU64::new(0);

/// Whether TSC calibration has completed
static TSC_CALIBRATED: AtomicBool = AtomicBool::new(false);

/// TSC value at the moment we started tracking time
static TSC_BASE: AtomicU64 = AtomicU64::new(0);

/// Read the Time Stamp Counter using RDTSC instruction.
///
/// Returns a 64-bit cycle count. On modern CPUs with invariant TSC,
/// this increments at a constant rate regardless of CPU frequency scaling.
#[inline(always)]
pub fn read_tsc() -> u64 {
    let low: u32;
    let high: u32;

    // RDTSC returns the 64-bit TSC in EDX:EAX
    // Using RDTSCP would also give us the processor ID, but RDTSC is sufficient
    // and more widely supported
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

/// Read TSC with serialization using LFENCE + RDTSC.
///
/// LFENCE ensures all previous loads have completed before reading the TSC,
/// providing more accurate timing for benchmarking. This is more portable
/// than RDTSCP which isn't available on all CPU models.
#[inline(always)]
pub fn read_tsc_serialized() -> u64 {
    let low: u32;
    let high: u32;

    unsafe {
        core::arch::asm!(
            "lfence",
            "rdtsc",
            out("eax") low,
            out("edx") high,
            options(nostack, nomem, preserves_flags)
        );
    }

    ((high as u64) << 32) | (low as u64)
}

/// Calibrate TSC frequency using the PIT as a reference.
///
/// This function uses the PIT's known frequency to measure how many
/// TSC cycles occur in a fixed time period, allowing us to calculate
/// the TSC frequency.
///
/// Must be called after PIT is initialized but before interrupts are enabled.
pub fn calibrate() {
    use x86_64::instructions::port::Port;

    const PIT_CHANNEL2_PORT: u16 = 0x42;
    const PIT_COMMAND_PORT: u16 = 0x43;
    const PIT_GATE_PORT: u16 = 0x61;

    // We'll measure TSC cycles over ~50ms using PIT channel 2
    // PIT countdown value for ~50ms: 1193182 * 0.05 = 59659
    const CALIBRATION_TICKS: u16 = 59659;
    const CALIBRATION_MS: u64 = 50;

    log::info!("Calibrating TSC frequency using PIT...");

    unsafe {
        let mut ch2: Port<u8> = Port::new(PIT_CHANNEL2_PORT);
        let mut cmd: Port<u8> = Port::new(PIT_COMMAND_PORT);
        let mut gate: Port<u8> = Port::new(PIT_GATE_PORT);

        // Save original gate state
        let orig_gate = gate.read();

        // Disable speaker, enable PIT channel 2 gate
        // Bit 0: Gate for channel 2 (1 = enable counting)
        // Bit 1: Speaker data enable (0 = disable speaker)
        gate.write((orig_gate & 0xFC) | 0x01);

        // Program PIT channel 2 for one-shot mode
        // 0xB0 = channel 2, lobyte/hibyte, mode 0 (interrupt on terminal count), binary
        cmd.write(0xB0);

        // Load the countdown value
        ch2.write((CALIBRATION_TICKS & 0xFF) as u8);
        ch2.write((CALIBRATION_TICKS >> 8) as u8);

        // Read initial TSC
        let tsc_start = read_tsc_serialized();

        // Reset the gate to start counting (toggle bit 0)
        let g = gate.read();
        gate.write(g & 0xFE); // Disable gate
        gate.write(g | 0x01); // Re-enable gate to start countdown

        // Wait for PIT channel 2 to count down to zero
        // When the count reaches 0, bit 5 of port 0x61 goes high
        loop {
            let status = gate.read();
            if (status & 0x20) != 0 {
                break;
            }
        }

        // Read final TSC
        let tsc_end = read_tsc_serialized();

        // Restore original gate state
        gate.write(orig_gate);

        // Calculate frequency
        let tsc_elapsed = tsc_end.saturating_sub(tsc_start);

        // TSC frequency = (tsc_elapsed / calibration_time_seconds)
        // = tsc_elapsed * 1000 / CALIBRATION_MS
        let frequency_hz = (tsc_elapsed * 1000) / CALIBRATION_MS;

        TSC_FREQUENCY_HZ.store(frequency_hz, Ordering::SeqCst);
        TSC_BASE.store(tsc_start, Ordering::SeqCst);
        TSC_CALIBRATED.store(true, Ordering::SeqCst);

        log::info!(
            "TSC calibration complete: {} MHz ({} Hz)",
            frequency_hz / 1_000_000,
            frequency_hz
        );
        log::info!(
            "TSC cycles during {}ms calibration: {}",
            CALIBRATION_MS,
            tsc_elapsed
        );
    }
}

/// Check if TSC has been calibrated
#[inline]
pub fn is_calibrated() -> bool {
    TSC_CALIBRATED.load(Ordering::SeqCst)
}

/// Get TSC frequency in Hz
#[inline]
pub fn frequency_hz() -> u64 {
    TSC_FREQUENCY_HZ.load(Ordering::SeqCst)
}

/// Get the current time in nanoseconds since TSC base was established.
///
/// This provides nanosecond-precision monotonic time based on TSC.
/// Returns None if TSC hasn't been calibrated yet.
#[inline]
pub fn nanoseconds_since_base() -> Option<u64> {
    if !is_calibrated() {
        return None;
    }

    let freq = TSC_FREQUENCY_HZ.load(Ordering::Relaxed);
    if freq == 0 {
        return None;
    }

    let base = TSC_BASE.load(Ordering::Relaxed);
    let current = read_tsc();
    let elapsed = current.saturating_sub(base);

    // Convert cycles to nanoseconds: (cycles * 1_000_000_000) / frequency
    // To avoid overflow, we split this calculation
    // elapsed_ns = elapsed * (1_000_000_000 / freq) would lose precision
    // Instead: elapsed_ns = (elapsed / freq) * 1_000_000_000 + ((elapsed % freq) * 1_000_000_000) / freq

    let seconds = elapsed / freq;
    let remainder_cycles = elapsed % freq;

    // For the remainder, we can safely multiply by 1B if remainder_cycles < freq
    // Since freq is typically 2-4 GHz, remainder_cycles * 1B could overflow
    // Use u128 for intermediate calculation
    let remainder_ns = ((remainder_cycles as u128) * 1_000_000_000) / (freq as u128);

    Some(seconds * 1_000_000_000 + remainder_ns as u64)
}

/// Get high-resolution monotonic time as (seconds, nanoseconds).
///
/// Returns None if TSC hasn't been calibrated.
#[inline]
pub fn monotonic_time() -> Option<(u64, u64)> {
    nanoseconds_since_base().map(|ns| {
        let secs = ns / 1_000_000_000;
        let nanos = ns % 1_000_000_000;
        (secs, nanos)
    })
}

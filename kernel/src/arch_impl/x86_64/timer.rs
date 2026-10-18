//! x86_64 timer operations using TSC.
//!
//! Implements the TimerOps trait using the Time Stamp Counter (TSC).
//! This module provides all x86_64-specific timer functionality including
//! TSC reading and PIT-based calibration.
//!
//! Note: This is part of the complete HAL API. Helper functions for TSC
//! operations are defined for API completeness.

#![allow(dead_code)] // HAL module - complete API for x86_64 timer operations

use crate::arch_impl::traits::TimerOps;
use core::arch::asm;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Cached TSC frequency in Hz (set during calibration).
static TSC_FREQUENCY_HZ: AtomicU64 = AtomicU64::new(0);

/// Whether TSC calibration has completed.
static TSC_CALIBRATED: AtomicBool = AtomicBool::new(false);

/// TSC value at the moment we started tracking time.
static TSC_BASE: AtomicU64 = AtomicU64::new(0);

// PIT (Programmable Interval Timer) ports for calibration
const PIT_CHANNEL2_PORT: u16 = 0x42;
const PIT_COMMAND_PORT: u16 = 0x43;
const PIT_GATE_PORT: u16 = 0x61;

/// x86_64 timer operations implementation.
pub struct X86Timer;

impl TimerOps for X86Timer {
    #[inline(always)]
    fn read_timestamp() -> u64 {
        rdtsc()
    }

    fn frequency_hz() -> Option<u64> {
        let freq = TSC_FREQUENCY_HZ.load(Ordering::Relaxed);
        if freq == 0 {
            None
        } else {
            Some(freq)
        }
    }

    fn ticks_to_nanos(ticks: u64) -> u64 {
        let freq = TSC_FREQUENCY_HZ.load(Ordering::Relaxed);
        if freq == 0 {
            // Fallback: assume 1 GHz if not calibrated
            ticks
        } else {
            // ticks * 1_000_000_000 / freq
            // Use 128-bit math to avoid overflow
            let nanos = (ticks as u128 * 1_000_000_000) / freq as u128;
            nanos as u64
        }
    }
}

/// Read the Time Stamp Counter.
#[inline(always)]
pub fn rdtsc() -> u64 {
    let low: u32;
    let high: u32;
    unsafe {
        asm!(
            "rdtsc",
            out("eax") low,
            out("edx") high,
            options(nostack, nomem, preserves_flags)
        );
    }
    ((high as u64) << 32) | (low as u64)
}

/// Read TSC with a preceding LFENCE for serialization.
/// Use this when you need a precise timestamp that doesn't
/// reorder with surrounding instructions.
#[inline(always)]
pub fn rdtsc_serialized() -> u64 {
    let low: u32;
    let high: u32;
    unsafe {
        asm!(
            "lfence",
            "rdtsc",
            out("eax") low,
            out("edx") high,
            options(nostack, nomem, preserves_flags)
        );
    }
    ((high as u64) << 32) | (low as u64)
}

/// Helper: write to an I/O port (x86-specific).
#[inline(always)]
unsafe fn outb(port: u16, value: u8) {
    asm!(
        "out dx, al",
        in("dx") port,
        in("al") value,
        options(nostack, preserves_flags)
    );
}

/// Helper: read from an I/O port (x86-specific).
#[inline(always)]
unsafe fn inb(port: u16) -> u8 {
    let value: u8;
    asm!(
        "in al, dx",
        out("al") value,
        in("dx") port,
        options(nostack, preserves_flags)
    );
    value
}

/// PIT input clock, in Hz.
const PIT_HZ: u64 = 1_193_182;

/// Calibrate TSC frequency using the PIT as a reference.
///
/// This function uses the PIT's known frequency to measure how many
/// TSC cycles occur in a fixed time period, allowing us to calculate
/// the TSC frequency.
///
/// Must be called after PIT is initialized but before interrupts are enabled.
pub fn calibrate() {
    // Under KVM the host's own TSC rate is read, not measured: a measurement
    // the host interrupts by stopping this vCPU across an edge has been left
    // 0.1 to 0.6% off, past the 1000 ppm the time suite allows (#1495).
    if let Some(frequency_hz) = kvmclock_tsc_hz() {
        TSC_FREQUENCY_HZ.store(frequency_hz, Ordering::SeqCst);
        TSC_BASE.store(rdtsc_serialized(), Ordering::SeqCst);
        TSC_CALIBRATED.store(true, Ordering::SeqCst);
        log::info!(
            "TSC calibration complete: {} MHz ({} Hz), from the KVM paravirtual clock",
            frequency_hz / 1_000_000,
            frequency_hz
        );
        log::info!("HAL_TIMER_CALIBRATED: TSC calibration via HAL complete");
        return;
    }

    // PIT channel 2 runs as a square wave of ~50 ms periods, and the TSC times
    // its rising edges. An edge is seen between the last poll that read OUT low
    // and the first that read it high; a port access is a VM exit of tens of
    // microseconds on the x86 gate, so one edge is known to within one poll.
    // Timing whole periods between two edges found the same way cancels the
    // exit's own latency, and spreading the edges five or six periods apart
    // divides the remaining error by the span: a single 50 ms countdown timed
    // from its starting write left the kernel up to 1100 ppm off the host.
    const PERIOD_TICKS: u16 = 59660;
    const MIN_PERIODS: usize = 5;
    // Collections refused for a polling gap before calibration gives up.
    const ATTEMPTS: u32 = 8;

    log::info!("Calibrating TSC frequency using PIT...");

    // The edges are counted, not the periods between them, so the collection
    // is only usable if no rising edge went unseen. OUT stays low for half a
    // period, and an edge is missed only when no poll falls inside that low
    // half, so an interval that hides an edge holds a gap over half a period.
    // Requiring the longest gap after the first edge to stay under a quarter
    // of the shortest interval therefore refuses any collection in which one
    // interval missed an edge while another did not, and any in which every
    // interval missed exactly one; it cannot see every interval missing two
    // or more. A refused collection, whose virtual CPU was descheduled for a
    // long gap, is taken again.
    let mut attempt = 0;
    let edges = loop {
        attempt += 1;
        let Some((edges, max_gap)) = (unsafe { pit_rising_edges(PERIOD_TICKS) }) else {
            panic!("TSC calibration observed no PIT channel 2 square wave");
        };
        let shortest = edges.windows(2).map(|pair| pair[1].at - pair[0].at).min().unwrap_or(0);
        if max_gap.saturating_mul(4) < shortest {
            break edges;
        }
        log::warn!(
            "TSC calibration attempt {} refused: a {}-cycle gap between PIT polls could hide an edge (shortest interval {})",
            attempt,
            max_gap,
            shortest
        );
        if attempt == ATTEMPTS {
            panic!("TSC calibration could not observe every PIT edge in {} attempts", ATTEMPTS);
        }
    };

    // The two edges at least MIN_PERIODS apart whose times are known most
    // tightly: a virtual CPU descheduled across an edge widens that edge.
    let mut best = (0, MIN_PERIODS);
    for first in 0..edges.len() {
        for last in first + MIN_PERIODS..edges.len() {
            let width = edges[first].width + edges[last].width;
            if width < edges[best.0].width + edges[best.1].width {
                best = (first, last);
            }
        }
    }
    let (first, last) = (edges[best.0], edges[best.1]);
    let periods = (best.1 - best.0) as u64;
    let tsc_elapsed = last.at - first.at;
    let pit_ticks = periods * PERIOD_TICKS as u64;
    let measured_hz = tsc_elapsed * PIT_HZ / pit_ticks;

    // The rate the processor states (CPUID 15H/16H) is taken when the PIT
    // measurement agrees with it; otherwise the measurement is the rate.
    let stated = super::tsc_rate::stated(cpuid_leaves());
    let chosen = super::tsc_rate::choose(measured_hz, stated);
    let frequency_hz = chosen.hz;

    TSC_FREQUENCY_HZ.store(frequency_hz, Ordering::SeqCst);
    TSC_BASE.store(first.at, Ordering::SeqCst);
    TSC_CALIBRATED.store(true, Ordering::SeqCst);

    log::info!(
        "TSC calibration complete: {} MHz ({} Hz)",
        frequency_hz / 1_000_000,
        frequency_hz
    );
    log::info!(
        "TSC cycles over {} PIT periods ({} ms): {} (within {})",
        periods,
        pit_ticks * 1000 / PIT_HZ,
        tsc_elapsed,
        first.width + last.width
    );
    match stated {
        Some(stated) => log::info!(
            "[tsc] pit={} Hz {}={} Hz apart={} ppm using={}",
            measured_hz,
            stated.source,
            stated.hz,
            super::tsc_rate::ppm_apart(stated.hz, measured_hz),
            chosen.source
        ),
        None => log::info!("[tsc] pit={} Hz cpuid=none using={}", measured_hz, chosen.source),
    }
    if let Some((port, wide)) = super::platform::pm_timer().filter(|_| !crate::drivers::fw_cfg::present()) {
        match unsafe { pm_timer_rate(port, wide, measured_hz) } {
            Some(by_pm) => log::info!(
                "[tsc] pm_timer={} Hz apart={} ppm from the rate in use",
                by_pm,
                super::tsc_rate::ppm_apart(by_pm, frequency_hz)
            ),
            None => log::info!("[tsc] pm_timer did not count"),
        }
    }

    // HAL boot stage marker - proves HAL timer operations are working
    log::info!("HAL_TIMER_CALIBRATED: TSC calibration via HAL complete");
}

/// The TSC frequency, in Hz, that KVM's paravirtual clock reports when this
/// kernel runs as a KVM guest offering one: the rate the host calibrated and
/// converts this vCPU's TSC with, which Linux also takes for a KVM guest's
/// TSC rate. None elsewhere, or when the host never fills the structure.
fn kvmclock_tsc_hz() -> Option<u64> {
    use core::arch::x86_64::__cpuid;
    use x86_64::registers::model_specific::Msr;

    // CPUID 0x40000000 names the hypervisor: "KVMKVMKVM\0\0\0".
    // SAFETY: CPUID is available on every x86_64 processor.
    let signature = unsafe { __cpuid(0x4000_0000) };
    if [signature.ebx, signature.ecx, signature.edx] != [0x4b4d_564b, 0x564b_4d56, 0x0000_004d]
        || signature.eax < 0x4000_0001
    {
        return None;
    }
    // SAFETY: as above; the leaf exists, since the maximum leaf covers it.
    let features = unsafe { __cpuid(0x4000_0001) }.eax;
    let msr = if features & (1 << 3) != 0 {
        0x4b56_4d01 // MSR_KVM_SYSTEM_TIME_NEW (KVM_FEATURE_CLOCKSOURCE2)
    } else if features & 1 != 0 {
        0x12 // MSR_KVM_SYSTEM_TIME (KVM_FEATURE_CLOCKSOURCE)
    } else {
        return None;
    };

    // The host fills a struct pvclock_vcpu_time_info at the physical address
    // the MSR names: version (u32) at 0, tsc_to_system_mul (u32) at 24 and
    // tsc_shift (i8) at 28. A version that is odd, or that changes across the
    // read, is an update in progress.
    let frame = crate::memory::frame_allocator::allocate_frame()?;
    let physical = frame.start_address().as_u64();
    let info = (crate::memory::physical_memory_offset().as_u64() + physical) as *mut u8;
    let mut register = Msr::new(msr);
    // SAFETY: `info` maps the frame just allocated, which nothing else uses,
    // and enabling the clock only makes the host write that frame.
    unsafe {
        core::ptr::write_bytes(info, 0, 32);
        register.write(physical | 1);
    }
    let mut frequency_hz = None;
    for _ in 0..1000 {
        // SAFETY: as above; the reads are of the frame's first 32 bytes.
        let (version, mul, shift, again) = unsafe {
            let version = core::ptr::read_volatile(info as *const u32);
            core::sync::atomic::fence(Ordering::Acquire);
            let mul = core::ptr::read_volatile(info.add(24) as *const u32);
            let shift = core::ptr::read_volatile(info.add(28) as *const i8);
            core::sync::atomic::fence(Ordering::Acquire);
            (version, mul, shift, core::ptr::read_volatile(info as *const u32))
        };
        if version != 0 && version & 1 == 0 && version == again && mul != 0 {
            // ns = ((tsc shifted by tsc_shift) * mul) >> 32.
            let mut hz = (1_000_000_000u128 << 32) / u128::from(mul);
            if shift < 0 {
                hz <<= u32::from(shift.unsigned_abs());
            } else {
                hz >>= u32::from(shift.unsigned_abs());
            }
            frequency_hz = u64::try_from(hz).ok();
            break;
        }
        core::hint::spin_loop();
    }
    // SAFETY: disabling the clock stops the host's writes to the frame before
    // it is freed.
    unsafe { register.write(0) };
    crate::memory::frame_allocator::deallocate_frame(frame);
    frequency_hz
}

/// CPUID leaves 0, 15H and 16H as `tsc_rate` reads them.
fn cpuid_leaves() -> super::tsc_rate::Leaves {
    use core::arch::x86_64::__cpuid;
    // SAFETY: leaf 0 is architectural on x86-64, and each further leaf is read
    // only when leaf 0 says the processor implements it.
    unsafe {
        let max_leaf = __cpuid(0).eax;
        let (ratio_denominator, ratio_numerator, crystal_hz) = if max_leaf >= 0x15 {
            let leaf = __cpuid(0x15);
            (leaf.eax, leaf.ebx, leaf.ecx)
        } else {
            (0, 0, 0)
        };
        super::tsc_rate::Leaves {
            max_leaf,
            ratio_denominator,
            ratio_numerator,
            crystal_hz,
            base_mhz: if max_leaf >= 0x16 { __cpuid(0x16).eax } else { 0 },
        }
    }
}

/// Platform-reported TSC rate for the boot clock calibration check.
pub(crate) fn reported_tsc_frequency() -> Option<super::tsc_rate::Stated> {
    use core::arch::x86_64::__cpuid;
    use super::tsc_rate::Stated;

    if let Some(rate) = super::tsc_rate::stated(cpuid_leaves()) {
        return Some(rate);
    }
    // VMware's timing leaf reports the virtual TSC rate in kHz.
    // SAFETY: CPUID is architectural on x86-64; check the signature and
    // maximum hypervisor leaf before interpreting the timing leaf.
    unsafe {
        let hypervisor = __cpuid(0x4000_0000);
        if [hypervisor.ebx, hypervisor.ecx, hypervisor.edx]
            == [0x6177_4d56, 0x4d56_6572, 0x6572_6177]
            && hypervisor.eax >= 0x4000_0010
        {
            let khz = __cpuid(0x4000_0010).eax;
            if khz != 0 {
                return Some(Stated { hz: u64::from(khz) * 1000, source: "hypervisor-timing" });
            }
        }
    }
    None
}

/// The ACPI power-management timer's fixed rate, in Hz (ACPI 6.5, 4.8.3.3).
const PM_TIMER_HZ: u64 = 3_579_545;

/// The TSC rate the PM timer at `port` implies over about 50 ms, timed with
/// the PIT-measured rate `measured_hz`. Each PM timer read is bracketed by TSC
/// reads and placed at their midpoint. No value when the timer did not advance.
unsafe fn pm_timer_rate(port: u16, wide: bool, measured_hz: u64) -> Option<u64> {
    use x86_64::instructions::port::Port;
    let mask: u32 = if wide { u32::MAX } else { 0x00ff_ffff };
    let mut timer = Port::<u32>::new(port);
    let mut sample = || {
        let before = rdtsc_serialized();
        let value = timer.read() & mask;
        let after = rdtsc_serialized();
        (value, before + (after - before) / 2)
    };
    let (start, start_at) = sample();
    let deadline = start_at + measured_hz / 20;
    while rdtsc() < deadline {
        core::hint::spin_loop();
    }
    let (end, end_at) = sample();
    // One wrap at most: a 24-bit timer wraps after 4.7 s.
    let counted = u64::from(end.wrapping_sub(start) & mask);
    (counted != 0).then(|| (end_at - start_at) * PM_TIMER_HZ / counted)
}

/// A rising edge of PIT channel 2's OUT, timed by the TSC.
#[derive(Clone, Copy, Default)]
struct PitEdge {
    /// Middle of the poll in which OUT rose.
    at: u64,
    /// TSC cycles between the reads either side of the edge.
    width: u64,
}

/// Run PIT channel 2 as a square wave of `period` ticks and time its first
/// seven rising edges, with the longest TSC gap between consecutive polls from
/// the first edge on, or `None` when OUT stopped toggling.
unsafe fn pit_rising_edges(period: u16) -> Option<([PitEdge; 7], u64)> {
    // Bounds the wait for one edge far above a period's worth of polls, so a
    // PIT that never toggles fails calibration instead of hanging the boot.
    const POLLS_PER_EDGE: u64 = 50_000_000;

    // Save original gate state
    let orig_gate = inb(PIT_GATE_PORT);

    // Disable speaker, enable PIT channel 2 gate
    // Bit 0: Gate for channel 2 (1 = enable counting)
    // Bit 1: Speaker data enable (0 = disable speaker)
    outb(PIT_GATE_PORT, (orig_gate & 0xFC) | 0x01);

    // 0xB6 = channel 2, lobyte/hibyte, mode 3 (square wave), binary
    outb(PIT_COMMAND_PORT, 0xB6);
    outb(PIT_CHANNEL2_PORT, (period & 0xFF) as u8);
    outb(PIT_CHANNEL2_PORT, (period >> 8) as u8);

    // Bit 5 of port 0x61 is channel 2's OUT.
    let mut edges = [PitEdge::default(); 7];
    let mut found = 0;
    let mut was_high = inb(PIT_GATE_PORT) & 0x20 != 0;
    let mut last = rdtsc_serialized();
    let mut polls = 0u64;
    let mut max_gap = 0u64;
    while found < edges.len() && polls < POLLS_PER_EDGE {
        let high = inb(PIT_GATE_PORT) & 0x20 != 0;
        let at = rdtsc_serialized();
        if found > 0 {
            max_gap = max_gap.max(at - last);
        }
        if high && !was_high {
            edges[found] = PitEdge { at: last + (at - last) / 2, width: at - last };
            found += 1;
            polls = 0;
        }
        was_high = high;
        last = at;
        polls += 1;
    }

    // Restore original gate state
    outb(PIT_GATE_PORT, orig_gate);

    (found == edges.len()).then_some((edges, max_gap))
}

/// Check if TSC has been calibrated.
#[inline]
pub fn is_calibrated() -> bool {
    TSC_CALIBRATED.load(Ordering::SeqCst)
}

/// Set the TSC frequency (called during calibration).
pub fn set_tsc_frequency(freq_hz: u64) {
    TSC_FREQUENCY_HZ.store(freq_hz, Ordering::Relaxed);
}

/// Get the TSC frequency in Hz, or None if not yet calibrated.
pub fn get_tsc_frequency() -> Option<u64> {
    X86Timer::frequency_hz()
}

/// Get the TSC frequency in Hz directly.
#[inline]
pub fn frequency_hz() -> u64 {
    TSC_FREQUENCY_HZ.load(Ordering::SeqCst)
}

/// Calculate nanoseconds from TSC ticks.
#[inline(always)]
pub fn tsc_to_nanos(ticks: u64) -> u64 {
    X86Timer::ticks_to_nanos(ticks)
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
    let current = rdtsc();
    let elapsed = current.saturating_sub(base);

    // Convert cycles to nanoseconds: (cycles * 1_000_000_000) / frequency
    // To avoid overflow, we split this calculation
    let seconds = elapsed / freq;
    let remainder_cycles = elapsed % freq;

    // For the remainder, use u128 for intermediate calculation
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

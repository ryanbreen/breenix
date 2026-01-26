//! ARM64 Timer Interrupt Handler
//!
//! This module provides the timer interrupt handler for ARM64, integrating
//! with the scheduler for preemptive multitasking.
//!
//! The ARM64 Generic Timer (CNTP_EL1 or CNTV_EL0) provides periodic interrupts.
//! Unlike x86_64 which uses the PIC/APIC, ARM64 uses the GIC (Generic Interrupt
//! Controller) to route timer interrupts.
//!
//! Timer Interrupt Flow:
//! 1. Timer fires (IRQ 27 = virtual timer PPI)
//! 2. GIC routes interrupt to handle_irq()
//! 3. handle_irq() calls timer_interrupt_handler()
//! 4. Handler updates time, checks quantum, sets need_resched
//! 5. On exception return, check need_resched and perform context switch if needed

use crate::task::scheduler;
use core::sync::atomic::{AtomicU32, Ordering};

/// Virtual timer interrupt ID (PPI 27)
pub const TIMER_IRQ: u32 = 27;

/// Time quantum in timer ticks (10 ticks = ~50ms at 200Hz)
const TIME_QUANTUM: u32 = 10;

/// Timer frequency for scheduling (target: 200 Hz = 5ms per tick)
/// This is calculated based on CNTFRQ and desired interrupt rate
const TIMER_TICKS_PER_INTERRUPT: u64 = 120_000; // For 24MHz clock = ~5ms

/// Current thread's remaining time quantum
static CURRENT_QUANTUM: AtomicU32 = AtomicU32::new(TIME_QUANTUM);

/// Whether the timer is initialized
static TIMER_INITIALIZED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Raw serial output for debugging (no locks, minimal overhead)
#[inline(always)]
fn raw_serial_char(c: u8) {
    const PL011_DR: *mut u32 = 0x0900_0000 as *mut u32;
    unsafe {
        core::ptr::write_volatile(PL011_DR, c as u32);
    }
}

/// Initialize the timer interrupt system
///
/// Sets up the virtual timer to fire periodically for scheduling.
pub fn init() {
    if TIMER_INITIALIZED.load(Ordering::Relaxed) {
        return;
    }

    // Get the timer frequency
    let freq = super::timer::frequency_hz();
    log::info!("ARM64 timer interrupt init: frequency = {} Hz", freq);

    // Calculate ticks per interrupt for ~200 Hz scheduling rate
    // For 24 MHz clock: 24_000_000 / 200 = 120_000 ticks
    let ticks_per_interrupt = if freq > 0 {
        freq / 200 // 200 Hz = 5ms intervals
    } else {
        TIMER_TICKS_PER_INTERRUPT
    };

    log::info!(
        "Timer configured for ~200 Hz ({} ticks per interrupt)",
        ticks_per_interrupt
    );

    // Arm the timer for the first interrupt
    arm_timer(ticks_per_interrupt);

    // Enable the timer interrupt in the GIC
    use crate::arch_impl::aarch64::gic;
    use crate::arch_impl::traits::InterruptController;
    gic::Gicv2::enable_irq(TIMER_IRQ as u8);

    TIMER_INITIALIZED.store(true, Ordering::Release);
    log::info!("ARM64 timer interrupt initialized");
}

/// Arm the virtual timer to fire after `ticks` counter increments
fn arm_timer(ticks: u64) {
    unsafe {
        // Set countdown value (CNTV_TVAL_EL0)
        core::arch::asm!(
            "msr cntv_tval_el0, {}",
            in(reg) ticks,
            options(nomem, nostack)
        );
        // Enable timer with interrupts (CNTV_CTL_EL0)
        // Bit 0 = ENABLE, Bit 1 = IMASK (0 = interrupt enabled)
        core::arch::asm!("msr cntv_ctl_el0, {}", in(reg) 1u64, options(nomem, nostack));
    }
}

/// Timer interrupt handler - minimal work in interrupt context
///
/// This is called from handle_irq() when IRQ 27 (virtual timer) fires.
/// It performs the absolute minimum work:
/// 1. Re-arm the timer for the next interrupt
/// 2. Update global time
/// 3. Decrement time quantum
/// 4. Set need_resched if quantum expired
#[no_mangle]
pub extern "C" fn timer_interrupt_handler() {
    // Debug marker: 'T' for timer interrupt
    raw_serial_char(b'T');

    // Enter IRQ context (increment HARDIRQ count)
    crate::per_cpu_aarch64::irq_enter();

    // Re-arm the timer for the next interrupt
    let freq = super::timer::frequency_hz();
    let ticks_per_interrupt = if freq > 0 { freq / 200 } else { TIMER_TICKS_PER_INTERRUPT };
    arm_timer(ticks_per_interrupt);

    // Update global time (single atomic operation)
    crate::time::timer_interrupt();

    // Decrement quantum and check for reschedule
    let old_quantum = CURRENT_QUANTUM.fetch_sub(1, Ordering::Relaxed);
    if old_quantum <= 1 {
        // Quantum expired - request reschedule
        scheduler::set_need_resched();
        CURRENT_QUANTUM.store(TIME_QUANTUM, Ordering::Relaxed);
        raw_serial_char(b'!'); // Debug: quantum expired
    }

    // Exit IRQ context (decrement HARDIRQ count)
    crate::per_cpu_aarch64::irq_exit();
}

/// Reset the quantum counter (called when switching threads)
pub fn reset_quantum() {
    CURRENT_QUANTUM.store(TIME_QUANTUM, Ordering::Relaxed);
}

/// Check if the timer is initialized
pub fn is_initialized() -> bool {
    TIMER_INITIALIZED.load(Ordering::Acquire)
}

/// Get the current quantum value (for debugging)
#[allow(dead_code)]
pub fn current_quantum() -> u32 {
    CURRENT_QUANTUM.load(Ordering::Relaxed)
}

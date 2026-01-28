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
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// Virtual timer interrupt ID (PPI 27)
pub const TIMER_IRQ: u32 = 27;

/// Time quantum in timer ticks (10 ticks = ~50ms at 200Hz)
const TIME_QUANTUM: u32 = 10;

/// Default timer ticks per interrupt (fallback for 24MHz clock)
/// This value is overwritten at init() with the dynamically calculated value
const DEFAULT_TICKS_PER_INTERRUPT: u64 = 120_000; // For 24MHz clock = ~5ms

/// Target timer frequency in Hz (200 Hz = 5ms per interrupt)
const TARGET_TIMER_HZ: u64 = 200;

/// Dynamically calculated ticks per interrupt based on actual timer frequency
/// Set during init() and used by the interrupt handler for consistent timing
static TICKS_PER_INTERRUPT: AtomicU64 = AtomicU64::new(DEFAULT_TICKS_PER_INTERRUPT);

/// Current thread's remaining time quantum
static CURRENT_QUANTUM: AtomicU32 = AtomicU32::new(TIME_QUANTUM);

/// Whether the timer is initialized
static TIMER_INITIALIZED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Total timer interrupt count (for frequency verification)
static TIMER_INTERRUPT_COUNT: AtomicU64 = AtomicU64::new(0);

#[cfg(feature = "boot_tests")]
static RESET_QUANTUM_CALL_COUNT: AtomicU64 = AtomicU64::new(0);

/// Interval for printing timer count (every N interrupts for frequency verification)
/// Printing on every interrupt adds overhead; reduce frequency for more accurate measurement
/// At 200 Hz: print interval 200 = print once per second
const TIMER_COUNT_PRINT_INTERVAL: u64 = 200;

/// Initialize the timer interrupt system
///
/// Sets up the virtual timer to fire periodically for scheduling.
pub fn init() {
    if TIMER_INITIALIZED.load(Ordering::Relaxed) {
        return;
    }

    // Get the timer frequency from hardware
    let freq = super::timer::frequency_hz();
    log::info!("ARM64 timer interrupt init: frequency = {} Hz", freq);

    // Calculate ticks per interrupt for target Hz scheduling rate
    // For 62.5 MHz clock: 62_500_000 / 200 = 312_500 ticks
    // For 24 MHz clock: 24_000_000 / 200 = 120_000 ticks
    let ticks_per_interrupt = if freq > 0 {
        freq / TARGET_TIMER_HZ
    } else {
        DEFAULT_TICKS_PER_INTERRUPT
    };

    // Store the calculated value for use in the interrupt handler
    TICKS_PER_INTERRUPT.store(ticks_per_interrupt, Ordering::Release);

    crate::serial_println!(
        "[timer] Timer configured for ~{} Hz ({} ticks per interrupt)",
        TARGET_TIMER_HZ,
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
/// 3. Poll keyboard input (VirtIO doesn't use interrupts on ARM64)
/// 4. Decrement time quantum
/// 5. Set need_resched if quantum expired
#[no_mangle]
pub extern "C" fn timer_interrupt_handler() {
    // Enter IRQ context (increment HARDIRQ count)
    crate::per_cpu_aarch64::irq_enter();

    // Re-arm the timer for the next interrupt using the dynamically calculated value
    // This ensures consistent timing regardless of the actual timer frequency
    // (62.5 MHz on cortex-a72, 1 GHz on 'max' CPU, 24 MHz on some platforms)
    arm_timer(TICKS_PER_INTERRUPT.load(Ordering::Relaxed));

    // Update global time (single atomic operation)
    crate::time::timer_interrupt();

    // Increment timer interrupt counter (used for debugging when needed)
    let _count = TIMER_INTERRUPT_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    // Note: [TIMER_COUNT:N] output disabled - interrupt handlers must be minimal
    // To enable: uncomment and rebuild with TIMER_COUNT_PRINT_INTERVAL check

    // Poll VirtIO keyboard and push to stdin
    // VirtIO MMIO devices don't generate interrupts on ARM64 virt machine,
    // so we poll during timer tick to get keyboard input to userspace
    poll_keyboard_to_stdin();

    // Decrement quantum and check for reschedule
    let old_quantum = CURRENT_QUANTUM.fetch_sub(1, Ordering::Relaxed);
    if old_quantum <= 1 {
        // Quantum expired - request reschedule
        scheduler::set_need_resched();
        CURRENT_QUANTUM.store(TIME_QUANTUM, Ordering::Relaxed);
    }

    // Exit IRQ context (decrement HARDIRQ count)
    crate::per_cpu_aarch64::irq_exit();
}

/// Raw serial output - no locks, single char for debugging (used by print_timer_count)
#[inline(always)]
fn raw_serial_char(c: u8) {
    crate::serial_aarch64::raw_serial_char(c);
}

/// Raw serial output - write a string without locks for debugging
#[inline(always)]
fn raw_serial_str(s: &[u8]) {
    crate::serial_aarch64::raw_serial_str(s);
}

/// Print a decimal number using raw serial output
/// Used by timer interrupt handler to output [TIMER_COUNT:N] markers
fn print_timer_count_decimal(count: u64) {
    if count == 0 {
        raw_serial_char(b'0');
    } else {
        // Convert to decimal digits (max u64 is 20 digits)
        let mut digits = [0u8; 20];
        let mut n = count;
        let mut i = 0;
        while n > 0 {
            digits[i] = (n % 10) as u8 + b'0';
            n /= 10;
            i += 1;
        }
        // Print in reverse order
        while i > 0 {
            i -= 1;
            raw_serial_char(digits[i]);
        }
    }
}

/// Poll VirtIO keyboard and push characters to stdin buffer
///
/// This allows keyboard input to reach userspace processes that call read(0, ...)
fn poll_keyboard_to_stdin() {
    use crate::drivers::virtio::input_mmio::{self, event_type};

    // Track shift state across calls
    static SHIFT_PRESSED: core::sync::atomic::AtomicBool =
        core::sync::atomic::AtomicBool::new(false);

    if !input_mmio::is_initialized() {
        return;
    }

    for event in input_mmio::poll_events() {
        if event.event_type == event_type::EV_KEY {
            let keycode = event.code;
            let pressed = event.value != 0;

            // Track shift key state
            if input_mmio::is_shift(keycode) {
                SHIFT_PRESSED.store(pressed, core::sync::atomic::Ordering::Relaxed);
                continue;
            }

            // Only process key presses (not releases)
            if pressed {
                let shift = SHIFT_PRESSED.load(core::sync::atomic::Ordering::Relaxed);
                if let Some(c) = input_mmio::keycode_to_char(keycode, shift) {
                    // Debug marker: VirtIO key event -> stdin
                    raw_serial_str(b"[VIRTIO_KEY]");
                    // Push to stdin buffer so userspace can read it
                    crate::ipc::stdin::push_byte_from_irq(c as u8);
                }
            }
        }
    }
}

/// Reset the quantum counter (called when switching threads)
pub fn reset_quantum() {
    #[cfg(feature = "boot_tests")]
    RESET_QUANTUM_CALL_COUNT.fetch_add(1, Ordering::SeqCst);
    CURRENT_QUANTUM.store(TIME_QUANTUM, Ordering::Relaxed);
}

/// Get reset_quantum() call count for tests.
#[cfg(feature = "boot_tests")]
pub fn reset_quantum_call_count() -> u64 {
    RESET_QUANTUM_CALL_COUNT.load(Ordering::SeqCst)
}

/// Reset reset_quantum() call count for tests.
#[cfg(feature = "boot_tests")]
pub fn reset_quantum_call_count_reset() {
    RESET_QUANTUM_CALL_COUNT.store(0, Ordering::SeqCst);
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

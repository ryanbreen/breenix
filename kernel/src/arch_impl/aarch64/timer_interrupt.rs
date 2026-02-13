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
use crate::tracing::providers::irq::trace_timer_tick;
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

/// Per-CPU time quantum counters.
/// Each CPU decrements its own quantum independently.
static CURRENT_QUANTUM: [AtomicU32; crate::arch_impl::aarch64::constants::MAX_CPUS] = [
    AtomicU32::new(TIME_QUANTUM),
    AtomicU32::new(TIME_QUANTUM),
    AtomicU32::new(TIME_QUANTUM),
    AtomicU32::new(TIME_QUANTUM),
    AtomicU32::new(TIME_QUANTUM),
    AtomicU32::new(TIME_QUANTUM),
    AtomicU32::new(TIME_QUANTUM),
    AtomicU32::new(TIME_QUANTUM),
];

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
/// Each CPU fires its own timer (PPI 27 is per-CPU). The handler:
/// 1. Re-arms the timer for the next interrupt
/// 2. CPU 0 only: updates global wall clock time
/// 3. CPU 0 only: polls keyboard input
/// 4. All CPUs: decrements per-CPU time quantum
/// 5. CPU 0 only: sets need_resched if quantum expired (Phase 2: only CPU 0 schedules)
#[no_mangle]
pub extern "C" fn timer_interrupt_handler() {
    // Enter IRQ context (increment HARDIRQ count)
    crate::per_cpu_aarch64::irq_enter();

    let cpu_id = crate::arch_impl::aarch64::percpu::Aarch64PerCpu::cpu_id() as usize;

    // Re-arm the timer for the next interrupt using the dynamically calculated value
    arm_timer(TICKS_PER_INTERRUPT.load(Ordering::Relaxed));

    // CPU 0 only: update global wall clock time (single atomic operation)
    if cpu_id == 0 {
        crate::time::timer_interrupt();
    }

    // Trace timer tick (lock-free counter + optional event recording)
    trace_timer_tick(crate::time::get_ticks());

    // Increment timer interrupt counter (used for debugging when needed)
    let _count = TIMER_INTERRUPT_COUNT.fetch_add(1, Ordering::Relaxed) + 1;

    // CPU 0 only: poll VirtIO keyboard (single-device, not safe from multiple CPUs)
    if cpu_id == 0 {
        poll_keyboard_to_stdin();
    }

    // Decrement per-CPU quantum and check for reschedule
    let quantum_idx = if cpu_id < crate::arch_impl::aarch64::constants::MAX_CPUS {
        cpu_id
    } else {
        0
    };
    let old_quantum = CURRENT_QUANTUM[quantum_idx].fetch_sub(1, Ordering::Relaxed);
    if old_quantum <= 1 {
        // Quantum expired - request reschedule (all CPUs participate)
        scheduler::set_need_resched();
        CURRENT_QUANTUM[quantum_idx].store(TIME_QUANTUM, Ordering::Relaxed);
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
#[allow(dead_code)] // Debug utility, kept for future use
#[inline(always)]
fn raw_serial_str(s: &[u8]) {
    crate::serial_aarch64::raw_serial_str(s);
}

/// Print a decimal number using raw serial output
/// Used by timer interrupt handler to output [TIMER_COUNT:N] markers
#[allow(dead_code)] // Debug utility, kept for future use
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

/// Poll VirtIO keyboard and push characters to TTY
///
/// This routes keyboard input through the TTY subsystem for:
/// 1. Echo (so you can see what you type)
/// 2. Line discipline processing (backspace, Ctrl-C, etc.)
/// 3. Proper stdin delivery to userspace via TTY read
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
                    // Route through TTY for echo and line discipline processing.
                    // This is the non-blocking version safe for interrupt context.
                    // The TTY will:
                    // 1. Echo the character to the display
                    // 2. Process it through line discipline (handle backspace, Ctrl-C, etc.)
                    // 3. Add it to the TTY input buffer for userspace to read
                    if !crate::tty::push_char_nonblock(c as u8) {
                        // TTY busy - fall back to raw stdin buffer
                        // (no echo, but at least input isn't lost)
                        crate::ipc::stdin::push_byte_from_irq(c as u8);
                    }
                }
            }
        }
    }
}

/// Reset the quantum counter for the current CPU (called when switching threads)
pub fn reset_quantum() {
    #[cfg(feature = "boot_tests")]
    RESET_QUANTUM_CALL_COUNT.fetch_add(1, Ordering::SeqCst);
    let cpu_id = crate::arch_impl::aarch64::percpu::Aarch64PerCpu::cpu_id() as usize;
    let idx = if cpu_id < crate::arch_impl::aarch64::constants::MAX_CPUS {
        cpu_id
    } else {
        0
    };
    CURRENT_QUANTUM[idx].store(TIME_QUANTUM, Ordering::Relaxed);
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

/// Initialize the timer on a secondary CPU.
///
/// Each CPU has its own virtual timer (PPI 27 is per-CPU). The distributor
/// does not need re-configuration for PPIs. We just arm the timer and enable
/// the interrupt in this CPU's GIC interface.
pub fn init_secondary() {
    // Arm the timer with the same interval as CPU 0
    let ticks = TICKS_PER_INTERRUPT.load(Ordering::Relaxed);
    arm_timer(ticks);

    // Enable the virtual timer PPI in the GIC for this CPU.
    // PPIs are per-CPU, but ISENABLER0 (IRQs 0-31) is banked per-CPU,
    // so writing to it from this CPU enables it for this CPU.
    use crate::arch_impl::traits::InterruptController;
    crate::arch_impl::aarch64::gic::Gicv2::enable_irq(TIMER_IRQ as u8);
}

/// Check if the timer is initialized
pub fn is_initialized() -> bool {
    TIMER_INITIALIZED.load(Ordering::Acquire)
}

/// Get the current CPU's quantum value (for debugging)
#[allow(dead_code)]
pub fn current_quantum() -> u32 {
    let cpu_id = crate::arch_impl::aarch64::percpu::Aarch64PerCpu::cpu_id() as usize;
    let idx = if cpu_id < crate::arch_impl::aarch64::constants::MAX_CPUS {
        cpu_id
    } else {
        0
    };
    CURRENT_QUANTUM[idx].load(Ordering::Relaxed)
}

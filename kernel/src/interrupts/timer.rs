//! Timer interrupt handler following OS design best practices
//!
//! This handler ONLY:
//! 1. Updates the timer tick count
//! 2. Decrements current thread's time quantum
//! 3. Sets need_resched flag if quantum expired
//! 4. Sends EOI
//!
//! All context switching happens on the interrupt return path.

use crate::task::scheduler;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Time quantum in timer ticks (100ms per tick, 1000ms quantum = 10 ticks)
const TIME_QUANTUM: u32 = 10;

/// Current thread's remaining time quantum
static mut CURRENT_QUANTUM: u32 = TIME_QUANTUM;

/// Nested interrupt detection: set to true when inside timer handler
static IN_TIMER_HANDLER: AtomicBool = AtomicBool::new(false);

/// Count of nested timer interrupts detected
static NESTED_INTERRUPT_COUNT: AtomicU64 = AtomicU64::new(0);

/// Maximum handler duration observed (in TSC cycles)
static MAX_HANDLER_DURATION: AtomicU64 = AtomicU64::new(0);

/// Timer interrupt handler - absolutely minimal work
///
/// @param from_userspace: 1 if interrupted userspace, 0 if interrupted kernel
#[no_mangle]
pub extern "C" fn timer_interrupt_handler(from_userspace: u8) {
    // NESTED INTERRUPT DETECTION: Check if we're already in the handler
    if IN_TIMER_HANDLER.swap(true, Ordering::Acquire) {
        // We're already in the timer handler - nested interrupt!
        NESTED_INTERRUPT_COUNT.fetch_add(1, Ordering::Relaxed);
        // Send EOI immediately and return to prevent corrupting state
        send_timer_eoi();
        return;
    }

    // Record start time using TSC for duration measurement
    let start_tsc = crate::time::tsc::read_tsc();

    // Enter hardware IRQ context (increments HARDIRQ count)
    crate::per_cpu::irq_enter();

    // Track timer interrupts without logging
    static TIMER_COUNT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
    let _count = TIMER_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);

    // Suppress unused variable warning
    let _ = from_userspace;

    // Core time bookkeeping
    crate::time::timer_interrupt();

    // Decrement current thread's quantum
    unsafe {
        // Use raw pointer to avoid creating references to mutable static (Rust 2024 compatibility)
        let quantum_ptr = core::ptr::addr_of_mut!(CURRENT_QUANTUM);

        if *quantum_ptr > 0 {
            *quantum_ptr -= 1;
        }

        // Only reschedule when quantum expires - NOT every tick
        // The previous "|| has_user_threads" caused rescheduling on EVERY timer tick,
        // which combined with logging overhead meant userspace never got to execute.
        // Idle thread awakening should be handled separately (e.g., when new threads are enqueued).
        let should_set_need_resched = *quantum_ptr == 0;

        if should_set_need_resched {
            scheduler::set_need_resched();
            *quantum_ptr = TIME_QUANTUM; // Reset for next thread
        }
    }

    // CRITICAL FIX: EOI moved to assembly code just before IRETQ
    // Previously, EOI was sent here early, which allowed the PIC to queue
    // another timer interrupt during context switch processing. When IRETQ
    // re-enabled interrupts (IF=1), the pending interrupt fired immediately,
    // preventing any userspace instruction from executing.
    //
    // EOI is now sent by send_timer_eoi() called from timer_entry.asm
    // just before IRETQ, after all processing is complete.

    // Exit hardware IRQ context (decrements HARDIRQ count and may schedule)
    crate::per_cpu::irq_exit();

    // Record handler duration before clearing the in-handler flag
    let end_tsc = crate::time::tsc::read_tsc();
    let duration = end_tsc.saturating_sub(start_tsc);
    MAX_HANDLER_DURATION.fetch_max(duration, Ordering::Relaxed);

    // Clear the in-handler flag to allow next timer interrupt
    IN_TIMER_HANDLER.store(false, Ordering::Release);

    // Periodic diagnostic output every 1000 interrupts (approximately every 1 second at 1000 Hz)
    static DIAG_COUNTER: AtomicU64 = AtomicU64::new(0);
    let diag_count = DIAG_COUNTER.fetch_add(1, Ordering::Relaxed);

    // Print diagnostics every 1000 interrupts (every ~1 second)
    if diag_count > 0 && diag_count % 1000 == 0 {
        let nested = NESTED_INTERRUPT_COUNT.load(Ordering::Relaxed);
        let max_dur = MAX_HANDLER_DURATION.load(Ordering::Relaxed);
        let enters = crate::per_cpu::get_irq_enter_count();
        let exits = crate::per_cpu::get_irq_exit_count();
        let imbalance = crate::per_cpu::get_max_preempt_imbalance();

        crate::serial_println!(
            "DIAG[{}]: nested={}, max_dur={}, enters={}, exits={}, imbalance={}",
            diag_count, nested, max_dur, enters, exits, imbalance
        );
    }
}

/// Reset the quantum counter (called when switching threads)
pub fn reset_quantum() {
    unsafe {
        // Use raw pointer to avoid creating reference to mutable static (Rust 2024 compatibility)
        let quantum_ptr = core::ptr::addr_of_mut!(CURRENT_QUANTUM);
        *quantum_ptr = TIME_QUANTUM;
    }
}

/// Get the number of nested timer interrupts detected
///
/// A nested interrupt occurs when a timer interrupt fires while we're already
/// inside the timer handler. This indicates that the handler is taking longer
/// than the timer period (1ms at 1000 Hz), which can lead to stack overflow
/// and system instability.
#[allow(dead_code)]
pub fn get_nested_interrupt_count() -> u64 {
    NESTED_INTERRUPT_COUNT.load(Ordering::Relaxed)
}

/// Get the maximum timer handler duration observed (in TSC cycles)
///
/// Returns the longest duration the timer handler has taken, measured in CPU cycles.
/// To convert to time, divide by TSC frequency (available from crate::time::tsc::frequency_hz()).
#[allow(dead_code)]
pub fn get_max_handler_duration() -> u64 {
    MAX_HANDLER_DURATION.load(Ordering::Relaxed)
}

/// Get the maximum timer handler duration in microseconds
///
/// Returns None if TSC hasn't been calibrated yet.
#[allow(dead_code)]
pub fn get_max_handler_duration_us() -> Option<u64> {
    let freq = crate::time::tsc::frequency_hz();
    if freq == 0 {
        return None;
    }

    let cycles = MAX_HANDLER_DURATION.load(Ordering::Relaxed);
    // Convert cycles to microseconds: (cycles * 1_000_000) / frequency
    // Use u128 to avoid overflow
    let us = ((cycles as u128) * 1_000_000) / (freq as u128);
    Some(us as u64)
}

/// Log full interrupt frame details when timer fires from userspace
/// Called from assembly with pointer to interrupt frame
/// LOGGING REMOVED - was causing deadlock with serial lock
#[no_mangle]
pub extern "C" fn log_timer_frame_from_userspace(_frame_ptr: *const u64) {
    // No-op: All logging removed to prevent serial lock deadlock
}

/// Log the iretq frame right before returning
/// LOGGING REMOVED - was causing deadlock with serial lock
#[no_mangle]
pub extern "C" fn log_iretq_frame(_frame_ptr: *const u64) {
    // No-op: All logging removed to prevent serial lock deadlock
}

/// Log that we're about to return to userspace from timer interrupt
/// LOGGING REMOVED - was causing deadlock with serial lock
#[no_mangle]
pub extern "C" fn log_timer_return_to_userspace() {
    // No-op: All logging removed to prevent serial lock deadlock
}

/// Log CR3 switch for debugging
/// LOGGING REMOVED - was causing deadlock with serial lock
#[no_mangle]
pub extern "C" fn log_cr3_switch(_new_cr3: u64) {
    // No-op: All logging removed to prevent serial lock deadlock
}

/// Dump IRET frame to serial for debugging
/// LOGGING REMOVED - was causing deadlock with serial lock
#[no_mangle]
pub extern "C" fn dump_iret_frame_to_serial(_frame_ptr: *const u64) {
    // No-op: All logging removed to prevent serial lock deadlock
}

/// Log CR3 value at IRET time
/// LOGGING REMOVED - was causing deadlock with serial lock
#[no_mangle]
pub extern "C" fn log_cr3_at_iret(_cr3: u64) {
    // No-op: All logging removed to prevent serial lock deadlock
}

/// Log GDTR (base and limit) at IRET time
/// LOGGING REMOVED - was causing deadlock with serial lock
#[no_mangle]
pub extern "C" fn log_gdtr_at_iret(_gdtr_ptr: *const u8) {
    // No-op: All logging removed to prevent serial lock deadlock
}

/// Timer interrupt handler for assembly entry point (legacy, unused)
#[no_mangle]
pub extern "C" fn timer_interrupt_handler_asm() {
    // This wrapper is no longer used since the assembly calls timer_interrupt_handler directly
    // Kept for backward compatibility but should be removed
    timer_interrupt_handler(0);
}

/// Send End-Of-Interrupt for the timer
/// CRITICAL: This MUST be called just before IRETQ, after all timer interrupt
/// processing is complete. Calling EOI earlier allows the PIC to queue another
/// timer interrupt during processing, which fires immediately when IRETQ
/// re-enables interrupts.
///
/// Uses try_lock() to avoid deadlock if a nested timer interrupt fires while
/// the lock is held. If try_lock fails, sends EOI directly to the PIC hardware.
#[no_mangle]
pub extern "C" fn send_timer_eoi() {
    // Track how many times try_lock fails (for diagnostics)
    static EOI_LOCK_FAILURES: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

    unsafe {
        // Try to acquire the PICS lock without blocking
        if let Some(mut pics) = super::PICS.try_lock() {
            // Lock acquired successfully, send EOI through the PICS abstraction
            pics.notify_end_of_interrupt(super::InterruptIndex::Timer.as_u8());
        } else {
            // Lock contention detected - use direct hardware access
            EOI_LOCK_FAILURES.fetch_add(1, core::sync::atomic::Ordering::Relaxed);

            // Replicate ChainedPics::notify_end_of_interrupt logic
            use x86_64::instructions::port::Port;

            let interrupt_id = super::InterruptIndex::Timer.as_u8();

            // Timer (IRQ0) is interrupt 32, which is on PIC1 (master)
            // PIC1 handles 32-39, PIC2 handles 40-47
            const PIC_1_OFFSET: u8 = 32;
            const PIC_2_OFFSET: u8 = 40;

            if interrupt_id >= PIC_1_OFFSET && interrupt_id < PIC_2_OFFSET + 8 {
                // If on PIC2 (slave), send EOI to slave first
                if interrupt_id >= PIC_2_OFFSET {
                    let mut pic2_cmd: Port<u8> = Port::new(0xA0);
                    pic2_cmd.write(0x20);
                }
                // Always send EOI to PIC1 (master) for all PIC interrupts
                let mut pic1_cmd: Port<u8> = Port::new(0x20);
                pic1_cmd.write(0x20);
            }
        }
    }
}

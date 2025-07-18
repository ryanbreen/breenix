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

/// Time quantum in timer ticks (10ms per tick, 100ms quantum = 10 ticks)
const TIME_QUANTUM: u32 = 10;

/// Current thread's remaining time quantum
static mut CURRENT_QUANTUM: u32 = TIME_QUANTUM;

/// Timer interrupt handler - absolutely minimal work
#[no_mangle]
pub extern "C" fn timer_interrupt_handler() {
    // Log the first few timer interrupts for debugging
    static TIMER_COUNT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
    let _count = TIMER_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    // TEMPORARILY DISABLE ALL TIMER INTERRUPT LOGGING TO DEBUG DEADLOCK
    // if count < 5 {
    //     log::debug!("Timer interrupt #{}", count);
    //     log::debug!("Timer interrupt #{} - starting handler", count);
    // }
    
    // Update global timer tick count
    crate::time::increment_ticks();
    
    // Decrement current thread's quantum
    unsafe {
        if CURRENT_QUANTUM > 0 {
            CURRENT_QUANTUM -= 1;
        }
        
        // Check if there are user threads ready to run
        let has_user_threads = scheduler::with_scheduler(|s| s.has_userspace_threads()).unwrap_or(false);
        
        // If quantum expired OR there are user threads ready (for idle thread), set need_resched flag
        if CURRENT_QUANTUM == 0 || has_user_threads {
            scheduler::set_need_resched();
            CURRENT_QUANTUM = TIME_QUANTUM; // Reset for next thread
        }
    }
    
    // Send End Of Interrupt
    // TEMPORARILY DISABLE LOGGING
    // if count < 5 {
    //     log::debug!("Timer interrupt #{} - sending EOI", count);
    // }
    unsafe {
        super::PICS.lock()
            .notify_end_of_interrupt(super::InterruptIndex::Timer.as_u8());
    }
    // if count < 5 {
    //     log::debug!("Timer interrupt #{} - EOI sent", count);
    // }
    
    // if count < 5 {
    //     log::debug!("Timer interrupt #{} complete", count);
    // }
}

/// Reset the quantum counter (called when switching threads)
pub fn reset_quantum() {
    unsafe {
        CURRENT_QUANTUM = TIME_QUANTUM;
    }
}

/// Timer interrupt handler for assembly entry point
#[no_mangle]
pub extern "C" fn timer_interrupt_handler_asm() {
    timer_interrupt_handler();
}
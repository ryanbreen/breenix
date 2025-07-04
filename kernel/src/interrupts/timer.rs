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
    // Update global timer tick count
    crate::time::timer_interrupt();
    
    // Decrement current thread's quantum
    unsafe {
        if CURRENT_QUANTUM > 0 {
            CURRENT_QUANTUM -= 1;
        }
        
        // If quantum expired, set need_resched flag
        if CURRENT_QUANTUM == 0 {
            scheduler::set_need_resched();
            CURRENT_QUANTUM = TIME_QUANTUM; // Reset for next thread
        }
    }
    
    // Send End Of Interrupt
    unsafe {
        super::PICS.lock()
            .notify_end_of_interrupt(super::InterruptIndex::Timer.as_u8());
    }
}

/// Reset the quantum counter (called when switching threads)
pub fn reset_quantum() {
    unsafe {
        CURRENT_QUANTUM = TIME_QUANTUM;
    }
}
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
    let count = TIMER_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    
    // Update global timer tick count
    crate::time::increment_ticks();
    
    // Get current thread ID
    if let Some(current_tid) = scheduler::current_thread_id() {
        // Update thread's first_run flag and ticks_run
        let (first_run, _ticks_run) = scheduler::with_scheduler_mut(|s| {
            if let Some(thread) = s.get_thread_mut(current_tid) {
                let was_first_run = thread.first_run;
                thread.ticks_run += 1;
                
                // Skip debug logging for first_run behavior
                
                // If this is the first tick for a user thread, mark it as having run
                if !was_first_run && thread.privilege == crate::task::thread::ThreadPrivilege::User {
                    thread.first_run = true;
                    log::info!("Timer: thread {} completed first run, enabling preemption", current_tid);
                }
                
                (was_first_run, thread.ticks_run)
            } else {
                (true, 0) // Default to allowing preemption if thread not found
            }
        }).unwrap_or((true, 0));
        
        // Only proceed with quantum decrement and preemption if first_run is true
        if first_run {
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
        } else {
            // Thread hasn't had its first run yet, skip preemption
            // Thread hasn't had its first run yet, skip preemption
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

/// Timer interrupt handler for assembly entry point
#[no_mangle]
pub extern "C" fn timer_interrupt_handler_asm() {
    timer_interrupt_handler();
}
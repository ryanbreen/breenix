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
    
    // Check if we're coming from userspace (for Ring 3 verification)
    // Note: We need to check the saved CS on the stack, not the current CS
    // The interrupt frame is at RSP+24 (after error code push and saved registers)
    static mut TIMER_FROM_USERSPACE_LOGGED: bool = false;
    unsafe {
        // Get the saved CS from the interrupt frame
        // The frame is pushed by the CPU: SS, RSP, RFLAGS, CS, RIP
        // We need to look at the saved CS value
        let saved_cs_ptr: *const u64;
        core::arch::asm!("lea {}, [rsp + 0x88]", out(reg) saved_cs_ptr); // Offset to saved CS
        let saved_cs = *(saved_cs_ptr as *const u16);
        
        // If saved CS indicates we interrupted userspace and haven't logged it yet
        if !TIMER_FROM_USERSPACE_LOGGED && (saved_cs & 3) == 3 {
            TIMER_FROM_USERSPACE_LOGGED = true;
            log::info!("✓ Timer interrupt from USERSPACE detected!");
            log::info!("  Timer tick #{}, saved CS={:#x} (Ring 3)", count, saved_cs);
            log::info!("  This confirms async preemption from CPL=3 works");
        }
    }
    
    // ENABLE FIRST FEW TIMER INTERRUPT LOGS FOR CI DEBUGGING
    if count < 5 {
        log::info!("Timer interrupt #{}", count);
        log::info!("Timer interrupt #{} - starting handler", count);
    }

    // Core time bookkeeping
    crate::time::timer_interrupt();
    // Decrement current thread's quantum
    unsafe {
        if CURRENT_QUANTUM > 0 {
            CURRENT_QUANTUM -= 1;
        }

        // Check if there are user threads ready to run
        let has_user_threads =
            scheduler::with_scheduler(|s| s.has_userspace_threads()).unwrap_or(false);

        // If quantum expired OR there are user threads ready (for idle thread), set need_resched flag
        if CURRENT_QUANTUM == 0 || has_user_threads {
            // ENABLE LOGGING FOR CI DEBUGGING
            if count < 5 {
                log::info!("Timer quantum expired or user threads ready, setting need_resched");
                log::info!("About to call scheduler::set_need_resched()");
            }
            scheduler::set_need_resched();
            if count < 5 {
                log::info!("scheduler::set_need_resched() completed");
            }
            CURRENT_QUANTUM = TIME_QUANTUM; // Reset for next thread
        }
    }

    // Send End Of Interrupt
    // TEMPORARILY DISABLE LOGGING
    // if count < 5 {
    //     log::debug!("Timer interrupt #{} - sending EOI", count);
    // }
    unsafe {
        super::PICS
            .lock()
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

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
/// 
/// @param from_userspace: 1 if interrupted userspace, 0 if interrupted kernel
#[no_mangle]
pub extern "C" fn timer_interrupt_handler(from_userspace: u8) {
    // Log the first few timer interrupts for debugging
    static TIMER_COUNT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
    let count = TIMER_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    
    // Check if we're coming from userspace (for Ring 3 verification)
    // The assembly entry point now passes this as a parameter
    use core::sync::atomic::{AtomicU32, Ordering};
    static TIMER_FROM_USERSPACE_COUNT: AtomicU32 = AtomicU32::new(0);
    
    if from_userspace != 0 {
        let userspace_count = TIMER_FROM_USERSPACE_COUNT.fetch_add(1, Ordering::Relaxed);
        if userspace_count < 5 {  // Log first 5 occurrences for verification
            log::info!("✓ Timer interrupt #{} from USERSPACE detected!", userspace_count + 1);
            log::info!("  Timer tick #{}, interrupted Ring 3 code", count);
            log::info!("  This confirms async preemption from CPL=3 works");
            // Note: Full frame details will be logged from assembly
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

/// Log full interrupt frame details when timer fires from userspace
/// Called from assembly with pointer to interrupt frame
#[no_mangle]
pub extern "C" fn log_timer_frame_from_userspace(frame_ptr: *const u64) {
    use core::sync::atomic::{AtomicU32, Ordering};
    static LOG_COUNT: AtomicU32 = AtomicU32::new(0);
    
    let count = LOG_COUNT.fetch_add(1, Ordering::Relaxed);
    if count >= 5 {
        return; // Only log first 5 for analysis
    }
    
    unsafe {
        // Frame layout: [RIP][CS][RFLAGS][RSP][SS]
        let rip = *frame_ptr;
        let cs = *frame_ptr.offset(1);
        let rflags = *frame_ptr.offset(2);
        let rsp = *frame_ptr.offset(3);
        let ss = *frame_ptr.offset(4);
        
        log::info!("Timer interrupt frame from userspace #{}", count + 1);
        log::info!("  RIP: {:#x}", rip);
        log::info!("  CS: {:#x} (RPL={})", cs, cs & 3);
        log::info!("  RFLAGS: {:#x} (IF={})", rflags, if rflags & 0x200 != 0 { "1" } else { "0" });
        log::info!("  RSP: {:#x} (user stack)", rsp);
        log::info!("  SS: {:#x} (RPL={})", ss, ss & 3);
        
        // Validate invariants
        if (cs & 3) != 3 {
            log::error!("  ERROR: CS RPL is not 3!");
        }
        if (ss & 3) != 3 {
            log::error!("  ERROR: SS RPL is not 3!");
        }
        if rflags & 0x200 == 0 {
            log::error!("  ERROR: IF is not set in RFLAGS!");
        }
        if rsp < 0x10000000 || rsp > 0x20000000 {
            log::warn!("  WARNING: RSP {:#x} may be outside expected user range", rsp);
        }
        
        // Get current CR3
        let cr3: u64;
        core::arch::asm!("mov {}, cr3", out(reg) cr3);
        log::info!("  CR3: {:#x} (current page table)", cr3);
    }
}

/// Timer interrupt handler for assembly entry point (legacy, unused)
#[no_mangle]
pub extern "C" fn timer_interrupt_handler_asm() {
    // This wrapper is no longer used since the assembly calls timer_interrupt_handler directly
    // Kept for backward compatibility but should be removed
    timer_interrupt_handler(0);
}

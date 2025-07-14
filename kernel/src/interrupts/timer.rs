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
use crate::serial_println;

// Fatal build ID to ensure we're running the right kernel
#[no_mangle]
#[used]
static FATAL_BUILD_ID: &[u8] = b"BUILD_ID_2024_01_12_xyz";

/// Time quantum in timer ticks (10ms per tick, 100ms quantum = 10 ticks)
const TIME_QUANTUM: u32 = 10;

/// Current thread's remaining time quantum
static mut CURRENT_QUANTUM: u32 = TIME_QUANTUM;

/// Timer interrupt handler - absolutely minimal work
#[no_mangle]
pub extern "C" fn timer_interrupt_handler() {
    // STEP 1: RSP check breadcrumb at very start of Rust handler
    // Output 'D' to COM1 using inline assembly
    unsafe {
        core::arch::asm!(
            "mov dx, 0x3F8",     // COM1 data port
            "mov al, {marker}",  // Load marker
            "out dx, al",        // Output to COM1
            marker = const b'D' as u8,
            out("dx") _,
            out("al") _,
        );
    }
    
    // Log the first few timer interrupts for debugging
    static TIMER_COUNT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
    let count = TIMER_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    
    // Add unique marker to verify we're running the new kernel
    if count == 0 {
        serial_println!("### NEW TIMER BUILD 2024-01-12-FIXED ###");
        serial_println!("### BUILD_ID: {} ###", core::str::from_utf8(FATAL_BUILD_ID).unwrap());
    }
    
    // Add instrumentation to confirm the fix  
    if let Some(current_tid) = scheduler::current_thread_id() {
        if count < 5 || count % 10 == 0 {  // First 5, then every 10th
            serial_println!("TIMER_ISR_START tid={} count={}", current_tid, count);
        }
    }
    
    // Update global timer tick count
    crate::time::increment_ticks();
    
    // Get current thread ID
    if let Some(current_tid) = scheduler::current_thread_id() {
        // Update thread's first_run flag and ticks_run
        let (first_run, _ticks_run) = scheduler::with_scheduler_mut(|s| {
            if let Some(mut thread) = s.get_thread_mut(current_tid) {
                let was_first_run = thread.first_run;
                thread.ticks_run += 1;
                
                // Skip debug logging for first_run behavior
                
                // If this is the first tick for a user thread, mark it as having run
                if !was_first_run && thread.privilege == crate::task::thread::ThreadPrivilege::User {
                    thread.first_run = true;
                    // DISABLE: log::info!("Timer: thread {} completed first run, enabling preemption", current_tid);
                }
                
                // 3.4 Timer-tick accounting per thread
                #[cfg(feature = "sched_debug")]
                if count < 10 || thread.ticks_run % 50 == 0 {
                    crate::serial_println!("TIMER_TICK: tid={} ticks={} quantum={} privilege={:?}", 
                        current_tid, thread.ticks_run, unsafe { CURRENT_QUANTUM }, thread.privilege);
                }
                
                (was_first_run, thread.ticks_run)
            } else {
                (true, 0) // Default to allowing preemption if thread not found
            }
        }).unwrap_or((true, 0));
        
        // TEMPORARILY DISABLE ALL TIMER LOGGING TO FIX DEADLOCK
        // if count < 3 {
        //     serial_println!("TIMER_DEBUG: current_tid={:?}, first_run={}, ticks_run={}", current_tid, first_run, _ticks_run);
        // }
        
        // Fix scheduler deadlock: Allow scheduling for idle thread (TID 0) or first_run threads
        let should_account = first_run || current_tid == 0;
        
        if should_account {
            // Decrement current thread's quantum
            unsafe {
                if CURRENT_QUANTUM > 0 {
                    CURRENT_QUANTUM -= 1;
                }
                
                // Check if there are user threads ready to run
                let has_user_threads = scheduler::with_scheduler(|s| s.has_userspace_threads()).unwrap_or(false);
                
                // If quantum expired OR there are user threads ready (for idle thread), set need_resched flag
                if CURRENT_QUANTUM == 0 || has_user_threads {
                    // 3.4 Timer-tick accounting - reschedule decision
                    #[cfg(feature = "sched_debug")]
                    crate::serial_println!("TIMER_RESCHED: tid={} quantum_expired={} has_user_threads={}", 
                        current_tid, CURRENT_QUANTUM == 0, has_user_threads);
                    
                    scheduler::set_need_resched();
                    CURRENT_QUANTUM = TIME_QUANTUM; // Reset for next thread
                    
                    // DISABLE ALL LOGGING TO FIX DEADLOCK
                    // Debug: Log when need_resched is set
                    // log::debug!("Timer: need_resched set, cur_tid={}", current_tid);
                    
                    // Step 2: Scheduler invocation tracing  
                    // let current_tid = scheduler::current_thread_id();
                    // let has_userspace = scheduler::with_scheduler(|s| s.has_userspace_threads()).unwrap_or(false);
                    // log::info!("TIMER_ISR: need_resched=true, cur={:?}, has_userspace={}", current_tid, has_userspace);
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
    
    // Debug dump for context switch debugging
    extern "C" {
        static _dbg_cr3: u64;
        static _dbg_rip: u64;
        static _debug_current_rsp: u64;
    }
    unsafe {
        if _dbg_cr3 != 0 || _dbg_rip != 0 {
            crate::serial_println!("DBG: cr3={:#x} rip={:#x}", _dbg_cr3, _dbg_rip);
        }
        if _debug_current_rsp != 0 {
            crate::serial_println!("DBG: kernel_rsp={:#x} (PML4 index={})", 
                _debug_current_rsp, 
                (_debug_current_rsp >> 39) & 0x1ff);
        }
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
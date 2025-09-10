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
    // Enter hardware IRQ context (increments HARDIRQ count)
    crate::per_cpu::irq_enter();
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
            crate::irq_info!("✓ Timer interrupt #{} from USERSPACE detected!", userspace_count + 1);
            crate::irq_info!("  Timer tick #{}, interrupted Ring 3 code", count);
            crate::irq_info!("  This confirms async preemption from CPL=3 works");
            // Note: Full frame details will be logged from assembly
        }
    }
    
    // ENABLE FIRST FEW TIMER INTERRUPT LOGS FOR CI DEBUGGING
    if count < 5 {
        crate::irq_debug!("Timer interrupt #{}", count);
        crate::irq_debug!("Timer interrupt #{} - starting handler", count);
    }

    // Core time bookkeeping
    crate::time::timer_interrupt();
    // Decrement current thread's quantum
    unsafe {
        // CRITICAL DEBUG: Log all quantum state
        let quantum_before = CURRENT_QUANTUM;
        if CURRENT_QUANTUM > 0 {
            CURRENT_QUANTUM -= 1;
        }
        let quantum_after = CURRENT_QUANTUM;

        // Check if there are user threads ready to run
        let has_user_threads =
            scheduler::with_scheduler(|s| s.has_userspace_threads()).unwrap_or(false);

        // CRITICAL DEBUG: Log condition evaluation
        if count < 10 {  // Log first 10 to see pattern
            crate::irq_debug!("TIMER DEBUG #{}: quantum_before={}, quantum_after={}, has_user_threads={}", 
                      count, quantum_before, quantum_after, has_user_threads);
        }

        // If quantum expired OR there are user threads ready (for idle thread), set need_resched flag
        let should_set_need_resched = CURRENT_QUANTUM == 0 || has_user_threads;
        
        if count < 10 {
            crate::irq_debug!("TIMER DEBUG #{}: should_set_need_resched={} (quantum_zero={}, has_user={})", 
                      count, should_set_need_resched, CURRENT_QUANTUM == 0, has_user_threads);
        }
        
        if should_set_need_resched {
            // ENABLE LOGGING FOR CI DEBUGGING
            if count < 10 {
                crate::irq_debug!("TIMER DEBUG #{}: Setting need_resched (quantum={}, has_user={})", 
                          count, CURRENT_QUANTUM, has_user_threads);
                crate::irq_debug!("About to call scheduler::set_need_resched()");
            }
            scheduler::set_need_resched();
            if count < 10 {
                crate::irq_debug!("scheduler::set_need_resched() completed");
            }
            CURRENT_QUANTUM = TIME_QUANTUM; // Reset for next thread
        } else {
            if count < 10 {
                crate::irq_debug!("TIMER DEBUG #{}: NOT setting need_resched (quantum={}, has_user={})",
                          count, CURRENT_QUANTUM, has_user_threads);
            }
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

    // Exit hardware IRQ context (decrements HARDIRQ count and may schedule)
    crate::per_cpu::irq_exit();
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
        let saved_rip = *frame_ptr;
        let saved_cs = *frame_ptr.offset(1);
        let rflags = *frame_ptr.offset(2);
        let saved_rsp = *frame_ptr.offset(3);
        let saved_ss = *frame_ptr.offset(4);
        let cpl = saved_cs & 3;
        
        // Get current CR3
        let cr3: u64;
        core::arch::asm!("mov {}, cr3", out(reg) cr3);
        
        // Enhanced logging per Cursor requirements
        crate::irq_info!("R3-TIMER #{}: saved_cs={:#x}, cpl={}, saved_rip={:#x}, saved_rsp={:#x}, saved_ss={:#x}, cr3={:#x}",
            count + 1, saved_cs, cpl, saved_rip, saved_rsp, saved_ss, cr3);
        
        // Verify we interrupted Ring 3
        if cpl == 3 {
            crate::irq_info!("  ✓ Timer interrupted Ring 3 (CPL=3)");
            
            // Verify RIP is in user VA range (typically below 0x7fff_ffff_ffff)
            if saved_rip < 0x0000_8000_0000_0000 {
                crate::irq_info!("  ✓ Saved RIP {:#x} is in user VA range", saved_rip);
            } else {
                crate::irq_info!("  ⚠ Saved RIP {:#x} seems to be in kernel range?", saved_rip);
            }
            
            // Verify SS is also Ring 3
            if (saved_ss & 3) == 3 {
                crate::irq_info!("  ✓ Saved SS {:#x} is Ring 3", saved_ss);
            } else {
                crate::irq_error!("  ⚠ ERROR: Saved SS {:#x} is not Ring 3!", saved_ss);
            }
        } else {
                crate::irq_error!("  ⚠ Timer interrupted Ring {} (not Ring 3!)", cpl);
        }
        
        // Additional validation
        if rflags & 0x200 == 0 {
            crate::irq_error!("  ⚠ ERROR: IF is not set in RFLAGS!");
        }
    }
}

/// Log the iretq frame right before returning
#[no_mangle]
pub extern "C" fn log_iretq_frame(frame_ptr: *const u64) {
    use core::sync::atomic::{AtomicU64, Ordering};
    static LOG_COUNT: AtomicU64 = AtomicU64::new(0);
    let count = LOG_COUNT.fetch_add(1, Ordering::Relaxed);
    
    if count < 5 {
        unsafe {
            let rip = *frame_ptr;
            let cs = *frame_ptr.offset(1);
            let rflags = *frame_ptr.offset(2);
            let rsp = *frame_ptr.offset(3);
            let ss = *frame_ptr.offset(4);
            
            crate::serial_println!("IRETQ FRAME #{}: RIP={:#x}, CS={:#x}, RFLAGS={:#x}, RSP={:#x}, SS={:#x}",
                count, rip, cs, rflags, rsp, ss);
            
            // Check if CS is correct for Ring 3
            if (cs & 3) == 3 {
                crate::serial_println!("  ✓ CS is Ring 3");
            } else {
                crate::serial_println!("  ✗ ERROR: CS is NOT Ring 3! CS={:#x}", cs);
            }
        }
    }
}

/// Log that we're about to return to userspace from timer interrupt
#[no_mangle]
pub extern "C" fn log_timer_return_to_userspace() {
    use core::sync::atomic::{AtomicU64, Ordering};
    // Simple log to track if we reach this point
    static RETURN_COUNT: AtomicU64 = AtomicU64::new(0);
    let count = RETURN_COUNT.fetch_add(1, Ordering::Relaxed);
    if count < 10 {
        crate::serial_println!("TIMER: About to iretq to userspace (count: {})", count);
    }
}

/// Log CR3 switch for debugging
#[no_mangle]
pub extern "C" fn log_cr3_switch(new_cr3: u64) {
    use core::sync::atomic::{AtomicU64, Ordering};
    static LOG_COUNT: AtomicU64 = AtomicU64::new(0);
    let count = LOG_COUNT.fetch_add(1, Ordering::Relaxed);
    
    if count < 10 {
        // Get current CR3 for comparison
        let current_cr3: u64;
        unsafe {
            core::arch::asm!("mov {}, cr3", out(reg) current_cr3);
        }
        
        crate::serial_println!("CR3 SWITCH #{}: current={:#x} -> new={:#x}",
            count, current_cr3, new_cr3);
        
        if new_cr3 != current_cr3 {
            crate::serial_println!("  ✓ Switching from kernel to process page table");
            
            // Log critical addresses (use a known address for now)
            // We know the timer handler is in the same code segment as this function
            let timer_handler_addr = log_cr3_switch as usize as u64;
            crate::serial_println!("  Timer-related function at: {:#x}", timer_handler_addr);
            
            // Check PML4 index for the timer handler
            let pml4_index = (timer_handler_addr >> 39) & 0x1FF;
            crate::serial_println!("  Timer handler is in PML4 entry: {}", pml4_index);
            
            // Get current RIP to see where we're executing from
            let current_rip: u64;
            unsafe {
                core::arch::asm!(
                    "lea {}, [rip]",
                    out(reg) current_rip
                );
            }
            crate::serial_println!("  Current execution at: {:#x} (PML4 entry {})",
                current_rip, (current_rip >> 39) & 0x1FF);
            
            // Get current stack pointer
            let current_rsp: u64;
            unsafe {
                core::arch::asm!(
                    "mov {}, rsp",
                    out(reg) current_rsp
                );
            }
            let rsp_pml4_index = (current_rsp >> 39) & 0x1FF;
            crate::serial_println!("  Current stack at: {:#x} (PML4 entry {})", 
                current_rsp, rsp_pml4_index);
            
            // Check if the IDT is in a mapped PML4 entry
            let idt_addr = 0x100000eea20u64; // From the kernel logs
            let idt_pml4_index = (idt_addr >> 39) & 0x1FF;
            crate::serial_println!("  IDT at: {:#x} (PML4 entry {})", idt_addr, idt_pml4_index);
        }
    }
}

/// Dump IRET frame to serial for debugging
#[no_mangle]
pub extern "C" fn dump_iret_frame_to_serial(frame_ptr: *const u64) {
    use core::sync::atomic::{AtomicU64, Ordering};
    use x86_64::{VirtAddr, structures::paging::{PageTable, PageTableFlags}};
    
    static DUMP_COUNT: AtomicU64 = AtomicU64::new(0);
    let count = DUMP_COUNT.fetch_add(1, Ordering::Relaxed);
    
    // Only dump first few to avoid spam
    if count < 5 {
        unsafe {
            // First, dump raw hex values to see exactly what's in memory
            crate::serial_println!("RAW IRET FRAME #{} at {:#x}:", count, frame_ptr as u64);
            for i in 0..5 {
                let val = *frame_ptr.offset(i);
                crate::serial_println!("  [{}] = {:#018x}", i, val);
            }
            
            let rip = *frame_ptr;
            let cs = *frame_ptr.offset(1);
            let rflags = *frame_ptr.offset(2);
            let rsp = *frame_ptr.offset(3);
            let ss = *frame_ptr.offset(4);
            
            crate::serial_println!("XYZIRET#{}: RIP={:#x} CS={:#x} RFLAGS={:#x} RSP={:#x} SS={:#x}",
                count, rip, cs, rflags, rsp, ss);
            
            // Validate the frame
            if (cs & 3) == 3 {
                crate::serial_println!("  ✓ CS is Ring 3 (user)");
            } else {
                crate::serial_println!("  ⚠ CS is Ring {} (NOT user!)", cs & 3);
            }
            
            if (ss & 3) == 3 {
                crate::serial_println!("  ✓ SS is Ring 3 (user)");
            } else {
                crate::serial_println!("  ⚠ SS is Ring {} (NOT user!)", ss & 3);
            }
            
            if rip < 0x8000_0000_0000 {
                crate::serial_println!("  ✓ RIP in user range");
                
                // CRITICAL: Walk the page table for userspace RIP
                let rip_vaddr = VirtAddr::new(rip);
                let p4_index = (rip >> 39) & 0x1FF;
                let p3_index = (rip >> 30) & 0x1FF;
                let p2_index = (rip >> 21) & 0x1FF;
                let p1_index = (rip >> 12) & 0x1FF;
                
                crate::serial_println!("  Page walk for RIP {:#x}:", rip);
                crate::serial_println!("    P4[{}] P3[{}] P2[{}] P1[{}]", p4_index, p3_index, p2_index, p1_index);
                
                // Get current CR3 to check page table
                let cr3: u64;
                core::arch::asm!("mov {}, cr3", out(reg) cr3);
                
                // Check if user code page is mapped
                // NOTE: This is simplified - in reality we'd need to walk the full hierarchy
                crate::serial_println!("    Current CR3: {:#x}", cr3);
                
                // Check TSS.RSP0 is mapped
                let tss_rsp0 = crate::gdt::get_tss_rsp0();
                crate::serial_println!("  TSS.RSP0: {:#x}", tss_rsp0);
                
            } else {
                crate::serial_println!("  ⚠ RIP looks like kernel address!");
            }
            
            if rflags & 0x200 != 0 {
                crate::serial_println!("  ✓ IF set in RFLAGS");
            } else {
                crate::serial_println!("  ⚠ IF not set in RFLAGS");
            }
        }
    }
}

/// Log CR3 value at IRET time
#[no_mangle]
pub extern "C" fn log_cr3_at_iret(cr3: u64) {
    use core::sync::atomic::{AtomicU64, Ordering};
    static LOG_COUNT: AtomicU64 = AtomicU64::new(0);
    let count = LOG_COUNT.fetch_add(1, Ordering::Relaxed);
    
    if count < 5 {
        crate::serial_println!("CR3 at IRET #{}: {:#x}", count, cr3);
        
        // Check if this is kernel or process page table
        // Kernel typically uses 0x1000000, processes use different values
        if cr3 & 0xFFF == 0 {  // Sanity check - should be page-aligned
            if cr3 == 0x1000000 {
                crate::serial_println!("  ⚠ Still on kernel page table!");
            } else {
                crate::serial_println!("  ✓ On process page table");
            }
        }
    }
}

/// Log GDTR (base and limit) at IRET time
#[no_mangle]
pub extern "C" fn log_gdtr_at_iret(gdtr_ptr: *const u8) {
    use core::sync::atomic::{AtomicU64, Ordering};
    static LOG_COUNT: AtomicU64 = AtomicU64::new(0);
    let count = LOG_COUNT.fetch_add(1, Ordering::Relaxed);
    
    if count < 5 {
        unsafe {
            // GDTR is 10 bytes: 2-byte limit + 8-byte base
            let limit = *(gdtr_ptr as *const u16);
            let base = *(gdtr_ptr.offset(2) as *const u64);
            
            crate::serial_println!("GDTR at IRET #{}: base={:#x}, limit={:#x}", count, base, limit);
            
            // Check if GDT is accessible
            // Try to read the user code selector (index 6)
            if limit >= 55 {  // Need at least 56 bytes for index 6
                crate::serial_println!("  ✓ GDT limit covers user selectors");
                
                // Try to read and dump user segment descriptors
                let gdt_base = base as *const u64;
                
                // Read index 5 (user data, selector 0x2b)
                let user_data_desc = *gdt_base.offset(5);
                crate::serial_println!("  User data (0x2b): {:#018x}", user_data_desc);
                
                // Decode the descriptor
                let present = (user_data_desc >> 47) & 1;
                let dpl = (user_data_desc >> 45) & 3;
                let s_bit = (user_data_desc >> 44) & 1;
                let type_field = (user_data_desc >> 40) & 0xF;
                
                crate::serial_println!("    P={} DPL={} S={} Type={:#x}", present, dpl, s_bit, type_field);
                
                // Read index 6 (user code, selector 0x33)
                let user_code_desc = *gdt_base.offset(6);
                crate::serial_println!("  User code (0x33): {:#018x}", user_code_desc);
                
                // Decode the descriptor
                let present = (user_code_desc >> 47) & 1;
                let dpl = (user_code_desc >> 45) & 3;
                let s_bit = (user_code_desc >> 44) & 1;
                let type_field = (user_code_desc >> 40) & 0xF;
                let l_bit = (user_code_desc >> 53) & 1;
                let d_bit = (user_code_desc >> 54) & 1;
                
                crate::serial_println!("    P={} DPL={} S={} Type={:#x} L={} D={}", 
                    present, dpl, s_bit, type_field, l_bit, d_bit);
            } else {
                crate::serial_println!("  ⚠ GDT limit too small for user selectors!");
            }
        }
    }
}

/// Timer interrupt handler for assembly entry point (legacy, unused)
#[no_mangle]
pub extern "C" fn timer_interrupt_handler_asm() {
    // This wrapper is no longer used since the assembly calls timer_interrupt_handler directly
    // Kept for backward compatibility but should be removed
    timer_interrupt_handler(0);
}

//! Context switching logic for interrupt return path
//!
//! This module handles the actual context switching when returning from
//! interrupts. It's called from assembly code after the interrupt handler
//! has completed its minimal work.

use x86_64::structures::idt::InterruptStackFrame;
// PhysFrame import removed - now using thread's page_table_frame field
use crate::task::process_context::{SavedRegisters, save_userspace_context, restore_userspace_context};
use crate::task::scheduler;
use crate::task::thread::ThreadPrivilege;

// NEXT_PAGE_TABLE removed - now using thread CR3 field directly


/// Check if rescheduling is needed and perform context switch if necessary
/// 
/// This is called from the assembly interrupt return path and is the
/// CORRECT place to handle context switching (not in the interrupt handler).
#[no_mangle]
pub extern "C" fn check_need_resched_and_switch(
    saved_regs: &mut SavedRegisters,
    interrupt_frame: &mut InterruptStackFrame,
) {
    // Check if reschedule is needed
    if !scheduler::check_and_clear_need_resched() {
        // No reschedule needed, just return
        return;
    }
    
    // EXTENDED INTERRUPT MASKING TEST:
    // Disable interrupts for the ENTIRE context switch operation
    // to test if timing is the root cause of heap corruption
    log::debug!("EXTENDED_MASKING: Disabling interrupts for entire context switch");
    x86_64::instructions::interrupts::disable();
    
    // Step 4: Confirm context switch code is reached
    unsafe {
        core::arch::asm!("out 0x80, al"); // QEMU logs this magic I/O
    }
    
    // log::debug!("check_need_resched_and_switch: Need resched is true, proceeding...");
    
    // Rate limit the debug message
    static RESCHED_LOG_COUNTER: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
    let count = RESCHED_LOG_COUNTER.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    if count < 5 || count % 30 == 0 {
        log::debug!("check_need_resched_and_switch: Reschedule needed (count: {})", count);
    }
    
    // Perform scheduling decision
    let schedule_result = scheduler::schedule();
    // Always log the first few results
    if count < 10 || schedule_result.is_some() {
        log::info!("scheduler::schedule() returned: {:?} (count: {})", schedule_result, count);
    } else if count % 30 == 0 {
        log::debug!("scheduler::schedule() returned: {:?} (count: {})", schedule_result, count);
    }
    
    // Always log if we didn't get a schedule result
    if schedule_result.is_none() {
        if count < 20 {
            log::debug!("scheduler::schedule() returned None - no thread switch available (count: {})", count);
        }
        // Early return if no scheduling decision
        return;
    }
    if let Some((old_thread_id, new_thread_id)) = schedule_result {
        if old_thread_id == new_thread_id {
            // Same thread continues running
            return;
        }
        
        log::debug!("Context switch on interrupt return: {} -> {}", old_thread_id, new_thread_id);
        
        // Check if we're coming from userspace
        let from_userspace = (interrupt_frame.code_segment.0 & 3) == 3;
        log::debug!("Context switch: from_userspace={}, CS={:#x}", from_userspace, interrupt_frame.code_segment.0);
        
        // Save current thread's context if coming from userspace
        if from_userspace {
            save_current_thread_context(old_thread_id, saved_regs, interrupt_frame);
        }
        
        // Switch to the new thread
        switch_to_thread(new_thread_id, saved_regs, interrupt_frame);
        
        // Reset the timer quantum for the new thread
        super::timer::reset_quantum();
    }
    
    // NOTE: Interrupts remain DISABLED for extended masking test
    // They will be re-enabled by IRETQ when returning to userspace
    // or by the kernel when appropriate for kernel threads
    log::debug!("Context switch completed with extended interrupt masking");
}

/// Save the current thread's userspace context
fn save_current_thread_context(
    thread_id: u64,
    saved_regs: &mut SavedRegisters,
    interrupt_frame: &mut InterruptStackFrame,
) {
    // CRITICAL: Use try_manager in interrupt context to avoid deadlock
    // Never use with_process_manager() from interrupt handlers!
    if let Some(mut manager_guard) = crate::process::try_manager() {
        if let Some(ref mut manager) = *manager_guard {
            if let Some((pid, process)) = manager.find_process_by_thread_mut(thread_id) {
                if let Some(ref mut thread) = process.main_thread {
                    save_userspace_context(thread, interrupt_frame, saved_regs);
                    log::trace!("Saved context for process {} (thread {})", pid.as_u64(), thread_id);
                }
            }
        }
    } else {
        log::warn!("Could not acquire process manager lock in interrupt context for thread {}", thread_id);
    }
}

/// Switch to a different thread
fn switch_to_thread(
    thread_id: u64,
    saved_regs: &mut SavedRegisters,
    interrupt_frame: &mut InterruptStackFrame,
) {
    // Switch TLS if needed (kernel threads don't have TLS)
    let is_kernel_thread = scheduler::with_thread_mut(thread_id, |thread| {
        thread.privilege == ThreadPrivilege::Kernel
    }).unwrap_or(false);
    
    if !is_kernel_thread {
        if let Err(e) = crate::tls::switch_tls(thread_id) {
            log::error!("Failed to switch TLS for thread {}: {}", thread_id, e);
            return;
        }
    }
    
    // Check if this is the idle thread
    let is_idle = scheduler::with_scheduler(|sched| thread_id == sched.idle_thread()).unwrap_or(false);
    
    if is_idle {
        // Set up to return to idle loop
        setup_idle_return(interrupt_frame);
    } else if is_kernel_thread {
        // Set up to return to kernel thread
        setup_kernel_thread_return(thread_id, saved_regs, interrupt_frame);
    } else {
        // Restore userspace thread context
        restore_userspace_thread_context(thread_id, saved_regs, interrupt_frame);
    }
}

/// Set up interrupt frame to return to idle loop
fn setup_idle_return(interrupt_frame: &mut InterruptStackFrame) {
    unsafe {
        interrupt_frame.as_mut().update(|frame| {
            frame.code_segment = crate::gdt::kernel_code_selector();
            frame.stack_segment = crate::gdt::kernel_data_selector();
            frame.instruction_pointer = x86_64::VirtAddr::new(idle_loop as *const () as u64);
            frame.cpu_flags = x86_64::registers::rflags::RFlags::INTERRUPT_FLAG;
            
            // CRITICAL: Must set kernel stack pointer when returning to idle!
            // The idle thread runs in kernel mode and needs a kernel stack.
            // Get the kernel stack pointer from the current CPU stack
            let current_rsp: u64;
            core::arch::asm!("mov {}, rsp", out(reg) current_rsp);
            // Add some space to account for the interrupt frame
            frame.stack_pointer = x86_64::VirtAddr::new(current_rsp + 256);
        });
        
        // No page table switch needed - staying in kernel mode
    }
    log::trace!("Set up return to idle loop");
}

/// Set up interrupt frame to return to kernel thread
fn setup_kernel_thread_return(
    thread_id: u64,
    saved_regs: &mut SavedRegisters,
    interrupt_frame: &mut InterruptStackFrame,
) {
    // Get thread info
    let thread_info = scheduler::with_thread_mut(thread_id, |thread| {
        (
            thread.name.clone(),
            thread.context.rip,
            thread.context.rsp,
            thread.context.rflags,
            thread.context.rdi,
        )
    });
    
    if let Some((name, rip, rsp, rflags, rdi)) = thread_info {
        unsafe {
            interrupt_frame.as_mut().update(|frame| {
                frame.instruction_pointer = x86_64::VirtAddr::new(rip);
                frame.stack_pointer = x86_64::VirtAddr::new(rsp);
                frame.code_segment = crate::gdt::kernel_code_selector();
                frame.stack_segment = crate::gdt::kernel_data_selector();
                frame.cpu_flags = x86_64::registers::rflags::RFlags::from_bits_truncate(rflags);
            });
            
            // Set up argument in RDI
            saved_regs.rdi = rdi;
            
            // Clear other registers for safety
            saved_regs.rax = 0;
            saved_regs.rbx = 0;
            saved_regs.rcx = 0;
            saved_regs.rdx = 0;
            saved_regs.rsi = 0;
            saved_regs.rbp = 0;
            saved_regs.r8 = 0;
            saved_regs.r9 = 0;
            saved_regs.r10 = 0;
            saved_regs.r11 = 0;
            saved_regs.r12 = 0;
            saved_regs.r13 = 0;
            saved_regs.r14 = 0;
            saved_regs.r15 = 0;
        }
        
        log::trace!("Set up kernel thread {} '{}' to run at {:#x}", thread_id, name, rip);
        
        // No page table switch needed - staying in kernel mode
    }
}

/// Restore userspace thread context
fn restore_userspace_thread_context(
    thread_id: u64,
    saved_regs: &mut SavedRegisters,
    interrupt_frame: &mut InterruptStackFrame,
) {
    log::info!("restore_userspace_thread_context: Restoring thread {}", thread_id);
    
    // CRITICAL: Use try_manager in interrupt context to avoid deadlock
    // Never use with_process_manager() from interrupt handlers!
    if let Some(mut manager_guard) = crate::process::try_manager() {
        log::debug!("Got process manager lock");
        if let Some(ref mut manager) = *manager_guard {
            if let Some((pid, process)) = manager.find_process_by_thread_mut(thread_id) {
                log::debug!("Found process {} for thread {}", pid.as_u64(), thread_id);
                if let Some(ref mut thread) = process.main_thread {
                    log::debug!("Thread privilege: {:?}", thread.privilege);
                    if thread.privilege == ThreadPrivilege::User {
                        restore_userspace_context(thread, interrupt_frame, saved_regs);
                        log::trace!("Restored context for process {} (thread {})", pid.as_u64(), thread_id);
                        
                        // Load page table frame from thread (updated by exec)
                        // The actual switch will happen in assembly code right before iretq
                        if let Some(page_table_frame) = thread.page_table_frame {
                            // Validate with process page table if available
                            if let Some(ref page_table) = process.page_table {
                                // STEP 2: Verify user pages are present & executable
                                use x86_64::VirtAddr;
                                
                                let user_rsp = thread.context.rsp;
                                let user_rip = thread.context.rip;
                                
                                crate::serial_println!("USER-MAP-DEBUG: About to check RSP={:#x}, RIP={:#x}", user_rsp, user_rip);
                                
                                let rsp_ok = page_table.translate_page(VirtAddr::new(user_rsp));
                                let rip_ok = page_table.translate_page(VirtAddr::new(user_rip));
                                
                                crate::serial_println!(
                                    "USER-MAP: RIP={:#x} {:?}, RSP={:#x} {:?}",
                                    user_rip, rip_ok, user_rsp, rsp_ok
                                );
                                
                                // Also check RSP-8 for safety as suggested in checklist
                                let rsp_safe = page_table.translate_page(VirtAddr::new(user_rsp - 8));
                                crate::serial_println!("USER-MAP: RSP-8={:#x} {:?}", user_rsp - 8, rsp_safe);
                                
                                // STEP 4: Check if current kernel stack is mapped in new CR3
                                let current_kernel_rsp: u64;
                                unsafe {
                                    core::arch::asm!("mov {}, rsp", out(reg) current_kernel_rsp);
                                }
                                let kstack_ok = page_table.translate_page(VirtAddr::new(current_kernel_rsp));
                                crate::serial_println!("USER-MAP: KSTACK={:#x} {:?}", current_kernel_rsp, kstack_ok);
                                
                                // STEP 5: Log userspace RIP to see where we're returning
                                crate::serial_println!("USER-RIP: About to return to userspace at RIP={:#x}", user_rip);
                                
                                // Additional debug: Check segment registers
                                let cs: u16;
                                let ss: u16;
                                unsafe {
                                    core::arch::asm!("mov {cs:x}, cs", cs = out(reg) cs);
                                    core::arch::asm!("mov {ss:x}, ss", ss = out(reg) ss);
                                }
                                crate::serial_println!("CURRENT-SEGS: CS={:#x} SS={:#x}", cs, ss);
                            }
                            
                            // Use thread's page table frame for CR3 loading
                            log::info!("Process {} will use page table frame={:#x} (from thread)", 
                                     pid.as_u64(), page_table_frame.start_address().as_u64());
                        } else if let Some(ref page_table) = process.page_table {
                            // Fallback to process page table if thread doesn't have one
                            let page_table_frame = page_table.level_4_frame();
                            log::info!("Process {} will use page table frame={:#x} (from process)", 
                                     pid.as_u64(), page_table_frame.start_address().as_u64());
                        } else {
                            log::warn!("Process {} has no page table!", pid.as_u64());
                        }
                        
                        // Update TSS RSP0 for the new thread's kernel stack
                        // CRITICAL: Use the kernel stack, not the userspace stack!
                        if let Some(kernel_stack_top) = thread.kernel_stack_top {
                            log::info!("Setting kernel stack for thread {} to {:#x}", thread_id, kernel_stack_top.as_u64());
                            crate::gdt::set_kernel_stack(kernel_stack_top);
                        } else {
                            log::error!("Userspace thread {} has no kernel stack!", thread_id);
                        }
                    }
                }
            }
        }
    } else {
        log::warn!("Could not acquire process manager lock in interrupt context for thread {}", thread_id);
    }
}

/// Simple idle loop
fn idle_loop() -> ! {
    loop {
        x86_64::instructions::hlt();
    }
}

/// Set the next page table to switch to on interrupt return
/// This is used by execve() to schedule a page table switch
// set_next_page_table() removed - now using thread CR3 field directly

/// Get the page table frame (CR3) for the current thread
/// This is called from assembly code before returning to userspace
#[no_mangle]
pub extern "C" fn get_thread_cr3() -> u64 {
    // Get the current thread ID
    if let Some(thread_id) = scheduler::current_thread_id() {
        // Get the thread's page table frame
        let cr3 = scheduler::with_scheduler(|scheduler| {
            if let Some(thread) = scheduler.get_thread(thread_id) {
                if let Some(page_table_frame) = thread.page_table_frame {
                    let addr = page_table_frame.start_address().as_u64();
                    
                    // DEBUG: Log current CR3 for comparison
                    let current_cr3 = x86_64::registers::control::Cr3::read().0.start_address().as_u64();
                    crate::serial_println!("THREAD_CR3_SWITCH: Current CR3={:#x}, switching to CR3={:#x}", current_cr3, addr);
                    
                    // Verify the page table frame is valid
                    if addr == 0 || addr >= 0x100000000 {
                        crate::serial_println!("INVALID_CR3: Attempted to switch to invalid CR3={:#x}", addr);
                        return 0;
                    }
                    
                    return addr;
                }
            }
            0
        }).unwrap_or(0);
        
        if cr3 == 0 {
            // Fallback to process page table
            if let Some(mut manager_guard) = crate::process::try_manager() {
                if let Some(ref manager) = *manager_guard {
                    if let Some((_, process)) = manager.find_process_by_thread(thread_id) {
                        if let Some(ref page_table) = process.page_table {
                            let addr = page_table.level_4_frame().start_address().as_u64();
                            crate::serial_println!("FALLBACK_CR3: Using process page table CR3={:#x}", addr);
                            return addr;
                        }
                    }
                }
            }
        }
        
        cr3
    } else {
        0 // No thread, no page table switch
    }
}
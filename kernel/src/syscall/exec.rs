//! POSIX-compliant execve() implementation
//!
//! This module implements exec_replace() which never returns on success,
//! properly replacing the current process image with a new program.

use crate::task::scheduler;
use alloc::string::String;
use x86_64::structures::paging::{PhysFrame, Size4KiB};
use x86_64::PhysAddr;
use crate::memory::process_memory::ProcessPageTable;
use core::sync::atomic::{AtomicBool, Ordering};

/// Flag to indicate that an exec() has occurred and the interrupt frame needs updating
static EXEC_PENDING: AtomicBool = AtomicBool::new(false);

/// Set the exec pending flag
pub fn set_exec_pending() {
    EXEC_PENDING.store(true, Ordering::SeqCst);
}

/// Check and clear the exec pending flag
pub fn check_and_clear_exec_pending() -> bool {
    EXEC_PENDING.swap(false, Ordering::SeqCst)
}

/// Switch to kernel page table for a closure, restore on return.
/// This ensures page table modifications are done safely without page faults.
pub fn with_kernel_page_table<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    use x86_64::registers::control::Cr3;
    
    // 1. Read current CR3
    let (old_frame, old_flags) = Cr3::read();
    
    // 2. If already on kernel CR3 (0x101000), just run f()
    const KERNEL_CR3: u64 = 0x101000;
    let kernel_frame = PhysFrame::<Size4KiB>::containing_address(PhysAddr::new(KERNEL_CR3));
    
    if old_frame == kernel_frame {
        log::debug!("with_kernel_page_table: Already on kernel CR3");
        return f();
    }
    
    // 3. Switch to kernel page table
    log::debug!("with_kernel_page_table: Switching from CR3={:#x} to kernel CR3={:#x}", 
                old_frame.start_address().as_u64(), KERNEL_CR3);
    unsafe { 
        Cr3::write(kernel_frame, old_flags);
    }
    
    // 4. Run critical section
    let ret = f();
    
    // 5. Restore original page table
    log::debug!("with_kernel_page_table: Restoring CR3={:#x}", 
                old_frame.start_address().as_u64());
    unsafe { 
        Cr3::write(old_frame, old_flags);
    }
    
    ret
}

/// Replace the current process with a new program (never returns on success)
/// 
/// This function:
/// 1. Updates the current thread's saved registers for the new program
/// 2. Returns 0 to indicate success (the interrupt return path will use the new context)
/// 
/// # Arguments
/// * `program_name` - Name of program to execute
/// * `elf_bytes` - ELF binary data
/// 
/// # Returns
/// 0 on success (but control never returns to original caller)
/// Negative error code on failure
pub fn exec_replace(program_name: String, elf_bytes: &[u8]) -> isize {
    log::info!("exec_replace: Starting exec of program '{}'", program_name);
    
    // Get current thread and process
    let current_thread_id = scheduler::current_thread_id()
        .expect("exec_replace: No current thread");
    
    // Step 1: Create new page table and load ELF - all within kernel CR3 context
    let (entry_rip, stack_top, new_pt_frame) = with_kernel_page_table(|| {
        // Allocate fresh process page table
        let mut new_pt = match ProcessPageTable::new() {
            Ok(pt) => pt,
            Err(e) => {
                log::error!("exec_replace: Failed to create new page table: {}", e);
                panic!("exec_replace: Failed to create new page table");
            }
        };
        
        // Clear any user mappings that might have been copied from the current page table
        // This prevents conflicts when loading the new program
        new_pt.clear_user_entries();
        
        // Unmap the old program's pages in common userspace ranges
        // This is necessary because PML4 entry 0 contains both kernel and user mappings
        // Typical userspace code location: 0x10000000 - 0x10100000 (1MB range)
        if let Err(e) = new_pt.unmap_user_pages(
            x86_64::VirtAddr::new(0x10000000), 
            x86_64::VirtAddr::new(0x10100000)
        ) {
            log::warn!("Failed to unmap old user code pages: {}", e);
        }
        
        // Also unmap any pages in the BSS/data area (just after code)
        if let Err(e) = new_pt.unmap_user_pages(
            x86_64::VirtAddr::new(0x10001000), 
            x86_64::VirtAddr::new(0x10010000)
        ) {
            log::warn!("Failed to unmap old user data pages: {}", e);
        }
        
        // Load the ELF into the new page table
        let loaded_elf = match crate::elf::load_elf_into_page_table(elf_bytes, &mut new_pt) {
            Ok(elf) => elf,
            Err(e) => {
                log::error!("exec_replace: Failed to load ELF: {}", e);
                panic!("exec_replace: Failed to load ELF");
            }
        };
        
        let entry_rip = loaded_elf.entry_point.as_u64();
        
        // Debug: Log entry point
        crate::serial_println!("EXEC_DEBUG: New ELF entry point: {:#x}", entry_rip);
        
        // Allocate and map user stack
        use crate::memory::stack;
        const USER_STACK_SIZE: usize = 64 * 1024; // 64KB stack
        let user_stack = match stack::allocate_stack(USER_STACK_SIZE) {
            Ok(stack) => stack,
            Err(_) => {
                log::error!("exec_replace: Failed to allocate user stack");
                panic!("exec_replace: Failed to allocate user stack");
            }
        };
        let stack_top = user_stack.top();
        
        // Map stack to process page table
        let stack_bottom = stack_top - USER_STACK_SIZE as u64;
        match crate::memory::process_memory::map_user_stack_to_process(&mut new_pt, stack_bottom, stack_top) {
            Ok(()) => {},
            Err(e) => {
                log::error!("exec_replace: Failed to map user stack: {}", e);
                panic!("exec_replace: Failed to map user stack");
            }
        }
        
        crate::serial_println!("EXEC_DEBUG: Setting thread context RIP to {:#x}", entry_rip);
        log::info!("exec_replace: Successfully loaded ELF, entry_rip={:#x}, stack_top={:#x}", 
                   entry_rip, stack_top.as_u64());
        
        // Get the physical frame before we move the page table
        let pt_frame = new_pt.level_4_frame();
        
        // Store new page table and stack in the process
        let mut manager_guard = crate::process::manager();
        if let Some(ref mut manager) = *manager_guard {
            if let Some((pid, process)) = manager.find_process_by_thread_mut(current_thread_id) {
                log::info!("exec_replace: Replacing page table for process {}", pid.as_u64());
                
                // CRITICAL: Replace page table before scheduling CR3 switch
                // This ensures the scheduler will use the new page table on the next context switch
                process.replace_page_table(new_pt);
                process.stack = Some(alloc::boxed::Box::new(user_stack));
                
                // Verify page table replacement worked
                if let Some(ref new_page_table) = process.page_table {
                    let new_frame = new_page_table.level_4_frame();
                    log::info!("exec_replace: Page table replaced successfully, new frame={:#x}", 
                              new_frame.start_address().as_u64());
                } else {
                    log::error!("exec_replace: Page table replacement failed - no page table found");
                    panic!("exec_replace: Page table replacement failed");
                }
            } else {
                log::error!("exec_replace: Current thread {} not found in process manager", current_thread_id);
                panic!("exec_replace: Current thread not found in process manager");
            }
        } else {
            log::error!("exec_replace: Could not acquire process manager lock");
            panic!("exec_replace: Could not acquire process manager lock");
        }
        
        (entry_rip, x86_64::VirtAddr::new(stack_top.as_u64()), pt_frame)
    });
    
    // Step 2: Overwrite the current thread's saved registers
    // We need to update the current thread's context in the scheduler
    let thread_updated = scheduler::with_scheduler(|scheduler| {
        if let Some(mut thread) = scheduler.get_thread_mut(current_thread_id) {
            log::info!("exec_replace: Updating thread {} context", current_thread_id);
            
            // Update the thread's saved context for IRET
            thread.context.rip = entry_rip;
            thread.context.cs = crate::gdt::user_code_selector().0 as u64;
            thread.context.rflags = 0x202; // IF=1, all other bits clear
            thread.context.rsp = stack_top.as_u64();
            thread.context.ss = crate::gdt::user_data_selector().0 as u64;
            
            // CRITICAL: Store page table frame in thread for direct CR3 loading
            thread.page_table_frame = Some(new_pt_frame);
            
            // Optionally zero GP registers (ABI permits anything)
            thread.context.rax = 0;
            thread.context.rbx = 0;
            thread.context.rcx = 0;
            thread.context.rdx = 0;
            thread.context.rbp = 0;
            thread.context.rsi = 0;
            thread.context.rdi = 0;
            thread.context.r8 = 0;
            thread.context.r9 = 0;
            thread.context.r10 = 0;
            thread.context.r11 = 0;
            thread.context.r12 = 0;
            thread.context.r13 = 0;
            thread.context.r14 = 0;
            thread.context.r15 = 0;
            
            log::info!("exec_replace: Updated thread context - RIP={:#x}, RSP={:#x}, CR3={:#x}", 
                       thread.context.rip, thread.context.rsp, new_pt_frame.start_address().as_u64());
            true
        } else {
            false
        }
    }).unwrap_or(false);
    
    if !thread_updated {
        log::error!("exec_replace: Current thread {} not found", current_thread_id);
        panic!("exec_replace: Current thread not found");
    }
    
    // Step 3: The page table is now stored in the process structure
    // The scheduler will automatically use the new page table on the next context switch
    log::info!("exec_replace: Page table stored in process structure, frame={:#x}", 
               new_pt_frame.start_address().as_u64());
    log::info!("exec_replace: Scheduler will use new page table on next context switch");
    
    // TODO: Also store page table frame in thread context for direct CR3 loading
    
    log::info!("exec_replace: Process image replaced, context will be used on syscall return");
    
    // Step 4: Force the interrupt return path to use the new context
    // We've updated the thread's saved context and scheduled a CR3 switch.
    // When this syscall returns through the interrupt return path, it will:
    // 1. Switch to the new CR3 (via get_thread_cr3)
    // 2. Use the updated RIP/RSP from the thread context
    // 
    // This effectively implements exec() - the old program never resumes because
    // the return address has been replaced with the new program's entry point.
    
    // The exec() syscall "never returns" in the sense that control never returns
    // to the original caller. However, we need to ensure the interrupt frame gets
    // updated with the new context.
    // 
    // Since we might be the only thread, the scheduler won't switch contexts.
    // We need to force the interrupt frame to be updated for exec.
    
    // Mark that an exec happened and we need special handling
    set_exec_pending();
    
    // Debug: Confirm flag is set
    crate::serial_println!("EXEC_DEBUG: exec_replace completed, flag set");
    log::info!("exec_replace: Returning 0 with exec pending flag set");
    0
}


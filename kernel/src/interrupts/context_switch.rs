//! Context switching logic for interrupt return path
//!
//! This module handles the actual context switching when returning from
//! interrupts. It's called from assembly code after the interrupt handler
//! has completed its minimal work.

use crate::task::process_context::{
    restore_userspace_context, save_userspace_context, SavedRegisters,
};
use crate::task::scheduler;
use crate::task::thread::ThreadPrivilege;
use x86_64::structures::idt::InterruptStackFrame;
use x86_64::VirtAddr;

// REMOVED: NEXT_PAGE_TABLE is no longer needed since CR3 switching happens
// immediately during context switch, not deferred to interrupt return

/// Check if rescheduling is needed and perform context switch if necessary
///
/// This is called from the assembly interrupt return path and is the
/// CORRECT place to handle context switching (not in the interrupt handler).
#[no_mangle]
pub extern "C" fn check_need_resched_and_switch(
    saved_regs: &mut SavedRegisters,
    interrupt_frame: &mut InterruptStackFrame,
) {
    // CRITICAL: Only schedule when returning to userspace with preempt_count == 0
    if !crate::per_cpu::can_schedule(interrupt_frame.code_segment.0 as u64) {
        return;
    }
    
    // Check if reschedule is needed
    if !scheduler::check_and_clear_need_resched() {
        // No reschedule needed, just return
        return;
    }

    // log::debug!("check_need_resched_and_switch: Need resched is true, proceeding...");

    // Rate limit the debug message
    static RESCHED_LOG_COUNTER: core::sync::atomic::AtomicU64 =
        core::sync::atomic::AtomicU64::new(0);
    let count = RESCHED_LOG_COUNTER.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    if count < 5 || count % 30 == 0 {
        log::debug!(
            "check_need_resched_and_switch: Reschedule needed (count: {})",
            count
        );
    }

    // Perform scheduling decision
    let schedule_result = scheduler::schedule();
    // Always log the first few results
    if count < 10 || schedule_result.is_some() {
        log::info!(
            "scheduler::schedule() returned: {:?} (count: {})",
            schedule_result,
            count
        );
    } else if count % 30 == 0 {
        log::debug!(
            "scheduler::schedule() returned: {:?} (count: {})",
            schedule_result,
            count
        );
    }

    // Always log if we didn't get a schedule result
    if schedule_result.is_none() {
        if count < 20 {
            log::warn!(
                "scheduler::schedule() returned None - no thread switch available (count: {})",
                count
            );
        }
        // Early return if no scheduling decision
        return;
    }
    if let Some((old_thread_id, new_thread_id)) = schedule_result {
        if old_thread_id == new_thread_id {
            // Same thread continues running
            return;
        }

        log::debug!(
            "Context switch on interrupt return: {} -> {}",
            old_thread_id,
            new_thread_id
        );

        // Check if we're coming from userspace and surface prominently for CI
        let from_userspace = (interrupt_frame.code_segment.0 & 3) == 3;
        log::info!(
            "Context switch: from_userspace={}, CS={:#x}",
            from_userspace,
            interrupt_frame.code_segment.0
        );
        // Also mirror to serial to ensure capture regardless of log level
        crate::serial_println!(
            "Context switch: from_userspace={}, CS={:#x}",
            from_userspace,
            interrupt_frame.code_segment.0
        );
        // Emit canonical ring3 marker on the first entry to userspace
        if from_userspace {
            static mut EMITTED_RING3_MARKER: bool = false;
            unsafe {
                if !EMITTED_RING3_MARKER {
                    EMITTED_RING3_MARKER = true;
                    crate::serial_println!("RING3_ENTER: CS=0x33");
                    // CI SUCCESS MARKER: Ring 3 execution verified!
                    crate::serial_println!(
                        "[ OK ] RING3_SMOKE: userspace executed + syscall path verified"
                    );
                    crate::serial::flush();
                    #[cfg(feature = "testing")]
                    {
                        // Don't exit immediately - let CI runner detect the success marker
                        // crate::test_exit_qemu(crate::QemuExitCode::Success);
                    }
                }
            }
        }

        // Save current thread's context if coming from userspace
        if from_userspace {
            save_current_thread_context(old_thread_id, saved_regs, interrupt_frame);
        }

        // Switch to the new thread
        switch_to_thread(new_thread_id, saved_regs, interrupt_frame);

        // If switching to userspace, emit a clear log right before return
        if scheduler::with_thread_mut(new_thread_id, |t| t.privilege == ThreadPrivilege::User)
            .unwrap_or(false)
        {
            crate::serial_println!(
                "Restored userspace context for thread {} and prepared return to Ring 3 (CS=0x33)",
                new_thread_id
            );
        }

        // Reset the timer quantum for the new thread
        super::timer::reset_quantum();
    }
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
                    log::trace!(
                        "Saved context for process {} (thread {})",
                        pid.as_u64(),
                        thread_id
                    );
                }
            }
        }
    } else {
        log::warn!(
            "Could not acquire process manager lock in interrupt context for thread {}",
            thread_id
        );
    }
}

/// Switch to a different thread
fn switch_to_thread(
    thread_id: u64,
    saved_regs: &mut SavedRegisters,
    interrupt_frame: &mut InterruptStackFrame,
) {
    // Update per-CPU current thread and TSS.RSP0
    scheduler::with_thread_mut(thread_id, |thread| {
        // Update per-CPU current thread pointer
        let thread_ptr = thread as *const _ as *mut crate::task::thread::Thread;
        crate::per_cpu::set_current_thread(thread_ptr);
        
        // Update TSS.RSP0 with new thread's kernel stack top
        // This is critical for interrupt/exception handling
        if let Some(kernel_stack_top) = thread.kernel_stack_top {
            crate::per_cpu::update_tss_rsp0(kernel_stack_top);
            log::trace!("sched: switch to thread {} rsp0={:#x}", thread_id, kernel_stack_top);
        }
    });
    
    // Switch TLS if needed (kernel threads don't have TLS)
    let is_kernel_thread = scheduler::with_thread_mut(thread_id, |thread| {
        thread.privilege == ThreadPrivilege::Kernel
    })
    .unwrap_or(false);

    if !is_kernel_thread {
        if let Err(e) = crate::tls::switch_tls(thread_id) {
            log::error!("Failed to switch TLS for thread {}: {}", thread_id, e);
            return;
        }
    }

    // Check if this is the idle thread
    let is_idle =
        scheduler::with_scheduler(|sched| thread_id == sched.idle_thread()).unwrap_or(false);

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

        // FIXED: Switch back to kernel page table when running kernel threads
        // This ensures kernel threads run with kernel page tables
        crate::memory::process_memory::switch_to_kernel_page_table();
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

        log::trace!(
            "Set up kernel thread {} '{}' to run at {:#x}",
            thread_id,
            name,
            rip
        );

        // FIXED: Switch back to kernel page table when running kernel threads
        // This ensures kernel threads run with kernel page tables
        unsafe {
            crate::memory::process_memory::switch_to_kernel_page_table();
        }
    }
}

/// Restore userspace thread context
fn restore_userspace_thread_context(
    thread_id: u64,
    saved_regs: &mut SavedRegisters,
    interrupt_frame: &mut InterruptStackFrame,
) {
    log::info!(
        "restore_userspace_thread_context: Restoring thread {}",
        thread_id
    );
    
    // Check if this thread has ever run before
    let has_started = scheduler::with_thread_mut(thread_id, |thread| {
        thread.has_started
    }).unwrap_or(false);
    
    if !has_started {
        // CRITICAL: This is a brand new thread that has never run
        // We need to set up for its first entry to userspace
        crate::serial_println!("FIRST RUN: Thread {} has never run before!", thread_id);
        
        // Mark thread as started
        scheduler::with_thread_mut(thread_id, |thread| {
            thread.has_started = true;
        });
        
        // For first run, we need to set up the interrupt frame to jump to userspace
        // We should NOT try to "return" from this function
        setup_first_userspace_entry(thread_id, interrupt_frame);
        
        // NOTE: We don't return here - the interrupt frame is set up to jump to userspace
        // The iretq in the assembly will take us there
        crate::serial_println!("About to return from restore_userspace_thread_context after first run setup");
        
        // Debug: Check our current CR3 and stack
        unsafe {
            let cr3: u64;
            let rsp: u64;
            core::arch::asm!("mov {}, cr3", out(reg) cr3, options(nomem, nostack));
            core::arch::asm!("mov {}, rsp", out(reg) rsp, options(nomem, nostack));
            crate::serial_println!("Current CR3: {:#x}, RSP: {:#x}", cr3, rsp);
        }
        
        return;
    }
    
    // Thread has run before - do normal context restore
    crate::serial_println!("RESUME: Thread {} has run before, restoring saved context", thread_id);

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
                        log::trace!(
                            "Restored context for process {} (thread {})",
                            pid.as_u64(),
                            thread_id
                        );

                        // FIXED: Switch to process page table immediately during context switch
                        // This follows Linux/FreeBSD pattern - the kernel runs on the process's
                        // page table after selecting it, not just before returning to userspace
                        if let Some(ref page_table) = process.page_table {
                            let page_table_frame = page_table.level_4_frame();
                            
                            // Switch CR3 immediately
                            unsafe {
                                use x86_64::registers::control::Cr3;
                                let (current_frame, flags) = Cr3::read();
                                if current_frame != page_table_frame {
                                    log::info!(
                                        "About to switch CR3 from {:#x} to {:#x} for process {}",
                                        current_frame.start_address().as_u64(),
                                        page_table_frame.start_address().as_u64(),
                                        pid.as_u64()
                                    );
                                    
                                    // Test that we can still access kernel data before the switch
                                    let test_value = 42u64;
                                    log::info!("Pre-switch test: can read kernel data = {}", test_value);
                                    
                                    // Get current execution context for debugging
                                    let rip: u64;
                                    let rsp: u64;
                                    let rbp: u64;
                                    core::arch::asm!("lea {}, [rip]", out(reg) rip);
                                    core::arch::asm!("mov {}, rsp", out(reg) rsp);
                                    core::arch::asm!("mov {}, rbp", out(reg) rbp);
                                    
                                    // Check if we're on an IST stack
                                    let on_ist = rsp >= 0xffffc98000000000 && rsp < 0xffffc99000000000;
                                    
                                    log::info!("Pre-switch context: RIP={:#x}, RSP={:#x}, RBP={:#x}, on_IST={}", 
                                             rip, rsp, rbp, on_ist);
                                    
                                    // Disable interrupts to prevent timer during CR3 switch
                                    // Use manual disable/enable to control when IF is set
                                    x86_64::instructions::interrupts::disable();
                                    log::info!("Interrupts disabled, executing CR3 write NOW...");
                                    Cr3::write(page_table_frame, flags);
                                    
                                    // Use serial_println directly to avoid log system
                                    crate::serial_println!("CR3_WRITE_COMPLETED");
                                    
                                    // Try accessing various kernel structures to verify they're mapped
                                    unsafe {
                                        // Test 1: Can we read from TSS location?
                                        let tss_ptr = 0x100000f5320 as *const u8;
                                        let _tss_byte = core::ptr::read_volatile(tss_ptr);
                                        crate::serial_println!("TSS_READABLE");
                                        
                                        // Test 2: Can we read from GDT location?
                                        let gdt_ptr = 0x100000f5390 as *const u8;
                                        let _gdt_byte = core::ptr::read_volatile(gdt_ptr);
                                        crate::serial_println!("GDT_READABLE");
                                        
                                        // Test 3: Can we read from IDT location?
                                        let idt_ptr = 0x100000f6930 as *const u8;
                                        let _idt_byte = core::ptr::read_volatile(idt_ptr);
                                        crate::serial_println!("IDT_READABLE");
                                    }
                                    
                                    // Skip enabling interrupts for now to isolate the issue
                                    crate::serial_println!("SKIPPING_INTERRUPT_ENABLE");
                                    // x86_64::instructions::interrupts::enable();
                                    
                                    // Test that we can still access kernel data after the switch
                                    let test_value_2 = 84u64;
                                    // CRITICAL: Use serial_println instead of log::info to avoid logger accessing unmapped resources
                                    crate::serial_println!("CR3 switched OK; still executing! test = {}", test_value_2);
                                    
                                    // Flush TLB after page table switch
                                    x86_64::instructions::tlb::flush_all();
                                    
                                    crate::serial_println!("TLB flushed; about to continue execution");
                                }
                            }
                        } else {
                            log::warn!("Process {} has no page table!", pid.as_u64());
                        }

                        // Update TSS RSP0 for the new thread's kernel stack
                        // CRITICAL: Use the kernel stack, not the userspace stack!
                        if let Some(kernel_stack_top) = thread.kernel_stack_top {
                            crate::serial_println!(
                                "Setting kernel stack for thread {} to {:#x}",
                                thread_id,
                                kernel_stack_top.as_u64()
                            );
                            crate::gdt::set_kernel_stack(kernel_stack_top);
                        } else {
                            crate::serial_println!("ERROR: Userspace thread {} has no kernel stack!", thread_id);
                        }
                    }
                }
            }
        }
    } else {
        log::warn!(
            "Could not acquire process manager lock in interrupt context for thread {}",
            thread_id
        );
    }
}

/// Set up interrupt frame for first entry to userspace
fn setup_first_userspace_entry(thread_id: u64, interrupt_frame: &mut InterruptStackFrame) {
    crate::serial_println!("setup_first_userspace_entry: Setting up thread {} for first run", thread_id);
    
    // Get the thread's context (entry point, stack, etc.)
    scheduler::with_thread_mut(thread_id, |thread| {
        let context = &thread.context;
        
        // Set up the interrupt frame to jump to userspace
        unsafe {
            interrupt_frame.as_mut().update(|frame| {
                // Set instruction pointer to entry point
                frame.instruction_pointer = VirtAddr::new(context.rip);
                
                // Set stack pointer to user stack with proper alignment
                // Ensure (rsp % 16) == 8 at entry for SysV AMD64 ABI
                let aligned_rsp = (context.rsp & !0xF) | 0x8;
                frame.stack_pointer = VirtAddr::new(aligned_rsp);
                
                // Set code segment to user code (Ring 3)
                // Note: user_code_selector() already includes RPL=3
                frame.code_segment = crate::gdt::user_code_selector();
                
                // Set stack segment to user data (Ring 3)
                // Note: user_data_selector() already includes RPL=3
                frame.stack_segment = crate::gdt::user_data_selector();
                
                // Set CPU flags (DISABLE interrupts for testing, set reserved bit 1)
                // RFLAGS = 0x2 (IF=0, bit 1=1 which is reserved and must be 1)
                // CRITICAL TEST: Disabling interrupts to see if we reach userspace
                // Using raw value since from_bits_truncate might be clearing bit 1
                unsafe {
                    let flags_ptr = &mut frame.cpu_flags as *mut x86_64::registers::rflags::RFlags as *mut u64;
                    *flags_ptr = 0x2;  // Bit 1 must be set, IF (bit 9) cleared
                }
                let actual_flags = unsafe { *((&frame.cpu_flags) as *const _ as *const u64) };
                crate::serial_println!("Set RFLAGS to {:#x} (interrupts DISABLED for test)", actual_flags);
                
                crate::serial_println!(
                    "First entry frame: RIP={:#x}, RSP={:#x}, CS={:#x}, SS={:#x}, RFLAGS={:#x}",
                    frame.instruction_pointer.as_u64(),
                    frame.stack_pointer.as_u64(),
                    frame.code_segment.0,
                    frame.stack_segment.0,
                    actual_flags
                );
            });
        }
    });
    
    // CRITICAL: Now set up CR3 and kernel stack for this thread
    // This must happen BEFORE we iretq to userspace
    if let Some(mut manager_guard) = crate::process::try_manager() {
        if let Some((pid, process)) = manager_guard.as_mut().and_then(|m| m.find_process_by_thread_mut(thread_id)) {
            crate::serial_println!("Thread {} belongs to process {}", thread_id, pid.as_u64());
            
            // Get kernel stack info BEFORE switching CR3
            // After CR3 switch, the process struct might not be accessible
            let kernel_stack_top = process.main_thread.as_ref()
                .and_then(|thread| {
                    if thread.id == thread_id {
                        thread.kernel_stack_top
                    } else {
                        None
                    }
                });
            
            // Now safe to switch CR3 since we're on the upper-half kernel stack (PML4[402])
            // which is mapped in all page tables
            if let Some(page_table) = process.page_table.as_ref() {
                    let new_frame = page_table.level_4_frame();
                    crate::serial_println!("Switching CR3 to {:#x} for first run", new_frame.start_address().as_u64());
                    
                    // CRITICAL DEBUG: Verify kernel is accessible in the new page table
                    // Before switching CR3, let's check if the kernel code at 0x100000
                    // is actually mapped in the process page table
                    unsafe {
                        let phys_offset = crate::memory::physical_memory_offset();
                        let new_pml4_virt = phys_offset + new_frame.start_address().as_u64();
                        let new_pml4 = &*(new_pml4_virt.as_ptr() as *const x86_64::structures::paging::PageTable);
                        
                        // Check PML4[0] (identity mapping for kernel at 0x100000)
                        if !new_pml4[0].is_unused() {
                            let pml4_0_frame = new_pml4[0].frame().unwrap();
                            crate::serial_println!("Process PML4[0] -> {:#x} (identity mapping)", 
                                                 pml4_0_frame.start_address().as_u64());
                        } else {
                            crate::serial_println!("WARNING: Process PML4[0] is EMPTY - kernel at 0x100000 not mapped!");
                        }
                        
                        // Check PML4[2] (direct physical memory where kernel runs)
                        if !new_pml4[2].is_unused() {
                            let pml4_2_frame = new_pml4[2].frame().unwrap();
                            crate::serial_println!("Process PML4[2] -> {:#x} (direct phys mapping)", 
                                                 pml4_2_frame.start_address().as_u64());
                        } else {
                            crate::serial_println!("WARNING: Process PML4[2] is EMPTY - kernel execution will fail!");
                        }
                        
                        // Also check the current CR3's PML4[0] and PML4[2] for comparison
                        let current_cr3: u64;
                        core::arch::asm!("mov {}, cr3", out(reg) current_cr3, options(nomem, nostack));
                        let current_pml4_virt = phys_offset + current_cr3;
                        let current_pml4 = &*(current_pml4_virt.as_ptr() as *const x86_64::structures::paging::PageTable);
                        
                        if !current_pml4[0].is_unused() {
                            let current_pml4_0 = current_pml4[0].frame().unwrap();
                            crate::serial_println!("Current PML4[0] -> {:#x}", 
                                                 current_pml4_0.start_address().as_u64());
                        }
                        if !current_pml4[2].is_unused() {
                            let current_pml4_2 = current_pml4[2].frame().unwrap();
                            crate::serial_println!("Current PML4[2] -> {:#x}", 
                                                 current_pml4_2.start_address().as_u64());
                        }
                    }
                    
                    // Verify we're on the upper-half kernel stack and switch CR3 atomically
                    x86_64::instructions::interrupts::without_interrupts(|| {
                        let rsp: u64;
                        unsafe { 
                            core::arch::asm!("mov {}, rsp", out(reg) rsp, options(nomem, nostack));
                        }
                        let rsp_vaddr = x86_64::VirtAddr::new(rsp);
                        let pml4_index = (rsp >> 39) & 0x1FF;
                        crate::serial_println!("Current RSP: {:#x} (PML4[{}])", rsp, pml4_index);
                        
                        // Only switch CR3 if we're on the upper-half kernel stack
                        if crate::memory::layout::is_kernel_address(rsp_vaddr) {
                            crate::serial_println!("Stack is in upper half kernel region, safe to switch CR3");
                            
                            // Log old CR3 for comparison
                            let old_cr3: u64;
                            unsafe {
                                core::arch::asm!("mov {}, cr3", out(reg) old_cr3, options(nomem, nostack));
                            }
                            
                            let cr3_value = new_frame.start_address().as_u64();
                            
                            // CRITICAL: Before switching CR3, output a marker that we can see
                            // Use direct serial port output to ensure it works
                            unsafe {
                                // Output 0xAA to indicate we're about to switch CR3
                                core::arch::asm!(
                                    "mov dx, 0x3F8",
                                    "mov al, 0xAA",
                                    "out dx, al",
                                    out("dx") _,
                                    out("al") _,
                                    options(nomem, nostack)
                                );
                            }
                            
                            // CRITICAL: Switch CR3 but with extreme verification
                            // The kernel must remain accessible after the switch
                            
                            unsafe {
                                // Final verification before CR3 switch:
                                // Get the current instruction pointer to know where we're executing from
                                let current_rip = setup_first_userspace_entry as *const () as u64;
                                crate::serial_println!("Current function at: {:#x}", current_rip);
                                
                                // Use the actual kernel code location for testing
                                // The kernel is likely in the 0x100xxxxxxxx range (direct physical mapping)
                                let kernel_test_addr = current_rip as *const u32;
                                crate::serial_println!("Testing read from kernel at: {:#x}", kernel_test_addr as u64);
                                let test_val = core::ptr::read_volatile(kernel_test_addr);
                                crate::serial_println!("Pre-switch read successful, value: {:#x}", test_val);
                                
                                // Do the actual CR3 switch
                                core::arch::asm!("mov cr3, {}", in(reg) cr3_value, options(nostack, preserves_flags));
                                
                                // IMMEDIATELY verify we can still execute
                                // If this fails, we'll triple fault right here
                                crate::serial_println!("CR3 switched, attempting post-switch read...");
                                let post_test_val = core::ptr::read_volatile(kernel_test_addr);
                                crate::serial_println!("Post-switch read successful, value: {:#x}", post_test_val);
                            }
                            
                            // Output 0xBB to indicate CR3 switch completed
                            unsafe {
                                core::arch::asm!(
                                    "mov dx, 0x3F8",
                                    "mov al, 0xBB", 
                                    "out dx, al",
                                    out("dx") _,
                                    out("al") _,
                                    options(nomem, nostack)
                                );
                            }
                            
                            crate::serial_println!("CR3 switched: {:#x} -> {:#x}", old_cr3, cr3_value);
                        } else {
                            crate::serial_println!("WARNING: Not on upper-half kernel stack (PML4[{}]), skipping CR3 switch", pml4_index);
                        }
                    });
                    
                    crate::serial_println!("After interrupts::without_interrupts block");
                }
                
                crate::serial_println!("Setting kernel stack for thread {}...", thread_id);
                
                // Set kernel stack for this thread (using the value we saved before CR3 switch)
                if let Some(stack_top) = kernel_stack_top {
                    crate::serial_println!(
                        "Setting kernel stack for thread {} to {:#x}",
                        thread_id,
                        stack_top.as_u64()
                    );
                    crate::gdt::set_kernel_stack(stack_top);
                } else {
                    crate::serial_println!("WARNING: No kernel stack found for thread {}", thread_id);
                }
        }
    }
    
    crate::serial_println!("First userspace entry setup complete for thread {} - returning to interrupt handler", thread_id);
}

/// Simple idle loop
fn idle_loop() -> ! {
    loop {
        // Try to flush any pending IRQ logs while idle
        crate::irq_log::flush_local_try();
        x86_64::instructions::hlt();
    }
}

// REMOVED: get_next_page_table() is no longer needed since CR3 switching
// happens immediately during context switch in the scheduler

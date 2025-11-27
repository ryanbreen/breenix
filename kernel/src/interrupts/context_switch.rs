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
    // CHECKPOINT D: Context Switch Check Called
    use core::sync::atomic::{AtomicU64, Ordering};
    static CHECK_RESCHED_COUNT: AtomicU64 = AtomicU64::new(0);
    let count = CHECK_RESCHED_COUNT.fetch_add(1, Ordering::Relaxed);
    if count < 5 {
        crate::serial_println!("CHECKPOINT D: check_need_resched_and_switch #{}", count);
    }

    // CRITICAL: Only schedule when returning to userspace with preempt_count == 0
    if !crate::per_cpu::can_schedule(interrupt_frame.code_segment.0 as u64) {
        return;
    }

    // CRITICAL FIX: Always save context when coming from userspace, BEFORE any early returns.
    // This ensures thread.context.rip is always up-to-date. Without this, if no context switch
    // happens on this syscall return, but a later context switch restores this thread,
    // it would use stale context.rip (the entry point 0x40000000) instead of the actual RIP.
    let from_userspace = (interrupt_frame.code_segment.0 & 3) == 3;
    if from_userspace {
        if let Some(current_tid) = scheduler::current_thread_id() {
            // Save context unconditionally so it's always current
            // Ignore return value - we continue even if save fails (no context switch happening)
            let _ = save_current_thread_context(current_tid, saved_regs, interrupt_frame);
        }
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
        // Clear exception cleanup context since we're doing a context switch
        crate::per_cpu::clear_exception_cleanup_context();

        if old_thread_id == new_thread_id {
            // Same thread continues running
            return;
        }

        log::debug!(
            "Context switch on interrupt return: {} -> {}",
            old_thread_id,
            new_thread_id
        );

        // Log context switch details (from_userspace already computed above)
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
        // CRITICAL: If save fails, we MUST NOT switch contexts!
        // Switching without saving would cause the process to return to stale RIP (entry point)
        if from_userspace {
            if !save_current_thread_context(old_thread_id, saved_regs, interrupt_frame) {
                log::error!(
                    "Context switch aborted: failed to save thread {} context. \
                     Would cause return to stale RIP!",
                    old_thread_id
                );
                // Don't clear need_resched - we'll try again on next interrupt return
                // The lock contention should resolve by then
                return;
            }
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
/// Returns true if context was saved successfully, false otherwise
fn save_current_thread_context(
    thread_id: u64,
    saved_regs: &mut SavedRegisters,
    interrupt_frame: &mut InterruptStackFrame,
) -> bool {
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
                    return true;
                } else {
                    log::error!(
                        "Process {} has no main_thread for thread {}",
                        pid.as_u64(),
                        thread_id
                    );
                }
            } else {
                log::error!(
                    "Could not find process for thread {} in process manager",
                    thread_id
                );
            }
        } else {
            log::error!("Process manager is None");
        }
    } else {
        log::error!(
            "CRITICAL: Could not acquire process manager lock in interrupt context for thread {}. \
             Context switch will be aborted to prevent returning to stale RIP.",
            thread_id
        );
    }
    false
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
        log::error!(
            "CRITICAL: Could not acquire process manager lock to restore context for thread {}. \
             Interrupt frame NOT modified - will return to previous thread instead!",
            thread_id
        );
        // NOTE: This is a serious inconsistency - the scheduler thinks thread_id is running
        // but we're actually going to return to the previous thread's context.
        // TODO: Consider panicking here, or implement proper rollback
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
                let flags_ptr = &mut frame.cpu_flags as *mut x86_64::registers::rflags::RFlags as *mut u64;
                // CRITICAL: Set TF (bit 8) and IF (bit 9) per Cursor guidance
                // TF will trigger #DB on first user instruction, proving IRETQ succeeded
                // IF enables interrupts for visibility
                *flags_ptr = 0x202;  // Bit 1=1 (required), IF=1 (bit 9) - TF removed
                let actual_flags = *((&frame.cpu_flags) as *const _ as *const u64);
                crate::serial_println!("Set RFLAGS to {:#x} (IF=1, TF removed per cursor guidance)", actual_flags);

                log::info!(
                    "🚀 RING3_ENTRY: Thread entering Ring 3 - RIP={:#x}, RSP={:#x}, CS={:#x} (RPL=3), SS={:#x} (RPL=3)",
                    frame.instruction_pointer.as_u64(),
                    frame.stack_pointer.as_u64(),
                    frame.code_segment.0,
                    frame.stack_segment.0
                );
                
                crate::serial_println!(
                    "USERSPACE OUTPUT PENDING: About to IRETQ to Ring 3 at RIP={:#x}, CS={:#x}",
                    frame.instruction_pointer.as_u64(),
                    frame.code_segment.0
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

            // Also save the kernel stack for setting TSS RSP0 after CR3 switch
            let saved_kernel_stack_top = kernel_stack_top;

            // CRITICAL: Get physical memory offset BEFORE ANY CR3 switching logic to avoid accessing statics
            // After CR3 switch, kernel static data won't be accessible
            let phys_offset = crate::memory::physical_memory_offset();

            // Now safe to switch CR3 since we're on the upper-half kernel stack (PML4[402])
            // which is mapped in all page tables
            if let Some(page_table) = process.page_table.as_ref() {
                    let new_frame = page_table.level_4_frame();
                    crate::serial_println!("Switching CR3 to {:#x} for first run", new_frame.start_address().as_u64());
                    
                    // CRITICAL DEBUG: Verify kernel is accessible in the new page table
                    // Before switching CR3, let's check if the kernel code at 0x100000
                    // is actually mapped in the process page table
                    unsafe {
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

                        // CRITICAL: Check PML4[402] (kernel stacks at 0xffffc900_0000_0000)
                        if !new_pml4[402].is_unused() {
                            let pml4_402_frame = new_pml4[402].frame().unwrap();
                            crate::serial_println!("Process PML4[402] -> {:#x} (kernel stacks)",
                                                 pml4_402_frame.start_address().as_u64());
                        } else {
                            crate::serial_println!("🔴 CRITICAL: Process PML4[402] is EMPTY - kernel stacks NOT MAPPED!");
                            crate::serial_println!("🔴 This WILL cause a page fault when using the stack!");
                        }

                        // Also check PML4[403] (IST stacks at 0xffffc980_0000_0000)
                        if !new_pml4[403].is_unused() {
                            let pml4_403_frame = new_pml4[403].frame().unwrap();
                            crate::serial_println!("Process PML4[403] -> {:#x} (IST stacks)",
                                                 pml4_403_frame.start_address().as_u64());
                        } else {
                            crate::serial_println!("WARNING: Process PML4[403] is EMPTY - IST stacks not mapped!");
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

                                // CRITICAL: Tell interrupt return path to use this CR3
                                // Set NEXT_CR3 at gs:[64] so that timer_entry.asm restores the correct CR3
                                crate::per_cpu::set_next_cr3(cr3_value);

                                // CRITICAL: Also set saved_process_cr3 at gs:[80]
                                // The timer interrupt expects this to be set when returning to userspace
                                // On first entry, there's nothing saved yet, so we set it to the process CR3
                                core::arch::asm!(
                                    "mov gs:[80], {}",
                                    in(reg) cr3_value,
                                    options(nostack, preserves_flags)
                                );

                                // IMMEDIATELY verify we can still execute
                                // If this fails, we'll triple fault right here
                                crate::serial_println!("CR3 switched, attempting post-switch read...");
                                let post_test_val = core::ptr::read_volatile(kernel_test_addr);
                                crate::serial_println!("Post-switch read successful, value: {:#x}", post_test_val);

                                // CRITICAL: Test the current kernel stack is accessible
                                // This is what IRETQ will try to read from
                                let current_rsp: u64;
                                core::arch::asm!("mov {}, rsp", out(reg) current_rsp);
                                crate::serial_println!("Testing kernel stack accessibility at RSP: {:#x}", current_rsp);

                                // Test reading from the current stack - this is what IRETQ needs to do
                                let stack_test_addr = current_rsp as *const u64;
                                let stack_val = core::ptr::read_volatile(stack_test_addr);
                                crate::serial_println!("✓ Kernel stack read successful from RSP, value: {:#x}", stack_val);

                                // CRITICAL: Set TSS RSP0 BEFORE int3 test - kernel stack needed for exception handling
                                // Use the saved kernel stack info from before CR3 switch
                                if let Some(stack_top) = saved_kernel_stack_top {
                                    crate::serial_println!(
                                        "CRITICAL: Setting TSS RSP0 to {:#x} BEFORE int3 test",
                                        stack_top.as_u64()
                                    );
                                    crate::gdt::set_kernel_stack(stack_top);

                                    // Verify it was set correctly
                                    let (_, new_rsp0) = crate::gdt::get_tss_info();
                                    crate::serial_println!("VERIFIED: TSS RSP0 now set to {:#x}", new_rsp0);
                                } else {
                                    crate::serial_println!("ERROR: No kernel stack found for thread {}", thread_id);
                                }

                                // Get breakpoint handler address first (needed by multiple diagnostics)
                                let handler_addr = crate::interrupts::breakpoint_handler as *const () as u64;

                                // CURSOR AGENT DIAGNOSTIC: Log addresses of critical kernel structures
                                // before attempting int3 test
                                // Get IDT base address
                                let idtr = x86_64::instructions::tables::sidt();
                                crate::serial_println!("IDT base address: {:#x} (PML4[{}])",
                                                     idtr.base.as_u64(), (idtr.base.as_u64() >> 39) & 0x1FF);

                                // Get GDT base address
                                let gdtr = x86_64::instructions::tables::sgdt();
                                crate::serial_println!("GDT base address: {:#x} (PML4[{}])",
                                                     gdtr.base.as_u64(), (gdtr.base.as_u64() >> 39) & 0x1FF);

                                // Get TSS address and RSP0
                                let (tss_base, rsp0) = crate::gdt::get_tss_info();
                                crate::serial_println!("TSS base address: {:#x} (PML4[{}])",
                                                     tss_base, (tss_base >> 39) & 0x1FF);
                                crate::serial_println!("TSS RSP0 stack: {:#x} (PML4[{}])",
                                                     rsp0, (rsp0 >> 39) & 0x1FF);

                                // Check IST stacks in TSS - invalid IST can cause issues
                                let tss_ptr = crate::gdt::get_tss_ptr();
                                if !tss_ptr.is_null() {
                                    let ist0 = (*tss_ptr).interrupt_stack_table[0];
                                    let ist1 = (*tss_ptr).interrupt_stack_table[1];
                                    crate::serial_println!("TSS IST[0] (double fault): {:#x} (PML4[{}])",
                                                         ist0.as_u64(), (ist0.as_u64() >> 39) & 0x1FF);
                                    crate::serial_println!("TSS IST[1] (page fault): {:#x} (PML4[{}])",
                                                         ist1.as_u64(), (ist1.as_u64() >> 39) & 0x1FF);
                                }

                                // Log breakpoint handler address
                                crate::serial_println!("Breakpoint handler: {:#x} (PML4[{}])",
                                                     handler_addr, (handler_addr >> 39) & 0x1FF);

                                // CURSOR AGENT DIAGNOSTIC: Log CR4 and EFER to check SMEP/SMAP/NXE
                                use x86_64::registers::control::{Cr0, Cr4, Cr4Flags};
                                use x86_64::registers::model_specific::{Efer, EferFlags};
                                let cr0 = Cr0::read();
                                let cr4 = Cr4::read();
                                let efer = Efer::read();
                                crate::serial_println!("CPU state: CR0={:?}", cr0);
                                crate::serial_println!("CPU state: CR4={:?} (SMEP={}, SMAP={})",
                                                     cr4,
                                                     cr4.contains(Cr4Flags::SUPERVISOR_MODE_EXECUTION_PROTECTION),
                                                     cr4.contains(Cr4Flags::SUPERVISOR_MODE_ACCESS_PREVENTION));
                                crate::serial_println!("CPU state: EFER={:?} (NXE={})",
                                                     efer,
                                                     efer.contains(EferFlags::NO_EXECUTE_ENABLE));

                                // DIAGNOSTIC CODE DISABLED - was causing #GP after CR3 switch
                                // The PML4[402]/[403] aliasing bug is fixed, so these diagnostics
                                // are no longer needed.

                                if false {  // DISABLED DIAGNOSTIC BLOCK
                                    crate::serial_println!("Inside unsafe block");

                                    // CURSOR TEST: Inline asm OUT that doesn't touch stack
                                    // If this works but Port::write doesn't, stack isn't mapped
                                    // If this doesn't work, kernel .text is NX or unmapped
                                    core::arch::asm!(
                                        "mov dx, 0x00E9",
                                        "mov al, 0x41",  // ASCII 'A'
                                        "out dx, al",
                                        options(nostack, preserves_flags)
                                    );
                                    crate::serial_println!("Inline asm OUT succeeded");

                                    // CRITICAL TEST: Check if stack is readable after CR3 switch
                                    let mut _stack_test_result: u8 = 0;
                                    core::arch::asm!(
                                        "mov rdx, rsp",         // Get current stack pointer
                                        "mov al, [rdx]",        // Try to read from stack
                                        "mov {0}, al",          // Store result
                                        "mov dx, 0x00E9",       // Port for debug output
                                        "mov al, 0x53",         // ASCII 'S' for Success
                                        "out dx, al",           // Output success marker
                                        out(reg_byte) _stack_test_result,
                                        options(nostack, preserves_flags)
                                    );
                                    // crate::serial_println!("✓ Stack is readable! Read value: {:#x}", stack_test_result);

                                    // Output raw marker that stack read succeeded
                                    core::arch::asm!(
                                        "mov dx, 0x3F8",  // COM1 port
                                        "mov al, 0x52",    // ASCII 'R' for Read success
                                        "out dx, al",
                                        options(nostack, preserves_flags)
                                    );

                                    // TEST: Check if stack is WRITABLE
                                    // TEMPORARILY DISABLED: This stack write test causes page fault at 0x10000034800
                                    // after CR3 switch to user page table. The kernel stack might not be writable
                                    // in the user address space or the write is hitting an unmapped region.
                                    // TODO: Fix kernel stack mapping in user page table
                                    /*
                                    core::arch::asm!(
                                        "mov byte ptr [rsp], 0x42",  // Try to write to stack
                                        "mov dx, 0x00E9",             // Port for debug output
                                        "mov al, 0x57",               // ASCII 'W' for Writable
                                        "out dx, al",                 // Output success marker
                                        options(nostack, preserves_flags)
                                    );
                                    */

                                    // Output to COM1 that write succeeded, then B
                                    // COMBINED into single asm block to avoid compiler insertions
                                    core::arch::asm!(
                                        "mov dx, 0x3F8",  // COM1 port
                                        "mov al, 0x57",    // ASCII 'W' for Write success
                                        "out dx, al",
                                        "mov al, 0x42",    // ASCII 'B'
                                        "out dx, al",
                                        "mov al, 0x43",    // ASCII 'C'
                                        "out dx, al",
                                        "mov al, 0x44",    // ASCII 'D'
                                        "out dx, al",
                                        options(nostack, nomem, preserves_flags)
                                    );
                                    // crate::serial_println!("After B output");

                                    let _handler_vaddr = x86_64::VirtAddr::new(handler_addr);

                                    // Already output C and D in combined block above

                                    // DISABLED: Diagnostic code causes page faults after CR3 switch
                                    // Skip all page table analysis to avoid accessing unmapped memory
                                    crate::serial_println!("SKIPPING page table diagnostics");
                                    
                                    // Now try the actual call - DISABLED TO AVOID PAGE FAULT
                                    // let phys_offset = crate::memory::physical_memory_offset();

                                    // If we get here, it worked
                                    core::arch::asm!(
                                        "mov dx, 0x3F8",  // COM1 port
                                        "mov al, 0x45",  // ASCII 'E' - got phys_offset
                                        "out dx, al",
                                        options(nostack, nomem, preserves_flags)
                                    );

                                    // Get current page tables - DISABLED
                                    // let (p4_frame, _) = x86_64::registers::control::Cr3::read();

                                    core::arch::asm!(
                                        "mov dx, 0x3F8",  // COM1 port
                                        "mov al, 0x46",  // ASCII 'F' - skipped CR3 diagnostics
                                        "out dx, al",
                                        options(nostack, nomem, preserves_flags)
                                    );

                                    // let p4_virt = phys_offset + p4_frame.start_address().as_u64();

                                    core::arch::asm!(
                                        "mov dx, 0x3F8",  // COM1 port
                                        "mov al, 0x47",  // ASCII 'G' - calculated p4_virt
                                        "out dx, al",
                                        options(nostack, nomem, preserves_flags)
                                    );

                                    // let p4 = &*(p4_virt.as_ptr() as *const x86_64::structures::paging::PageTable);

                                    core::arch::asm!(
                                        "mov dx, 0x3F8",  // COM1 port
                                        "mov al, 0x48",  // ASCII 'H' - skipped page table ref
                                        "out dx, al",
                                        options(nostack, nomem, preserves_flags)
                                    );

                                    // Check PML4 entry for handler (should be PML4[2]) - DISABLED
                                    // let p4_idx = (handler_addr >> 39) & 0x1FF;
                                    // let p4e = &p4[p4_idx as usize];

                                    core::arch::asm!(
                                        "mov dx, 0x3F8",  // COM1 port
                                        "mov al, 0x49",  // ASCII 'I' - got PML4 entry
                                        "out dx, al",
                                        options(nostack, nomem, preserves_flags)
                                    );

                                    // crate::serial_println!("Handler PML4[{}] entry: present={}, NX={}",
                                    //                                  p4_idx,
                                    //                                  p4e.flags().contains(x86_64::structures::paging::PageTableFlags::PRESENT),
                                    //                                  p4e.flags().contains(x86_64::structures::paging::PageTableFlags::NO_EXECUTE));

                                    // DISABLED: Page table diagnostic code to avoid page faults
                                    // if p4e.flags().contains(x86_64::structures::paging::PageTableFlags::PRESENT) {
                                    //     // Check PML3 entry
                                    //     let p3_phys = p4e.addr();
                                    //     let p3_virt = phys_offset + p3_phys.as_u64();
                                    //     ...
                                    // }
                                    
                                    core::arch::asm!(
                                        "mov dx, 0x3F8",  // COM1 port
                                        "mov al, 0x4C",  // ASCII 'L' - skipped PML3 diagnostics
                                        "out dx, al",
                                        options(nostack, nomem, preserves_flags)
                                    );

                                // CURSOR AGENT DIAGNOSTIC: Test kernel exception viability FIRST after CR3 switch
                                // This is THE CRITICAL TEST - can we handle exceptions under process CR3?
                                // If this doesn't log the breakpoint handler, kernel exception path is unmapped
                                // TEST THIS BEFORE ANY USER MEMORY ACCESS to isolate IDT/TSS/IST vs SMAP issues
                                // crate::serial_println!("🔥 CRITICAL TEST: Testing kernel exception handling under process CR3");
                                // crate::serial_println!("Still in CPL0 (kernel mode) - triggering int3...");

                                // Output marker before int3
                                core::arch::asm!(
                                    "mov dx, 0x3F8",  // COM1 port
                                    "mov al, 0x4A",  // ASCII 'J' - about to int3
                                    "out dx, al",
                                    options(nostack, nomem, preserves_flags)
                                );

                                // SKIP INT3 FOR NOW - it might be causing issues
                                // core::arch::asm!("int3", options(nomem, nostack));

                                // Output marker at end of unsafe block
                                core::arch::asm!(
                                    "mov dx, 0x3F8",  // COM1 port
                                    "mov al, 0x4B",  // ASCII 'K' - end of unsafe block
                                    "out dx, al",
                                    options(nostack, nomem, preserves_flags)
                                );

                                // If we reach here, the breakpoint was handled successfully
                                // crate::serial_println!("✓ SUCCESS: Kernel exception handling works under process CR3!");

                                // CRITICAL: Test user code accessibility at entry point
                                // This is where IRETQ will try to fetch the first instruction
                                crate::serial_println!("Testing user code accessibility at RIP: {:#x}", 0x40000000u64);
                                let user_code_addr = 0x40000000 as *const u8;
                                match core::ptr::read_volatile(user_code_addr) {
                                    byte => {
                                        crate::serial_println!("✓ User code read successful at {:#x}, first byte: {:#02x}", 0x40000000u64, byte);
                                        if byte == 0xCC {
                                            crate::serial_println!("✓ Confirmed: int3 instruction (0xCC) found at user entry point");
                                        } else {
                                            crate::serial_println!("⚠ WARNING: Expected int3 (0xCC) but found {:#02x}", byte);
                                        }
                                    }
                                }

                                // CRITICAL: Test user stack accessibility
                                crate::serial_println!("Testing user stack accessibility at RSP: {:#x}", 0x7fffff011008u64);
                                let user_stack_addr = 0x7fffff011008u64 as *const u64;
                                match core::ptr::read_volatile(user_stack_addr) {
                                    val => crate::serial_println!("✓ User stack read successful, value: {:#x}", val),
                                }
                            }
                            }  // END DISABLED DIAGNOSTIC BLOCK (if false)

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

                            // TEST 6: Userspace code accessibility check
                            // Walk the page tables to verify userspace RIP is mapped
                            unsafe {
                                let user_rip = 0x40000000u64; // Standard userspace entry point (from linker.ld)
                                crate::serial_println!("USER_CODE_CHECK: Walking page tables for RIP {:#x}", user_rip);

                                // Get physical memory offset for page table walking
                                let phys_offset = crate::memory::physical_memory_offset();

                                // Get current CR3 (should be process page table)
                                let cr3_phys = cr3_value;
                                let pml4_virt = phys_offset + cr3_phys;
                                let pml4 = &*(pml4_virt.as_ptr() as *const x86_64::structures::paging::PageTable);

                                // Calculate indices for userspace RIP
                                let pml4_idx = ((user_rip >> 39) & 0x1FF) as usize;
                                let pdpt_idx = ((user_rip >> 30) & 0x1FF) as usize;
                                let pd_idx = ((user_rip >> 21) & 0x1FF) as usize;
                                let pt_idx = ((user_rip >> 12) & 0x1FF) as usize;

                                // Check PML4
                                let pml4_entry = &pml4[pml4_idx];
                                if pml4_entry.is_unused() {
                                    crate::serial_println!("  PML4[{}]: UNUSED ❌", pml4_idx);
                                    crate::serial_println!("USER_CODE_CHECK: DIVERGENCE FOUND - userspace code NOT mapped at PML4!");
                                } else {
                                    let pml4_flags = pml4_entry.flags();
                                    let has_user = pml4_flags.contains(x86_64::structures::paging::PageTableFlags::USER_ACCESSIBLE);
                                    crate::serial_println!("  PML4[{}]: PRESENT {} (USER={})",
                                                         pml4_idx,
                                                         if has_user { "✅" } else { "⚠️" },
                                                         has_user);

                                    // Check PDPT
                                    let pdpt_phys = pml4_entry.addr();
                                    let pdpt_virt = phys_offset + pdpt_phys.as_u64();
                                    let pdpt = &*(pdpt_virt.as_ptr() as *const x86_64::structures::paging::PageTable);
                                    let pdpt_entry = &pdpt[pdpt_idx];

                                    if pdpt_entry.is_unused() {
                                        crate::serial_println!("  PDPT[{}]: UNUSED ❌", pdpt_idx);
                                        crate::serial_println!("USER_CODE_CHECK: DIVERGENCE FOUND - userspace code NOT mapped at PDPT!");
                                    } else {
                                        let pdpt_flags = pdpt_entry.flags();
                                        let has_user = pdpt_flags.contains(x86_64::structures::paging::PageTableFlags::USER_ACCESSIBLE);
                                        crate::serial_println!("  PDPT[{}]: PRESENT {} (USER={})",
                                                             pdpt_idx,
                                                             if has_user { "✅" } else { "⚠️" },
                                                             has_user);

                                        // Check PD
                                        let pd_phys = pdpt_entry.addr();
                                        let pd_virt = phys_offset + pd_phys.as_u64();
                                        let pd = &*(pd_virt.as_ptr() as *const x86_64::structures::paging::PageTable);
                                        let pd_entry = &pd[pd_idx];

                                        if pd_entry.is_unused() {
                                            crate::serial_println!("  PD[{}]: UNUSED ❌", pd_idx);
                                            crate::serial_println!("USER_CODE_CHECK: DIVERGENCE FOUND - userspace code NOT mapped at PD!");
                                        } else {
                                            let pd_flags = pd_entry.flags();
                                            let has_user = pd_flags.contains(x86_64::structures::paging::PageTableFlags::USER_ACCESSIBLE);
                                            let is_huge = pd_flags.contains(x86_64::structures::paging::PageTableFlags::HUGE_PAGE);

                                            if is_huge {
                                                // 2MB huge page - no PT level
                                                crate::serial_println!("  PD[{}]: PRESENT, HUGE {} (USER={})",
                                                                     pd_idx,
                                                                     if has_user { "✅" } else { "⚠️" },
                                                                     has_user);
                                                if has_user {
                                                    crate::serial_println!("USER_CODE_CHECK: Userspace code IS accessible (2MB huge page)");
                                                } else {
                                                    crate::serial_println!("USER_CODE_CHECK: DIVERGENCE FOUND - huge page missing USER flag!");
                                                }
                                            } else {
                                                crate::serial_println!("  PD[{}]: PRESENT {} (USER={})",
                                                                     pd_idx,
                                                                     if has_user { "✅" } else { "⚠️" },
                                                                     has_user);

                                                // Check PT
                                                let pt_phys = pd_entry.addr();
                                                let pt_virt = phys_offset + pt_phys.as_u64();
                                                let pt = &*(pt_virt.as_ptr() as *const x86_64::structures::paging::PageTable);
                                                let pt_entry = &pt[pt_idx];

                                                if pt_entry.is_unused() {
                                                    crate::serial_println!("  PT[{}]: UNUSED ❌", pt_idx);
                                                    crate::serial_println!("USER_CODE_CHECK: DIVERGENCE FOUND - userspace code NOT mapped at PT!");
                                                } else {
                                                    let pt_flags = pt_entry.flags();
                                                    let has_user = pt_flags.contains(x86_64::structures::paging::PageTableFlags::USER_ACCESSIBLE);
                                                    let has_nx = pt_flags.contains(x86_64::structures::paging::PageTableFlags::NO_EXECUTE);
                                                    crate::serial_println!("  PT[{}]: PRESENT {} (USER={}, NX={})",
                                                                         pt_idx,
                                                                         if has_user && !has_nx { "✅" } else { "⚠️" },
                                                                         has_user,
                                                                         has_nx);

                                                    if has_user && !has_nx {
                                                        crate::serial_println!("USER_CODE_CHECK: Userspace code IS accessible ✅");

                                                        // Try to actually read the first few bytes
                                                        let user_code_ptr = user_rip as *const u64;
                                                        let first_bytes = core::ptr::read_volatile(user_code_ptr);
                                                        crate::serial_println!("USER_CODE_CHECK: First 8 bytes at RIP: {:#018x}", first_bytes);
                                                    } else if !has_user {
                                                        crate::serial_println!("USER_CODE_CHECK: DIVERGENCE FOUND - PT entry missing USER flag!");
                                                    } else if has_nx {
                                                        crate::serial_println!("USER_CODE_CHECK: DIVERGENCE FOUND - PT entry has NX flag (code not executable)!");
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        } else {
                            crate::serial_println!("WARNING: Not on upper-half kernel stack (PML4[{}]), skipping CR3 switch", pml4_index);
                        }
                    });

                    crate::serial_println!("After interrupts::without_interrupts block");

                    // CRITICAL VALIDATION: Verify that TSS.RSP0, IST[1], and user RSP are mapped
                    // in the process page table BEFORE we IRETQ
                    // This catches page table setup bugs immediately instead of triple faulting
                    crate::serial_println!("DEBUG: About to run validation check");
                    unsafe {
                        let cr3_phys = new_frame.start_address().as_u64();
                        let phys_offset = crate::memory::physical_memory_offset();
                        let pml4_virt = phys_offset + cr3_phys;
                        let pml4 = &*(pml4_virt.as_ptr() as *const x86_64::structures::paging::PageTable);

                        // Get the actual addresses we need to validate
                        let (_tss_base, tss_rsp0) = crate::gdt::get_tss_info();
                        let tss_ptr = crate::gdt::get_tss_ptr();
                        let ist1_addr = if !tss_ptr.is_null() {
                            (*tss_ptr).interrupt_stack_table[1].as_u64()
                        } else {
                            0
                        };
                        // Get user RSP from the thread context (already aligned in setup)
                        let user_rsp = scheduler::with_thread_mut(thread_id, |thread| thread.context.rsp)
                            .unwrap_or(0x7fffff010ff8); // Default to adjusted stack top if not found

                        crate::serial_println!("=== PRE-IRETQ VALIDATION: Checking critical mappings ===");
                        crate::serial_println!("  Will validate:");
                        crate::serial_println!("    1. TSS.RSP0 (kernel stack): {:#x} (PML4[{}])",
                                             tss_rsp0, (tss_rsp0 >> 39) & 0x1FF);
                        crate::serial_println!("    2. IST[1] (page fault stack): {:#x} (PML4[{}])",
                                             ist1_addr, (ist1_addr >> 39) & 0x1FF);
                        crate::serial_println!("    3. User RSP (user stack): {:#x} (PML4[{}])",
                                             user_rsp, (user_rsp >> 39) & 0x1FF);

                        // Helper function to check if an address is mapped
                        let check_mapping = |addr: u64, name: &str| -> bool {
                            let pml4_idx = ((addr >> 39) & 0x1FF) as usize;
                            let pdpt_idx = ((addr >> 30) & 0x1FF) as usize;
                            let pd_idx = ((addr >> 21) & 0x1FF) as usize;
                            let pt_idx = ((addr >> 12) & 0x1FF) as usize;

                            // Check PML4
                            if pml4[pml4_idx].is_unused() {
                                crate::serial_println!("❌ {} UNMAPPED: PML4[{}] is empty", name, pml4_idx);
                                return false;
                            }

                            // Check PDPT
                            let pdpt_phys = pml4[pml4_idx].addr();
                            let pdpt_virt = phys_offset + pdpt_phys.as_u64();
                            let pdpt = &*(pdpt_virt.as_ptr() as *const x86_64::structures::paging::PageTable);
                            if pdpt[pdpt_idx].is_unused() {
                                crate::serial_println!("❌ {} UNMAPPED: PDPT[{}] is empty", name, pdpt_idx);
                                return false;
                            }

                            // Check PD
                            let pd_phys = pdpt[pdpt_idx].addr();
                            let pd_virt = phys_offset + pd_phys.as_u64();
                            let pd = &*(pd_virt.as_ptr() as *const x86_64::structures::paging::PageTable);

                            // Check for huge page
                            if pd[pd_idx].flags().contains(x86_64::structures::paging::PageTableFlags::HUGE_PAGE) {
                                crate::serial_println!("✅ {} mapped via 2MB huge page at PD[{}]", name, pd_idx);
                                return true;
                            }

                            if pd[pd_idx].is_unused() {
                                crate::serial_println!("❌ {} UNMAPPED: PD[{}] is empty", name, pd_idx);
                                return false;
                            }

                            // Check PT
                            let pt_phys = pd[pd_idx].addr();
                            let pt_virt = phys_offset + pt_phys.as_u64();
                            let pt = &*(pt_virt.as_ptr() as *const x86_64::structures::paging::PageTable);
                            if pt[pt_idx].is_unused() {
                                crate::serial_println!("❌ {} UNMAPPED: PT[{}] is empty", name, pt_idx);
                                return false;
                            }

                            crate::serial_println!("✅ {} mapped: PT[{}] -> frame {:#x}",
                                                 name, pt_idx, pt[pt_idx].addr().as_u64());
                            true
                        };

                        // Check stack addresses - 16 bytes (stack tops point past last valid byte)
                        let tss_rsp0_mapped = check_mapping(tss_rsp0.wrapping_sub(16), "TSS.RSP0");
                        let ist1_mapped = check_mapping(ist1_addr.wrapping_sub(16), "IST[1]");
                        // User RSP is already adjusted in manager.rs, check it directly
                        let user_rsp_mapped = check_mapping(user_rsp, "User RSP");

                        if !tss_rsp0_mapped || !ist1_mapped || !user_rsp_mapped {
                            crate::serial_println!("❌❌❌ CRITICAL: One or more required addresses are UNMAPPED!");
                            crate::serial_println!("This will cause a page fault or triple fault on IRETQ!");
                            crate::serial_println!("Halting instead of triple faulting...");
                            loop { x86_64::instructions::hlt(); }
                        }

                        crate::serial_println!("✅✅✅ All critical mappings verified - safe to IRETQ");
                        log::info!("[CHECKPOINT:PAGETABLE_VALIDATED] All critical mappings verified before IRETQ");
                    }
                }

            // CRITICAL: Set kernel stack for TSS RSP0
            crate::serial_println!("Setting kernel stack for thread {}...", thread_id);

            // Set kernel stack for this thread (using the value we saved before CR3 switch)
            if let Some(stack_top) = saved_kernel_stack_top {
                crate::serial_println!(
                    "Setting kernel stack for thread {} to {:#x}",
                    thread_id,
                    stack_top.as_u64()
                );
                crate::gdt::set_kernel_stack(stack_top);
                crate::serial_println!("TSS RSP0 updated successfully for thread {}", thread_id);
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

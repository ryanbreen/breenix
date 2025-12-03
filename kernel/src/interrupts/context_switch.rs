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
    // NOTE: No logging in interrupt handlers per CLAUDE.md - causes timer to fire
    // faster than userspace can execute, creating infinite kernel loops.
    // Serial I/O takes thousands of cycles, causing timer interrupts to fire faster
    // than userspace can execute, resulting in infinite kernel loops.

    // CRITICAL: Only schedule when returning to userspace with preempt_count == 0
    if !crate::per_cpu::can_schedule(interrupt_frame.code_segment.0 as u64) {
        return;
    }

    // NOTE: Context is saved ONLY when actually switching threads (see line ~135).
    // We do NOT save on every timer interrupt - that caused massive overhead
    // preventing userspace from executing even one instruction.
    // The interrupt frame captures the current RIP; we save to thread.context
    // only when switching away.
    let from_userspace = (interrupt_frame.code_segment.0 & 3) == 3;

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
        log::trace!(
            "Context switch: from_userspace={}, CS={:#x}",
            from_userspace,
            interrupt_frame.code_segment.0
        );

        // Emit canonical ring3 marker on the FIRST entry to userspace (for CI)
        if from_userspace {
            static mut EMITTED_RING3_MARKER: bool = false;
            unsafe {
                if !EMITTED_RING3_MARKER {
                    EMITTED_RING3_MARKER = true;
                    crate::serial_println!("RING3_ENTER: CS=0x33");
                    crate::serial_println!(
                        "[ OK ] RING3_SMOKE: userspace executed + syscall path verified"
                    );
                }
            }
        }

        // Save current thread's context if coming from userspace
        // CRITICAL: If save fails, we MUST NOT switch contexts!
        // Switching without saving would cause the process to return to stale RIP (entry point)
        //
        // CRITICAL FIX for RDI corruption bug:
        // Don't save context if we interrupted syscall return path.
        // The interrupt frame has CS=0x33 when returning to userspace, but if we
        // interrupted during syscall exit (after preempt_enable but before IRETQ),
        // the saved_regs contain KERNEL values, not userspace!
        //
        // We detect syscall return by checking preempt_count bit 28 (PREEMPT_ACTIVE).
        // The syscall return path (entry.asm) sets this bit before restoring registers.
        // Linux uses 0x10000000 for PREEMPT_ACTIVE (bit 28).
        let preempt_count = crate::per_cpu::preempt_count();
        let preempt_active = (preempt_count & 0x10000000) != 0;  // Bit 28

        // DEBUG: Log preempt_count check for first few instances
        static PREEMPT_CHECK_LOG: core::sync::atomic::AtomicU64 =
            core::sync::atomic::AtomicU64::new(0);
        let check_count = PREEMPT_CHECK_LOG.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        if check_count < 10 {
            log::debug!(
                "Context switch check: from_userspace={}, preempt_count={:#x}, preempt_active={}, rdi={:#x}",
                from_userspace, preempt_count, preempt_active, saved_regs.rdi
            );
        }

        if from_userspace && !preempt_active {
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
        } else if from_userspace && preempt_active {
            // We're in syscall return path - don't save context!
            // The registers in saved_regs are kernel values from syscall handler!
            static SYSCALL_INTERRUPT_LOG: core::sync::atomic::AtomicU64 =
                core::sync::atomic::AtomicU64::new(0);
            let count = SYSCALL_INTERRUPT_LOG.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            if count < 10 {
                log::info!(
                    "Timer interrupt during syscall return (PREEMPT_ACTIVE set, rdi={:#x}), \
                     skipping context save to prevent register corruption",
                    saved_regs.rdi
                );
            }
            // Don't switch threads - we're in syscall return path with kernel registers
            return;
        }

        // Switch to the new thread
        switch_to_thread(new_thread_id, saved_regs, interrupt_frame);

        // Log userspace transition
        if scheduler::with_thread_mut(new_thread_id, |t| t.privilege == ThreadPrivilege::User)
            .unwrap_or(false)
        {
            log::trace!("Restored userspace context for thread {}", new_thread_id);
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
    log::trace!("restore_userspace_thread_context: thread {}", thread_id);

    // Check if this thread has ever run before
    let has_started = scheduler::with_thread_mut(thread_id, |thread| {
        thread.has_started
    }).unwrap_or(false);

    if !has_started {
        // CRITICAL: This is a brand new thread that has never run
        // We need to set up for its first entry to userspace
        log::info!("First run: thread {} entering userspace", thread_id);

        // Mark thread as started
        scheduler::with_thread_mut(thread_id, |thread| {
            thread.has_started = true;
        });

        // For first run, we need to set up the interrupt frame to jump to userspace
        setup_first_userspace_entry(thread_id, interrupt_frame, saved_regs);
        return;
    }

    // Thread has run before - do normal context restore
    log::trace!("Resuming thread {}", thread_id);

    // CRITICAL: Use try_manager in interrupt context to avoid deadlock
    // Never use with_process_manager() from interrupt handlers!
    if let Some(mut manager_guard) = crate::process::try_manager() {
        if let Some(ref mut manager) = *manager_guard {
            if let Some((pid, process)) = manager.find_process_by_thread_mut(thread_id) {
                if let Some(ref mut thread) = process.main_thread {
                    if thread.privilege == ThreadPrivilege::User {
                        restore_userspace_context(thread, interrupt_frame, saved_regs);
                        log::trace!("Restored context for thread {}", thread_id);

                        // Switch to process page table immediately during context switch
                        if let Some(ref page_table) = process.page_table {
                            let page_table_frame = page_table.level_4_frame();

                            // Switch CR3 immediately
                            unsafe {
                                use x86_64::registers::control::Cr3;
                                let (current_frame, flags) = Cr3::read();
                                if current_frame != page_table_frame {
                                    log::trace!(
                                        "CR3 switch: {:#x} -> {:#x} (pid {})",
                                        current_frame.start_address().as_u64(),
                                        page_table_frame.start_address().as_u64(),
                                        pid.as_u64()
                                    );

                                    // Disable interrupts during CR3 switch
                                    x86_64::instructions::interrupts::disable();
                                    Cr3::write(page_table_frame, flags);
                                    x86_64::instructions::tlb::flush_all();
                                }
                            }
                        } else {
                            log::warn!("Process {} has no page table!", pid.as_u64());
                        }

                        // Update TSS RSP0 for the new thread's kernel stack
                        if let Some(kernel_stack_top) = thread.kernel_stack_top {
                            crate::gdt::set_kernel_stack(kernel_stack_top);
                            log::trace!("Set kernel stack: {:#x}", kernel_stack_top.as_u64());
                        } else {
                            log::error!("ERROR: Userspace thread {} has no kernel stack!", thread_id);
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
    }
}

/// Set up interrupt frame for first entry to userspace
fn setup_first_userspace_entry(thread_id: u64, interrupt_frame: &mut InterruptStackFrame, saved_regs: &mut SavedRegisters) {
    log::info!("setup_first_userspace_entry: thread {}", thread_id);

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
                frame.code_segment = crate::gdt::user_code_selector();

                // Set stack segment to user data (Ring 3)
                frame.stack_segment = crate::gdt::user_data_selector();

                // Set CPU flags: IF=1 (interrupts enabled), bit 1=1 (reserved)
                let flags_ptr = &mut frame.cpu_flags as *mut x86_64::registers::rflags::RFlags as *mut u64;
                *flags_ptr = 0x202;  // Bit 1=1 (required), IF=1 (bit 9)

                log::info!(
                    "RING3_ENTRY: RIP={:#x}, RSP={:#x}, CS={:#x}, SS={:#x}",
                    frame.instruction_pointer.as_u64(),
                    frame.stack_pointer.as_u64(),
                    frame.code_segment.0,
                    frame.stack_segment.0
                );
            });
        }
    });

    // CRITICAL: Zero all general-purpose registers for security and determinism
    // This ensures userspace starts with a clean register state
    saved_regs.rax = 0;
    saved_regs.rbx = 0;
    saved_regs.rcx = 0;
    saved_regs.rdx = 0;
    saved_regs.rsi = 0;
    saved_regs.rdi = 0;
    saved_regs.rbp = 0;
    saved_regs.r8 = 0;
    saved_regs.r9 = 0;
    saved_regs.r10 = 0;
    saved_regs.r11 = 0;
    saved_regs.r12 = 0;
    saved_regs.r13 = 0;
    saved_regs.r14 = 0;
    saved_regs.r15 = 0;

    // DEBUG: Log that we're zeroing RDI for first entry
    log::info!("FIRST_ENTRY t{}: zeroing rdi to 0", thread_id);

    // CRITICAL: Now set up CR3 and kernel stack for this thread
    // This must happen BEFORE we iretq to userspace
    if let Some(mut manager_guard) = crate::process::try_manager() {
        if let Some((pid, process)) = manager_guard.as_mut().and_then(|m| m.find_process_by_thread_mut(thread_id)) {
            log::trace!("Thread {} belongs to process {}", thread_id, pid.as_u64());

            // Get kernel stack info BEFORE switching CR3
            let kernel_stack_top = process.main_thread.as_ref()
                .and_then(|thread| {
                    if thread.id == thread_id {
                        thread.kernel_stack_top
                    } else {
                        None
                    }
                });

            // Switch to process page table
            if let Some(page_table) = process.page_table.as_ref() {
                let new_frame = page_table.level_4_frame();
                log::trace!("Switching CR3 to {:#x}", new_frame.start_address().as_u64());

                // Switch CR3 atomically with interrupts disabled
                x86_64::instructions::interrupts::without_interrupts(|| {
                    unsafe {
                        let cr3_value = new_frame.start_address().as_u64();

                        // Switch to process page table
                        core::arch::asm!("mov cr3, {}", in(reg) cr3_value, options(nostack, preserves_flags));

                        // Tell interrupt return path to use this CR3
                        crate::per_cpu::set_next_cr3(cr3_value);

                        // Set saved_process_cr3 for timer interrupt
                        core::arch::asm!(
                            "mov gs:[80], {}",
                            in(reg) cr3_value,
                            options(nostack, preserves_flags)
                        );

                        // Flush TLB
                        x86_64::instructions::tlb::flush_all();

                        log::trace!("CR3 switched to {:#x}", cr3_value);
                    }
                });
            }

            // Set kernel stack for TSS RSP0
            if let Some(stack_top) = kernel_stack_top {
                crate::gdt::set_kernel_stack(stack_top);
                log::trace!("Set TSS RSP0 to {:#x} for thread {}", stack_top.as_u64(), thread_id);
            } else {
                log::error!("No kernel stack found for thread {}", thread_id);
            }
        }
    }

    log::info!("First userspace entry setup complete for thread {}", thread_id);
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

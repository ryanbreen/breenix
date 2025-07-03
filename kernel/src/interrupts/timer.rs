//! Timer interrupt handler with userspace preemption support

use x86_64::structures::idt::InterruptStackFrame;
use crate::task::process_context::{SavedRegisters, save_userspace_context, restore_userspace_context};
use crate::task::scheduler;
use crate::task::thread::ThreadPrivilege;

/// Timer interrupt handler called from assembly
/// 
/// This is called with:
/// - saved_regs: pointer to saved general purpose registers (mutable)
/// - interrupt_frame: pointer to the interrupt stack frame (mutable)
#[no_mangle]
pub extern "C" fn timer_interrupt_rust_handler(
    saved_regs: &mut SavedRegisters,
    interrupt_frame: &mut InterruptStackFrame,
) {
    // Update timer tick count
    crate::time::timer_interrupt();
    
    // Check if we came from userspace
    let from_userspace = (interrupt_frame.code_segment.0 & 3) == 3;
    
    // Check if we have a current thread and if it's terminated
    let current_thread_id = scheduler::current_thread_id();
    let current_terminated = if let Some(id) = current_thread_id {
        scheduler::with_scheduler(|sched| {
            sched.get_thread(id).map(|t| t.state == crate::task::thread::ThreadState::Terminated).unwrap_or(false)
        }).unwrap_or(false)
    } else {
        // No current thread means the previous one terminated and was cleaned up
        true
    };
    
    if current_terminated && from_userspace {
        log::debug!("Current userspace thread is terminated or cleaned up, need to switch");
    }
    
    // Perform scheduling
    if let Some((old_id, new_id)) = scheduler::schedule() {
        if old_id != new_id {
            log::info!("Timer preemption: {} -> {} (from_userspace: {})", 
                       old_id, new_id, from_userspace);
            
            // Handle context switching
            handle_context_switch(old_id, new_id, from_userspace, saved_regs, interrupt_frame);
        }
    } else if current_terminated && from_userspace {
        // No threads to switch to, but we need to get out of userspace
        // Switch to idle thread
        log::debug!("No runnable threads, switching to idle from userspace");
        
        // Get the idle thread ID from scheduler
        let idle_id = scheduler::with_scheduler(|sched| sched.idle_thread()).unwrap_or(0);
        
        // If we have a current thread ID, do proper cleanup; otherwise just switch to idle
        if let Some(current_id) = current_thread_id {
            handle_context_switch(current_id, idle_id, from_userspace, saved_regs, interrupt_frame);
        } else {
            // No current thread, just set up to run idle thread
            handle_idle_transition(idle_id, interrupt_frame);
        }
    }
    
    // Send End Of Interrupt
    unsafe {
        super::PICS.lock()
            .notify_end_of_interrupt(super::InterruptIndex::Timer.as_u8());
    }
}

/// Handle context switching between threads
fn handle_context_switch(
    old_id: u64,
    new_id: u64,
    from_userspace: bool,
    saved_regs: &mut SavedRegisters,
    interrupt_frame: &mut InterruptStackFrame,
) {
    // We need to save the old thread's context and restore the new thread's context
    // This is tricky because we need to access scheduler internals
    
    // First, switch TLS
    if let Err(e) = crate::tls::switch_tls(new_id) {
        log::error!("Failed to switch TLS: {}", e);
        return;
    }
    
    // If we're coming from userspace, we need to save the context
    if from_userspace {
        // Find the process by thread ID and save its context
        if let Some(ref mut manager) = *crate::process::manager() {
            if let Some((pid, process)) = manager.find_process_by_thread_mut(old_id) {
                if let Some(ref mut thread) = process.main_thread {
                    save_userspace_context(thread, interrupt_frame, saved_regs);
                    log::trace!("Saved context for process {} (thread {})", pid.as_u64(), old_id);
                }
            }
        }
    }
    
    // Now check if we need to restore a userspace context for the new thread
    // First check if this is the idle thread
    let is_idle_thread = scheduler::with_scheduler(|sched| new_id == sched.idle_thread()).unwrap_or(false);
    
    if is_idle_thread {
        // Switching to idle thread - ensure we return to kernel mode
        log::debug!("Switching to idle thread {}, setting up kernel mode return", new_id);
        handle_idle_transition(new_id, interrupt_frame);
    } else if let Some(ref manager) = *crate::process::manager() {
        if let Some((pid, process)) = manager.find_process_by_thread(new_id) {
            if let Some(ref thread) = process.main_thread {
                if thread.privilege == ThreadPrivilege::User {
                    // Check if this is the first time running based on thread state
                    let is_first_run = thread.state == crate::task::thread::ThreadState::Ready;
                    log::info!("Thread {} is_first_run: {}, state: {:?}", new_id, is_first_run, thread.state);
                    
                    if is_first_run {
                        // First time running this thread, set up for initial userspace entry
                        log::info!("Setting up initial userspace entry for thread {}", new_id);
                        
                        // Disable interrupts during critical userspace setup
                        x86_64::instructions::interrupts::without_interrupts(|| {
                            setup_initial_userspace_entry(thread, interrupt_frame);
                        });
                        
                        log::info!("Initial userspace entry for process {} (thread {})", pid.as_u64(), new_id);
                    } else {
                        // This thread has run before, restore its saved context
                        log::info!("Restoring saved context for thread {}", new_id);
                        restore_userspace_context(thread, interrupt_frame, saved_regs);
                        log::trace!("Restored context for process {} (thread {})", pid.as_u64(), new_id);
                    }
                    
                    // Update TSS RSP0 for the new thread's kernel stack
                    crate::gdt::set_kernel_stack(thread.stack_top);
                }
            }
        }
    }
}

/// Set up interrupt frame for initial userspace entry
fn setup_initial_userspace_entry(
    thread: &crate::task::thread::Thread,
    interrupt_frame: &mut InterruptStackFrame,
) {
    unsafe {
        interrupt_frame.as_mut().update(|frame| {
            // For userspace threads, the entry point is stored in thread.context.rip
            let entry = thread.context.rip;
            log::info!("Setting up userspace entry: RIP={:#x}, RSP={:#x}", entry, thread.context.rsp);
            
            frame.instruction_pointer = x86_64::VirtAddr::new(entry);
            frame.stack_pointer = x86_64::VirtAddr::new(thread.context.rsp);
            frame.code_segment = x86_64::structures::gdt::SegmentSelector(
                crate::gdt::USER_CODE_SELECTOR.0 | 3
            );
            frame.stack_segment = x86_64::structures::gdt::SegmentSelector(
                crate::gdt::USER_DATA_SELECTOR.0 | 3
            );
            frame.cpu_flags = x86_64::registers::rflags::RFlags::INTERRUPT_FLAG;
            
            log::info!("Userspace segments: CS={:#x}, SS={:#x}", 
                      frame.code_segment.0, frame.stack_segment.0);
        });
    }
}

/// Handle transition to idle thread when no current thread exists
fn handle_idle_transition(
    idle_id: u64,
    interrupt_frame: &mut InterruptStackFrame,
) {
    // Switch TLS to idle thread
    if let Err(e) = crate::tls::switch_tls(idle_id) {
        log::error!("Failed to switch TLS to idle thread: {}", e);
        return;
    }
    
    // The idle thread should just halt and wait for interrupts
    // We'll set up the interrupt frame to return to a safe idle loop
    unsafe {
        interrupt_frame.as_mut().update(|frame| {
            // Set kernel code/data segments
            frame.code_segment = crate::gdt::kernel_code_selector();
            frame.stack_segment = crate::gdt::kernel_data_selector();
            
            // Set RIP to the idle function
            frame.instruction_pointer = x86_64::VirtAddr::new(idle_loop as *const () as u64);
            
            // Use the current kernel stack (it should be safe)
            // frame.stack_pointer is already set correctly for kernel mode
            
            frame.cpu_flags = x86_64::registers::rflags::RFlags::INTERRUPT_FLAG;
        });
    }
    
    log::debug!("Transitioned to idle thread {}", idle_id);
}

/// Simple idle loop that halts and waits for interrupts
fn idle_loop() -> ! {
    loop {
        // Halt and wait for next interrupt
        x86_64::instructions::hlt();
    }
}
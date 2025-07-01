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
    
    // Perform scheduling
    if let Some((old_id, new_id)) = scheduler::schedule() {
        if old_id != new_id {
            log::info!("Timer preemption: {} -> {} (from_userspace: {})", 
                       old_id, new_id, from_userspace);
            
            // Handle context switching
            handle_context_switch(old_id, new_id, from_userspace, saved_regs, interrupt_frame);
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
    if let Some(ref manager) = *crate::process::manager() {
        if let Some((pid, process)) = manager.find_process_by_thread(new_id) {
            if let Some(ref thread) = process.main_thread {
                if thread.privilege == ThreadPrivilege::User {
                    // Check if this is the first time running or a resume based on thread state
                    let is_first_run = thread.state == crate::task::thread::ThreadState::Ready;
                    log::info!("Thread {} is_first_run: {}, state: {:?}", new_id, is_first_run, thread.state);
                    
                    if is_first_run {
                        // First time running this thread, set up for initial userspace entry
                        log::info!("Setting up initial userspace entry for thread {}", new_id);
                        setup_initial_userspace_entry(thread, interrupt_frame);
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
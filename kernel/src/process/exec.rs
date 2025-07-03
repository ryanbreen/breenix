//! Process execution support
//!
//! This module implements the exec() family of functions for starting processes.
//! Unlike the current broken approach of starting processes from timer interrupts,
//! this provides a proper mechanism for transitioning to userspace.

use crate::process::{ProcessId, manager};
use crate::task::userspace_switch::switch_to_userspace;
use crate::gdt::{USER_CODE_SELECTOR, USER_DATA_SELECTOR};
use alloc::string::String;

/// Execute a process immediately, transitioning to userspace
/// 
/// This function creates a process and immediately switches to it,
/// avoiding the broken timer interrupt approach.
/// 
/// # Safety
/// This function never returns - it switches to userspace
pub unsafe fn exec_process(name: String, elf_data: &[u8]) -> Result<!, &'static str> {
    log::info!("exec_process: Starting process {} directly", name);
    
    // Create the process
    let (pid, entry_point, stack_top) = {
        let mut manager_guard = manager();
        let manager = manager_guard.as_mut()
            .ok_or("Process manager not initialized")?;
        
        // Create the process
        let pid = manager.create_process(name, elf_data)?;
        
        // Get the process to extract entry point and stack
        let process = manager.get_process(pid)
            .ok_or("Failed to find just-created process")?;
        
        // Get entry point and stack from main thread
        let thread = process.main_thread.as_ref()
            .ok_or("Process has no main thread")?;
        
        let entry_point = thread.context.rip;
        let stack_top = thread.context.rsp;
        
        // Update TLS for this thread
        crate::tls::switch_tls(thread.id)
            .map_err(|_| "Failed to set up TLS")?;
        
        // Update TSS RSP0 for syscalls
        crate::gdt::set_kernel_stack(thread.stack_top);
        
        log::info!("Process {} created: entry={:#x}, stack={:#x}", 
                  pid.as_u64(), entry_point, stack_top);
        
        (pid, entry_point, stack_top)
    };
    // Manager lock dropped here
    
    // Set up segments for Ring 3
    let user_cs = USER_CODE_SELECTOR.0 | 3;  // Set RPL=3
    let user_ds = USER_DATA_SELECTOR.0 | 3;  // Set RPL=3
    
    log::info!("Switching to userspace for process {} (PID {})", 
              pid.as_u64(), pid.as_u64());
    log::info!("Entry point: {:#x}, Stack: {:#x}", entry_point, stack_top);
    log::info!("Segments: CS={:#x}, DS/SS={:#x}", user_cs, user_ds);
    
    // Clear all general purpose registers for security
    core::arch::asm!(
        "xor rax, rax",
        "xor rbx, rbx", 
        "xor rcx, rcx",
        "xor rdx, rdx",
        "xor rsi, rsi",
        "xor rdi, rdi",
        "xor rbp, rbp",
        "xor r8, r8",
        "xor r9, r9",
        "xor r10, r10",
        "xor r11, r11",
        "xor r12, r12",
        "xor r13, r13",
        "xor r14, r14",
        "xor r15, r15",
        options(nomem, nostack)
    );
    
    // Switch to userspace - this never returns
    switch_to_userspace(
        x86_64::VirtAddr::new(entry_point),
        x86_64::VirtAddr::new(stack_top),
        user_cs as u16,
        user_ds as u16,
    )
}

/// Start the init process (PID 1)
/// 
/// This should be called during kernel initialization to start
/// the first userspace process. It avoids the timer interrupt
/// mechanism entirely.
pub unsafe fn start_init_process(elf_data: &[u8]) -> Result<!, &'static str> {
    log::info!("Starting init process (PID 1)");
    exec_process(String::from("init"), elf_data)
}
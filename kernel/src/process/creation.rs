//! Proper process creation with user threads from start
//!
//! This module implements the new process creation model that follows Unix semantics:
//! - Processes are created as user threads from the beginning
//! - No kernel-to-user transitions via spawn threads
//! - Direct creation of user threads in Ring 3 mode
//! - Proper integration with the new minimal timer interrupt system

use crate::process::ProcessId;
use alloc::string::String;
use alloc::boxed::Box;

/// Create a new user process directly without spawn mechanism
/// 
/// This creates a process that starts as a userspace thread from the beginning,
/// following proper Unix semantics. The process is ready to be scheduled and
/// will start executing in Ring 3 userspace.
/// 
/// This is a thin wrapper around the existing process creation that ensures
/// the process starts as a user thread without spawn thread transitions.
pub fn create_user_process(name: String, elf_data: &[u8]) -> Result<ProcessId, &'static str> {
    log::info!("create_user_process: Creating user process '{}' with new model", name);
    
    // Create the process using existing infrastructure
    // CRITICAL: Disable interrupts during process creation to prevent
    // context switches that could leave the process in an inconsistent state
    let pid = x86_64::instructions::interrupts::without_interrupts(|| {
        let mut manager_guard = crate::process::manager();
        if let Some(ref mut manager) = *manager_guard {
            manager.create_process(name.clone(), elf_data)
        } else {
            Err("Process manager not available")
        }
    })?;
    
    // The key difference: Add the thread directly to scheduler as a user thread
    // without going through the spawn mechanism
    // Note: spawn() already has its own interrupt protection, so we don't need
    // to wrap this part
    {
        let manager_guard = crate::process::manager();
        if let Some(ref manager) = *manager_guard {
            if let Some(process) = manager.get_process(pid) {
                if let Some(ref main_thread) = process.main_thread {
                    // Verify it's a user thread
                    if main_thread.privilege == crate::task::thread::ThreadPrivilege::User {
                        // Add directly to scheduler - no spawn thread needed!
                        // Note: spawn() internally uses without_interrupts
                        crate::task::scheduler::spawn(Box::new(main_thread.clone()));
                        log::info!("create_user_process: Added user thread {} directly to scheduler", 
                                   main_thread.id);
                    } else {
                        log::error!("create_user_process: Thread {} is not a user thread!", main_thread.id);
                        return Err("Created thread is not a user thread");
                    }
                } else {
                    return Err("Process has no main thread");
                }
            } else {
                return Err("Failed to find created process");
            }
        } else {
            return Err("Process manager not available");
        }
    }
    
    log::info!("create_user_process: Successfully created user process {} without spawn mechanism", 
               pid.as_u64());
    
    Ok(pid)
}

/// Initialize the first user process (init)
/// 
/// This creates PID 1 as a proper user process without spawn mechanisms.
#[allow(dead_code)]
pub fn init_user_process(elf_data: &[u8]) -> Result<ProcessId, &'static str> {
    log::info!("init_user_process: Creating init process (PID 1)");
    
    let pid = create_user_process(String::from("init"), elf_data)?;
    
    log::info!("init_user_process: Successfully created init process with PID {}", pid.as_u64());
    
    Ok(pid)
}
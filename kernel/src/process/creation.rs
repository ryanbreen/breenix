//! Proper process creation with user threads from start
//!
//! This module implements the new process creation model that follows Unix semantics:
//! - Processes are created as user threads from the beginning
//! - No kernel-to-user transitions via spawn threads
//! - Direct creation of user threads in Ring 3 mode
//! - Proper integration with the new minimal timer interrupt system

use crate::process::ProcessId;
use alloc::boxed::Box;
use alloc::string::String;

/// Create a new user process directly without spawn mechanism
///
/// This creates a process that starts as a userspace thread from the beginning,
/// following proper Unix semantics. The process is ready to be scheduled and
/// will start executing in Ring 3 userspace.
///
/// This is a thin wrapper around the existing process creation that ensures
/// the process starts as a user thread without spawn thread transitions.
pub fn create_user_process(name: String, elf_data: &[u8]) -> Result<ProcessId, &'static str> {
    log::info!(
        "create_user_process: Creating user process '{}' with new model",
        name
    );
    crate::serial_println!("create_user_process: ENTRY - Creating '{}'", name);

    // Create the process using existing infrastructure
    // CRITICAL: Disable interrupts during process creation to prevent
    // context switches that could leave the process in an inconsistent state
    crate::serial_println!("create_user_process: About to disable interrupts and call manager.create_process");
    let pid = x86_64::instructions::interrupts::without_interrupts(|| {
        crate::serial_println!("create_user_process: Interrupts disabled, acquiring process manager lock");
        let mut manager_guard = crate::process::manager();
        crate::serial_println!("create_user_process: Got process manager lock");
        if let Some(ref mut manager) = *manager_guard {
            crate::serial_println!("create_user_process: Calling manager.create_process");
            let result = manager.create_process(name.clone(), elf_data);
            crate::serial_println!("create_user_process: manager.create_process returned: {:?}", result.is_ok());
            result
        } else {
            crate::serial_println!("create_user_process: Process manager not available!");
            Err("Process manager not available")
        }
    })?;
    crate::serial_println!("create_user_process: Process created with PID {}", pid.as_u64());

    // The key difference: Add the thread directly to scheduler as a user thread
    // without going through the spawn mechanism
    // Note: spawn() already has its own interrupt protection, so we don't need
    // to wrap this part
    crate::serial_println!("create_user_process: About to add thread to scheduler");
    {
        crate::serial_println!("create_user_process: Acquiring process manager for thread scheduling");
        let manager_guard = crate::process::manager();
        crate::serial_println!("create_user_process: Got process manager lock for scheduling");
        if let Some(ref manager) = *manager_guard {
            if let Some(process) = manager.get_process(pid) {
                if let Some(ref main_thread) = process.main_thread {
                    // Verify it's a user thread
                    if main_thread.privilege == crate::task::thread::ThreadPrivilege::User {
                        log::info!(
                            "create_user_process: Scheduling user thread {} ('{}')",
                            main_thread.id,
                            main_thread.name
                        );
                        crate::serial_println!("create_user_process: Calling scheduler::spawn for thread {}", main_thread.id);
                        // Add directly to scheduler - no spawn thread needed!
                        // Note: spawn() internally uses without_interrupts
                        crate::task::scheduler::spawn(Box::new(main_thread.clone()));
                        crate::serial_println!("create_user_process: scheduler::spawn completed");

                        // REMOVED: set_next_cr3() call - CR3 switching happens during scheduling,
                        // not during process creation. The context_switch.rs::setup_first_userspace_entry()
                        // function handles CR3 switching when the thread is actually scheduled to run.

                        log::info!(
                            "create_user_process: User thread {} enqueued for scheduling",
                            main_thread.id
                        );
                    } else {
                        log::error!(
                            "create_user_process: Thread {} is not a user thread!",
                            main_thread.id
                        );
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

    log::info!(
        "create_user_process: Successfully created user process {} without spawn mechanism",
        pid.as_u64()
    );
    crate::serial_println!("create_user_process: COMPLETE - returning PID {}", pid.as_u64());

    Ok(pid)
}

/// Initialize the first user process (init)
///
/// This creates PID 1 as a proper user process without spawn mechanisms.
#[allow(dead_code)]
pub fn init_user_process(elf_data: &[u8]) -> Result<ProcessId, &'static str> {
    log::info!("init_user_process: Creating init process (PID 1)");

    let pid = create_user_process(String::from("init"), elf_data)?;

    log::info!(
        "init_user_process: Successfully created init process with PID {}",
        pid.as_u64()
    );

    Ok(pid)
}

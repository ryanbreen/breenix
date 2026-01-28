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
/// Note: Uses x86_64-specific ELF loader and process creation
#[cfg(target_arch = "x86_64")]
pub fn create_user_process(name: String, elf_data: &[u8]) -> Result<ProcessId, &'static str> {
    log::info!(
        "create_user_process: Creating user process '{}' with new model",
        name
    );
    crate::serial_println!("create_user_process: ENTRY - Creating '{}'", name);

    // Create the process using existing infrastructure
    // NOTE: We do NOT disable interrupts here. Process creation requires frame
    // allocation which needs the MEMORY_INFO lock. If we disable interrupts while
    // holding PROCESS_MANAGER and then try to acquire MEMORY_INFO, we can deadlock
    // with other threads doing the same. The process manager lock itself provides
    // mutual exclusion - interrupt protection is not needed.
    crate::serial_println!("create_user_process: Acquiring process manager lock");
    let pid = {
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
    }?;
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

                        // Set this process as the foreground process group for the console TTY
                        // This ensures Ctrl+C (SIGINT) and other TTY signals go to this process
                        // Note: TTY is only available on x86_64 currently
                        #[cfg(target_arch = "x86_64")]
                        if let Some(tty) = crate::tty::console() {
                            tty.set_foreground_pgrp(pid.as_u64());
                            log::debug!(
                                "create_user_process: Set PID {} as foreground pgrp for TTY",
                                pid.as_u64()
                            );
                        }

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

/// Create a new user process directly without spawn mechanism (ARM64 version)
///
/// This creates a process that starts as a userspace thread from the beginning,
/// following proper Unix semantics. The process is ready to be scheduled and
/// will start executing in EL0 userspace.
#[cfg(target_arch = "aarch64")]
pub fn create_user_process(name: String, elf_data: &[u8]) -> Result<ProcessId, &'static str> {
    log::info!(
        "create_user_process: Creating user process '{}' (ARM64)",
        name
    );
    crate::serial_println!("create_user_process: ENTRY - Creating '{}' (ARM64)", name);

    // Create the process using existing infrastructure
    crate::serial_println!("create_user_process: Acquiring process manager lock");
    let pid = {
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
    }?;
    crate::serial_println!("create_user_process: Process created with PID {}", pid.as_u64());

    // Add the thread directly to scheduler as a user thread
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
                        crate::task::scheduler::spawn(Box::new(main_thread.clone()));
                        crate::serial_println!("create_user_process: scheduler::spawn completed");

                        // Set this process as the foreground process group for the console TTY
                        // This ensures Ctrl+C (SIGINT) and other TTY signals go to this process
                        if let Some(tty) = crate::tty::console() {
                            tty.set_foreground_pgrp(pid.as_u64());
                            log::debug!(
                                "create_user_process: Set PID {} as foreground pgrp for TTY (ARM64)",
                                pid.as_u64()
                            );
                        }

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
        "create_user_process: Successfully created user process {} (ARM64)",
        pid.as_u64()
    );
    crate::serial_println!("create_user_process: COMPLETE - returning PID {}", pid.as_u64());

    Ok(pid)
}

/// Initialize the first user process (init)
///
/// This creates PID 1 as a proper user process without spawn mechanisms.
/// Note: Uses x86_64-specific process creation
#[cfg(target_arch = "x86_64")]
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

/// Initialize the first user process (init) - ARM64 version
#[cfg(target_arch = "aarch64")]
#[allow(dead_code)]
pub fn init_user_process(elf_data: &[u8]) -> Result<ProcessId, &'static str> {
    log::info!("init_user_process: Creating init process (PID 1) (ARM64)");

    let pid = create_user_process(String::from("init"), elf_data)?;

    log::info!(
        "init_user_process: Successfully created init process with PID {} (ARM64)",
        pid.as_u64()
    );

    Ok(pid)
}

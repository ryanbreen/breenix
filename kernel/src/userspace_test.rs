//! Userspace program testing module


/// Include the compiled userspace test binaries
#[cfg(feature = "testing")]
pub static HELLO_TIME_ELF: &[u8] = include_bytes!("../../userspace/tests/hello_time.elf");

#[cfg(feature = "testing")]
pub static HELLO_WORLD_ELF: &[u8] = include_bytes!("../../userspace/tests/hello_world.elf");

#[cfg(feature = "testing")]
pub static COUNTER_ELF: &[u8] = include_bytes!("../../userspace/tests/counter.elf");

#[cfg(feature = "testing")]
pub static SPINNER_ELF: &[u8] = include_bytes!("../../userspace/tests/spinner.elf");

#[cfg(feature = "testing")]
pub static FORK_TEST_ELF: &[u8] = include_bytes!("../../userspace/tests/fork_test.elf");

#[cfg(feature = "testing")]
pub static SPAWN_TEST_ELF: &[u8] = include_bytes!("../../userspace/tests/spawn_test.elf");

// Add test to ensure binaries are included
#[cfg(feature = "testing")]
fn _test_binaries_included() {
    assert!(HELLO_TIME_ELF.len() > 0, "hello_time.elf not included");
    assert!(HELLO_WORLD_ELF.len() > 0, "hello_world.elf not included");
    assert!(COUNTER_ELF.len() > 0, "counter.elf not included");
    assert!(SPINNER_ELF.len() > 0, "spinner.elf not included");
    assert!(FORK_TEST_ELF.len() > 0, "fork_test.elf not included");
    assert!(SPAWN_TEST_ELF.len() > 0, "spawn_test.elf not included");
}


/// Run userspace test - callable from keyboard handler
pub fn run_userspace_test() {
    log::info!("=== Running Userspace Test Program ===");
    
    // Check if we have the test binary
    #[cfg(feature = "testing")]
    {
        use alloc::string::String;
        
        log::info!("Creating userspace test process ({} bytes)", HELLO_TIME_ELF.len());
        log::info!("ELF entry point from header: 0x{:x}", {
            use crate::elf::Elf64Header;
            let header: &Elf64Header = unsafe { 
                &*(HELLO_TIME_ELF.as_ptr() as *const Elf64Header) 
            };
            header.entry
        });
        
        // Create and schedule a process for the test program
        match crate::process::creation::create_user_process(
            String::from("hello_time"), 
            HELLO_TIME_ELF
        ) {
            Ok(pid) => {
                log::info!("✓ Created and scheduled process with PID {}", pid.as_u64());
                
                // Get the process manager and debug print
                if let Some(ref manager) = *crate::process::manager() {
                    manager.debug_processes();
                }
                
                log::info!("Process scheduled - it will run when scheduler picks it up");
                log::info!("Timer interrupts should trigger scheduling");
                
                // Force a yield to try to switch to the process
                crate::task::scheduler::yield_current();
                log::info!("Yielded to scheduler");
            }
            Err(e) => {
                log::error!("✗ Failed to create process: {}", e);
            }
        }
    }
    
    #[cfg(not(feature = "testing"))]
    {
        log::warn!("Userspace test binary not available - compile with --features testing");
    }
}

/// Test multiple processes - callable from keyboard handler
pub fn test_multiple_processes() {
    log::info!("=== Testing Multiple Processes ===");
    
    #[cfg(feature = "testing")]
    {
        use alloc::string::String;
        
        // Create and schedule first process (counter)
        log::info!("Creating first process (counter)...");
        match crate::process::creation::create_user_process(
            String::from("counter"), 
            COUNTER_ELF
        ) {
            Ok(pid1) => {
                log::info!("✓ Created and scheduled process 1 (counter) with PID {}", pid1.as_u64());
                
                // Create and schedule second process (spinner)
                log::info!("Creating second process (spinner)...");
                match crate::process::creation::create_user_process(
                    String::from("spinner"), 
                    SPINNER_ELF
                ) {
                    Ok(pid2) => {
                        log::info!("✓ Created and scheduled process 2 (spinner) with PID {}", pid2.as_u64());
                        
                        // Debug print process list
                        if let Some(ref manager) = *crate::process::manager() {
                            manager.debug_processes();
                        }
                        
                        log::info!("Both processes scheduled - they will run concurrently");
                        log::info!("Processes will alternate execution based on timer interrupts");
                        log::info!("Counter will count from 0-9, Spinner will show a spinning animation");
                        log::info!("Each process yields after each output to allow the other to run");
                    }
                    Err(e) => {
                        log::error!("✗ Failed to create second process: {}", e);
                    }
                }
            }
            Err(e) => {
                log::error!("✗ Failed to create first process: {}", e);
            }
        }
    }
    
    #[cfg(not(feature = "testing"))]
    {
        log::warn!("Userspace test binaries not available - compile with --features testing");
    }
}

/// Test fork system call implementation (debug version)
#[cfg(feature = "testing")]
pub fn test_fork_debug() {
    log::info!("=== Testing Fork System Call (Debug Mode) ===");
    
    use alloc::string::String;
    
    log::info!("Creating process that will call fork() to debug thread ID tracking...");
    
    // Use the new spawn mechanism which creates a dedicated thread for exec
    match crate::process::creation::create_user_process(
        String::from("fork_debug"), 
        FORK_TEST_ELF
    ) {
        Ok(pid) => {
            log::info!("✓ Created and scheduled fork debug process with PID {}", pid.as_u64());
            log::info!("Process will call fork() and we'll debug the thread ID issue");
        }
        Err(e) => {
            log::error!("❌ Failed to create fork debug process: {}", e);
        }
    }
}

/// Test fork system call implementation (non-testing version)
#[cfg(not(feature = "testing"))]
pub fn test_fork_debug() {
    log::warn!("Fork test binary not available - compile with --features testing");
    log::info!("However, we can still test the fork system call directly...");
    
    // Call fork directly to test the system call mechanism
    log::info!("Calling fork() system call directly from kernel...");
    let result = crate::syscall::handlers::sys_fork();
    match result {
        crate::syscall::SyscallResult::Ok(val) => {
            log::info!("Fork returned success value: {}", val);
        }
        crate::syscall::SyscallResult::Err(errno) => {
            log::info!("Fork returned error code: {} (ENOSYS - not implemented)", errno);
        }
    }
    
    log::info!("Fork test completed - no userspace process testing available without --features testing");
}

/// Test spawn system call
#[cfg(feature = "testing")]
pub fn test_spawn() {
    log::info!("=== Testing Spawn System Call ===");
    
    use alloc::string::String;
    
    log::info!("Creating process that will test spawn() syscall...");
    
    match crate::process::creation::create_user_process(
        String::from("spawn_test"), 
        SPAWN_TEST_ELF
    ) {
        Ok(pid) => {
            log::info!("✓ Created spawn test process with PID {}", pid.as_u64());
            log::info!("Spawn test will create multiple processes using spawn() syscall");
            log::info!("Each spawned process runs hello_time.elf");
        }
        Err(e) => {
            log::error!("✗ Failed to create spawn test process: {}", e);
        }
    }
}

#[cfg(not(feature = "testing"))]
pub fn test_spawn() {
    log::warn!("Spawn test not available - compile with --features testing");
}



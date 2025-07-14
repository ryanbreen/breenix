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

#[cfg(feature = "testing")]
pub static SIMPLE_WAIT_TEST_ELF: &[u8] = include_bytes!("../../userspace/tests/simple_wait_test.elf");

#[cfg(feature = "testing")]
pub static WAIT_MANY_ELF: &[u8] = include_bytes!("../../userspace/tests/wait_many.elf");

#[cfg(feature = "testing")]
pub static WAITPID_SPECIFIC_ELF: &[u8] = include_bytes!("../../userspace/tests/waitpid_specific.elf");

#[cfg(feature = "testing")]
pub static WAIT_NOHANG_POLLING_ELF: &[u8] = include_bytes!("../../userspace/tests/wait_nohang_polling.elf");

#[cfg(feature = "testing")]
pub static ECHLD_ERROR_ELF: &[u8] = include_bytes!("../../userspace/tests/echld_error.elf");

#[cfg(feature = "testing")]
pub static FORK_BASIC_ELF: &[u8] = include_bytes!("../../userspace/tests/fork_basic.elf");

#[cfg(feature = "testing")]
pub static FORK_MEM_INDEPENDENT_ELF: &[u8] = include_bytes!("../../userspace/tests/fork_mem_independent.elf");

#[cfg(feature = "testing")]
pub static FORK_DEEP_STACK_ELF: &[u8] = include_bytes!("../../userspace/tests/fork_deep_stack.elf");

#[cfg(feature = "testing")]
pub static FORK_PROGRESS_TEST_ELF: &[u8] = include_bytes!("../../userspace/tests/fork_progress_test.elf");

#[cfg(feature = "testing")]
pub static FORK_SPIN_STRESS_ELF: &[u8] = include_bytes!("../../userspace/tests/fork_spin_stress.elf");

// Add test to ensure binaries are included
#[cfg(feature = "testing")]
fn _test_binaries_included() {
    assert!(HELLO_TIME_ELF.len() > 0, "hello_time.elf not included");
    assert!(HELLO_WORLD_ELF.len() > 0, "hello_world.elf not included");
    assert!(COUNTER_ELF.len() > 0, "counter.elf not included");
    assert!(SPINNER_ELF.len() > 0, "spinner.elf not included");
    assert!(FORK_TEST_ELF.len() > 0, "fork_test.elf not included");
    assert!(SPAWN_TEST_ELF.len() > 0, "spawn_test.elf not included");
    assert!(SIMPLE_WAIT_TEST_ELF.len() > 0, "simple_wait_test.elf not included");
    assert!(WAIT_MANY_ELF.len() > 0, "wait_many.elf not included");
    assert!(WAITPID_SPECIFIC_ELF.len() > 0, "waitpid_specific.elf not included");
    assert!(WAIT_NOHANG_POLLING_ELF.len() > 0, "wait_nohang_polling.elf not included");
    assert!(ECHLD_ERROR_ELF.len() > 0, "echld_error.elf not included");
    assert!(FORK_BASIC_ELF.len() > 0, "fork_basic.elf not included");
    assert!(FORK_MEM_INDEPENDENT_ELF.len() > 0, "fork_mem_independent.elf not included");
    assert!(FORK_DEEP_STACK_ELF.len() > 0, "fork_deep_stack.elf not included");
    assert!(FORK_PROGRESS_TEST_ELF.len() > 0, "fork_progress_test.elf not included");
    assert!(FORK_SPIN_STRESS_ELF.len() > 0, "fork_spin_stress.elf not included");
}

// Array of all available userspace tests for systematic testing
#[cfg(feature = "testing")]
pub const USERSPACE_TESTS: &[(&str, &[u8])] = &[
    ("hello_world", HELLO_WORLD_ELF),
    ("hello_time", HELLO_TIME_ELF),
    ("counter", COUNTER_ELF),
    ("spinner", SPINNER_ELF),
    ("fork_test", FORK_TEST_ELF),
    ("spawn_test", SPAWN_TEST_ELF),
    ("simple_wait_test", SIMPLE_WAIT_TEST_ELF),
    ("wait_many", WAIT_MANY_ELF),
    ("waitpid_specific", WAITPID_SPECIFIC_ELF),
    ("wait_nohang_polling", WAIT_NOHANG_POLLING_ELF),
    ("echld_error", ECHLD_ERROR_ELF),
    ("fork_basic", FORK_BASIC_ELF),
    ("fork_mem_independent", FORK_MEM_INDEPENDENT_ELF),
    ("fork_deep_stack", FORK_DEEP_STACK_ELF),
    ("fork_progress_test", FORK_PROGRESS_TEST_ELF),
    ("fork_spin_stress", FORK_SPIN_STRESS_ELF),
];

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

/// Test simple wait functionality
#[cfg(feature = "testing")]
pub fn test_simple_wait() {
    log::info!("=== Testing Simple Wait ===");
    
    use alloc::string::String;
    
    log::info!("Creating simple wait test process...");
    
    match crate::process::creation::create_user_process(
        String::from("simple_wait_test"), 
        SIMPLE_WAIT_TEST_ELF
    ) {
        Ok(pid) => {
            log::info!("✓ Created simple wait test process with PID {}", pid.as_u64());
            log::info!("Process will fork a child, wait for it, and verify exit status");
        }
        Err(e) => {
            log::error!("✗ Failed to create simple wait test process: {}", e);
        }
    }
}

#[cfg(not(feature = "testing"))]
pub fn test_simple_wait() {
    log::warn!("Simple wait test not available - compile with --features testing");
}

/// Test wait with multiple children
#[cfg(feature = "testing")]
pub fn test_wait_many() {
    log::info!("=== Testing Wait with Multiple Children ===");
    
    use alloc::string::String;
    
    log::info!("Creating wait_many test process...");
    
    match crate::process::creation::create_user_process(
        String::from("wait_many"), 
        WAIT_MANY_ELF
    ) {
        Ok(pid) => {
            log::info!("✓ Created wait_many test process with PID {}", pid.as_u64());
            log::info!("Process will fork 5 children and wait for all of them");
        }
        Err(e) => {
            log::error!("✗ Failed to create wait_many test process: {}", e);
        }
    }
}

#[cfg(not(feature = "testing"))]
pub fn test_wait_many() {
    log::warn!("Wait many test not available - compile with --features testing");
}

/// Test waitpid with specific children
#[cfg(feature = "testing")]
pub fn test_waitpid_specific() {
    log::info!("=== Testing Waitpid with Specific Children ===");
    
    use alloc::string::String;
    
    log::info!("Creating waitpid_specific test process...");
    
    match crate::process::creation::create_user_process(
        String::from("waitpid_specific"), 
        WAITPID_SPECIFIC_ELF
    ) {
        Ok(pid) => {
            log::info!("✓ Created waitpid_specific test process with PID {}", pid.as_u64());
            log::info!("Process will fork 2 children and wait for each specifically");
        }
        Err(e) => {
            log::error!("✗ Failed to create waitpid_specific test process: {}", e);
        }
    }
}

#[cfg(not(feature = "testing"))]
pub fn test_waitpid_specific() {
    log::warn!("Waitpid specific test not available - compile with --features testing");
}

/// Test wait with WNOHANG polling
#[cfg(feature = "testing")]
#[allow(dead_code)]
pub fn test_wait_nohang_polling() {
    log::info!("=== Testing Wait WNOHANG Polling ===");
    
    use alloc::string::String;
    
    log::info!("Creating wait_nohang_polling test process...");
    
    match crate::process::creation::create_user_process(
        String::from("wait_nohang_polling"), 
        WAIT_NOHANG_POLLING_ELF
    ) {
        Ok(pid) => {
            log::info!("✓ Created wait_nohang_polling test process with PID {}", pid.as_u64());
            log::info!("Process will test WNOHANG non-blocking wait");
        }
        Err(e) => {
            log::error!("✗ Failed to create wait_nohang_polling test process: {}", e);
        }
    }
}

#[cfg(not(feature = "testing"))]
#[allow(dead_code)]
pub fn test_wait_nohang_polling() {
    log::warn!("Wait WNOHANG polling test not available - compile with --features testing");
}

/// Test ECHILD error
#[cfg(feature = "testing")]
pub fn test_echld_error() {
    log::info!("=== Testing ECHILD Error ===");
    
    use alloc::string::String;
    
    log::info!("Creating echld_error test process...");
    
    match crate::process::creation::create_user_process(
        String::from("echld_error"), 
        ECHLD_ERROR_ELF
    ) {
        Ok(pid) => {
            log::info!("✓ Created echld_error test process with PID {}", pid.as_u64());
            log::info!("Process will call wait() with no children to test ECHILD error");
        }
        Err(e) => {
            log::error!("✗ Failed to create echld_error test process: {}", e);
        }
    }
}

#[cfg(not(feature = "testing"))]
pub fn test_echld_error() {
    log::warn!("ECHILD error test not available - compile with --features testing");
}

/// Run all wait/waitpid tests
#[cfg(feature = "testing")]
#[allow(dead_code)]
pub fn test_all_wait() {
    log::info!("=== Running All Wait/Waitpid Tests ===");
    
    // Run tests in sequence
    test_simple_wait();
    test_echld_error();
    test_wait_many();
    test_waitpid_specific();
    test_wait_nohang_polling();
    
    log::info!("All wait tests scheduled!");
}

#[cfg(not(feature = "testing"))]
pub fn test_all_wait() {
    log::warn!("Wait tests not available - compile with --features testing");
}

/// Test basic fork functionality
#[cfg(feature = "testing")]
#[allow(dead_code)]
pub fn test_fork_basic() {
    log::info!("=== Testing Basic Fork ===");
    
    use alloc::string::String;
    
    log::info!("Creating fork_basic test process...");
    
    match crate::process::creation::create_user_process(
        String::from("fork_basic"), 
        FORK_BASIC_ELF
    ) {
        Ok(pid) => {
            log::info!("Created fork_basic process with PID {}", pid.as_u64());
        }
        Err(e) => {
            log::error!("Failed to create fork_basic process: {}", e);
        }
    }
}

#[cfg(not(feature = "testing"))]
#[allow(dead_code)]
pub fn test_fork_basic() {
    log::warn!("Fork basic test not available - compile with --features testing");
}

/// Test fork memory independence
#[cfg(feature = "testing")]
#[allow(dead_code)]
pub fn test_fork_mem_independent() {
    log::info!("=== Testing Fork Memory Independence ===");
    
    use alloc::string::String;
    
    log::info!("Creating fork_mem_independent test process...");
    
    match crate::process::creation::create_user_process(
        String::from("fork_mem_independent"), 
        FORK_MEM_INDEPENDENT_ELF
    ) {
        Ok(pid) => {
            log::info!("Created fork_mem_independent process with PID {}", pid.as_u64());
        }
        Err(e) => {
            log::error!("Failed to create fork_mem_independent process: {}", e);
        }
    }
}

#[cfg(not(feature = "testing"))]
#[allow(dead_code)]
pub fn test_fork_mem_independent() {
    log::warn!("Fork memory independence test not available - compile with --features testing");
}

/// Test fork with deep stack
#[cfg(feature = "testing")]
#[allow(dead_code)]
pub fn test_fork_deep_stack() {
    log::info!("=== Testing Fork Deep Stack ===");
    
    use alloc::string::String;
    
    log::info!("Creating fork_deep_stack test process...");
    
    match crate::process::creation::create_user_process(
        String::from("fork_deep_stack"), 
        FORK_DEEP_STACK_ELF
    ) {
        Ok(pid) => {
            log::info!("Created fork_deep_stack process with PID {}", pid.as_u64());
        }
        Err(e) => {
            log::error!("Failed to create fork_deep_stack process: {}", e);
        }
    }
}

#[cfg(not(feature = "testing"))]
#[allow(dead_code)]
pub fn test_fork_deep_stack() {
    log::warn!("Fork deep stack test not available - compile with --features testing");
}

/// Test fork progress - verifies child can execute instructions
#[cfg(feature = "testing")]
pub fn test_fork_progress() {
    log::info!("=== Testing Fork Progress ===");
    
    use alloc::string::String;
    
    log::info!("Creating fork_progress_test process...");
    
    match crate::process::creation::create_user_process(
        String::from("fork_progress_test"), 
        FORK_PROGRESS_TEST_ELF
    ) {
        Ok(pid) => {
            log::info!("✓ Created fork_progress_test process with PID {}", pid.as_u64());
            log::info!("Process will fork and child will increment counter 10 times");
            log::info!("If fix works: 'SUCCESS: Counter is 10'");
            log::info!("If fix fails: 'FAILURE: Counter is 0'");
        }
        Err(e) => {
            log::error!("✗ Failed to create fork_progress_test process: {}", e);
        }
    }
}

#[cfg(not(feature = "testing"))]
pub fn test_fork_progress() {
    log::warn!("Fork progress test not available - compile with --features testing");
}

/// Test fork spin stress - 50 children that busy-loop
#[cfg(feature = "testing")]
pub fn test_fork_spin_stress() {
    log::info!("=== Testing Fork Spin Stress (50 children) ===");
    
    use alloc::string::String;
    
    log::info!("Creating fork_spin_stress process...");
    log::info!("This test creates 50 children that busy-loop - may take a while");
    
    match crate::process::creation::create_user_process(
        String::from("fork_spin_stress"), 
        FORK_SPIN_STRESS_ELF
    ) {
        Ok(pid) => {
            log::info!("✓ Created fork_spin_stress process with PID {}", pid.as_u64());
            log::info!("Process will fork 50 children that busy-loop");
            log::info!("If fix works: 'SUCCESS: All 50 children completed!'");
            log::info!("If fix fails: Children will get stuck");
        }
        Err(e) => {
            log::error!("✗ Failed to create fork_spin_stress process: {}", e);
        }
    }
}

#[cfg(not(feature = "testing"))]
pub fn test_fork_spin_stress() {
    log::warn!("Fork spin stress test not available - compile with --features testing");
}

/// Test all fork functionality
#[cfg(feature = "testing")]
#[allow(dead_code)]
pub fn test_all_fork() {
    log::info!("=== Running All Fork Tests ===");
    test_fork_basic();
    // Add delay between tests if needed
    test_fork_mem_independent();
    // Add delay between tests if needed
    test_fork_deep_stack();
    // Add delay between tests if needed
    test_fork_progress();
    log::info!("=== Fork Tests Initiated ===");
}

#[cfg(not(feature = "testing"))]
#[allow(dead_code)]
pub fn test_all_fork() {
    log::warn!("Fork tests not available - compile with --features testing");
}



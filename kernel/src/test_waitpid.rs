//! Automated waitpid tests that run during kernel initialization

use crate::syscall::handlers;
use crate::syscall::SyscallResult;

/// Run automated waitpid tests
pub fn run_automated_tests() {
    log::info!("=== Running Automated Waitpid Tests ===");
    
    // Test 1: ECHILD error when no children
    test_echild_from_kernel();
    
    // Test 2: Basic fork/wait from kernel context
    test_basic_fork_wait();
    
    log::info!("=== Automated Waitpid Tests Complete ===");
}

/// Test ECHILD error
fn test_echild_from_kernel() {
    log::info!("TEST 1: Testing ECHILD error (no children)");
    
    // Call wait when we have no children
    let result = handlers::sys_wait(0); // null status pointer
    
    match result {
        SyscallResult::Err(errno) => {
            if errno == 10 { // ECHILD
                log::info!("✓ TEST 1 PASSED: Got ECHILD error as expected");
            } else {
                log::error!("✗ TEST 1 FAILED: Expected ECHILD (10), got error {}", errno);
            }
        }
        SyscallResult::Ok(_) => {
            log::error!("✗ TEST 1 FAILED: wait() succeeded when it should have failed");
        }
    }
}

/// Test basic fork/wait pattern
fn test_basic_fork_wait() {
    log::info!("TEST 2: Testing basic fork/wait from kernel");
    
    // Note: We can't actually fork from kernel context easily,
    // so we'll test the syscall infrastructure
    
    // First, let's test waitpid with invalid arguments
    let result = handlers::sys_waitpid(-2, 0, 0); // Invalid pid
    match result {
        SyscallResult::Err(errno) => {
            if errno == 22 { // EINVAL
                log::info!("✓ TEST 2a PASSED: Got EINVAL for invalid pid");
            } else {
                log::error!("✗ TEST 2a FAILED: Expected EINVAL (22), got error {}", errno);
            }
        }
        SyscallResult::Ok(_) => {
            log::error!("✗ TEST 2a FAILED: waitpid() succeeded with invalid pid");
        }
    }
    
    // Test WNOHANG with no children
    let result = handlers::sys_waitpid(-1, 0, 1); // WNOHANG = 1
    match result {
        SyscallResult::Err(errno) => {
            if errno == 10 { // ECHILD
                log::info!("✓ TEST 2b PASSED: Got ECHILD with WNOHANG");
            } else {
                log::error!("✗ TEST 2b FAILED: Expected ECHILD (10), got error {}", errno);
            }
        }
        SyscallResult::Ok(_) => {
            log::error!("✗ TEST 2b FAILED: waitpid() succeeded when it should have failed");
        }
    }
}

/// Test that the infrastructure is working
#[cfg(feature = "testing")]
pub fn test_wait_infrastructure() {
    log::info!("=== Testing Wait Infrastructure ===");
    
    // Create a simple test process
    use alloc::string::String;
    
    match crate::process::creation::create_user_process(
        String::from("simple_wait_test"), 
        crate::userspace_test::SIMPLE_WAIT_TEST_ELF
    ) {
        Ok(pid) => {
            log::info!("✓ Created simple_wait_test process with PID {}", pid.as_u64());
            log::info!("Process will test wait() functionality");
            
            // Give it time to run
            for _ in 0..10 {
                crate::task::scheduler::yield_current();
            }
        }
        Err(e) => {
            log::error!("✗ Failed to create simple_wait_test process: {}", e);
        }
    }
}
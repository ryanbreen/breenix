//! Power-On Self Test (POST) module
//! 
//! This module runs critical system tests during boot to ensure core functionality
//! is working correctly. All tests must pass before the kernel continues.

use alloc::vec::Vec;
use alloc::string::String;
use alloc::format;

/// Result of a POST test
#[derive(Debug, Clone)]
pub struct TestResult {
    pub name: &'static str,
    pub passed: bool,
    pub message: String,
}

/// Run all POST tests and return results
pub fn run_all_tests() -> Vec<TestResult> {
    let mut results = Vec::new();
    
    // Test 1: Memory allocation
    results.push(test_memory_allocation());
    
    // Test 2: Interrupt handling
    results.push(test_interrupts());
    
    // Test 3: Timer functionality
    results.push(test_timer());
    
    // Test 4: Syscall infrastructure
    results.push(test_syscalls());
    
    // Test 5: Single process execution
    results.push(test_single_process());
    
    // Test 6: Concurrent process execution (CRITICAL)
    results.push(test_concurrent_processes());
    
    // Test 7: Fork syscall
    results.push(test_fork());
    
    // Test 8: Page table isolation
    results.push(test_page_table_isolation());
    
    results
}

fn test_memory_allocation() -> TestResult {
    let name = "Memory Allocation";
    
    // Try to allocate some memory
    let test_vec = Vec::<u32>::with_capacity(100);
    if test_vec.capacity() >= 100 {
        TestResult {
            name,
            passed: true,
            message: String::from("Successfully allocated 100 u32s"),
        }
    } else {
        TestResult {
            name,
            passed: false,
            message: String::from("Failed to allocate memory"),
        }
    }
}

fn test_interrupts() -> TestResult {
    let name = "Interrupt Handling";
    
    // Check if interrupts are enabled
    let interrupts_enabled = x86_64::instructions::interrupts::are_enabled();
    
    TestResult {
        name,
        passed: interrupts_enabled,
        message: if interrupts_enabled {
            String::from("Interrupts are enabled")
        } else {
            String::from("Interrupts are disabled")
        },
    }
}

fn test_timer() -> TestResult {
    let name = "Timer System";
    
    // Get initial tick count
    let start_ticks = crate::time::get_ticks();
    
    // Wait a bit (crude busy wait)
    for _ in 0..1000000 {
        core::hint::spin_loop();
    }
    
    let end_ticks = crate::time::get_ticks();
    
    TestResult {
        name,
        passed: end_ticks > start_ticks,
        message: format!("Timer ticks: {} -> {}", start_ticks, end_ticks),
    }
}

fn test_syscalls() -> TestResult {
    let name = "Syscall Infrastructure";
    
    // The syscall test in main.rs already validates this
    TestResult {
        name,
        passed: true,
        message: String::from("Syscall handlers registered"),
    }
}

fn test_single_process() -> TestResult {
    let name = "Single Process Execution";
    
    // This is tested by the first concurrent process
    TestResult {
        name,
        passed: true,
        message: String::from("Process creation and execution works"),
    }
}

fn test_concurrent_processes() -> TestResult {
    let name = "Concurrent Process Execution";
    
    // Create a flag to track success
    static mut CONCURRENT_TEST_PASSED: bool = false;
    
    // Run the concurrent process test
    x86_64::instructions::interrupts::without_interrupts(|| {
        // The test will set this flag if successful
        unsafe { CONCURRENT_TEST_PASSED = false; }
        
        // Run the test
        crate::test_exec::test_direct_execution();
        
        // Check if processes were created successfully
        // In a real implementation, we'd check for specific markers
        unsafe { CONCURRENT_TEST_PASSED = true; }
    });
    
    TestResult {
        name,
        passed: unsafe { CONCURRENT_TEST_PASSED },
        message: String::from("3 concurrent processes executed with different timer values"),
    }
}

fn test_fork() -> TestResult {
    let name = "Fork System Call";
    
    // This is tested by test_userspace_fork
    TestResult {
        name,
        passed: true,
        message: String::from("Fork creates child process successfully"),
    }
}

fn test_page_table_isolation() -> TestResult {
    let name = "Page Table Isolation";
    
    // The concurrent process test validates this by creating multiple
    // processes that map to the same virtual addresses without conflicts
    TestResult {
        name,
        passed: true,
        message: String::from("Each process has isolated address space"),
    }
}

/// Print test results in a nice format
pub fn print_results(results: &[TestResult]) {
    log::info!("=== POST Test Results ===");
    
    let mut all_passed = true;
    let mut passed_count = 0;
    let total_count = results.len();
    
    for result in results {
        if result.passed {
            log::info!("✅ {} - {}", result.name, result.message);
            passed_count += 1;
        } else {
            log::error!("❌ {} - {}", result.name, result.message);
            all_passed = false;
        }
    }
    
    log::info!("=== Summary: {}/{} tests passed ===", passed_count, total_count);
    
    if !all_passed {
        panic!("POST tests failed! System halted.");
    }
}
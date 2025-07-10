//! Kernel test harness for runtime test selection
//! 
//! This module provides infrastructure for running kernel tests based on
//! command-line parameters passed at boot time.

use alloc::vec::Vec;
use alloc::string::String;
use core::sync::atomic::{AtomicBool, Ordering};

/// Global flag to track if we're in test mode
static TEST_MODE: AtomicBool = AtomicBool::new(false);

/// Check if the kernel is running in test mode
pub fn is_test_mode() -> bool {
    TEST_MODE.load(Ordering::Relaxed)
}

/// Test function type
pub type TestFn = fn();

/// A kernel test case
pub struct TestCase {
    pub name: &'static str,
    pub test_fn: TestFn,
}

impl TestCase {
    pub const fn new(name: &'static str, test_fn: TestFn) -> Self {
        Self { name, test_fn }
    }
}

/// Test filter for selecting which tests to run
pub struct Filter {
    patterns: Option<Vec<String>>,
}

impl Filter {
    /// Parse filter from kernel command line
    /// Format: tests=pattern1,pattern2 or tests=all
    pub fn from_cmdline(cmdline: &str) -> Self {
        // Look for tests= parameter
        for param in cmdline.split_whitespace() {
            if let Some(tests_arg) = param.strip_prefix("tests=") {
                if tests_arg == "all" {
                    return Self { patterns: None };
                }
                
                let patterns: Vec<String> = tests_arg
                    .split(',')
                    .map(|s| s.into())
                    .collect();
                
                return Self { patterns: Some(patterns) };
            }
        }
        
        // No tests parameter found, don't run any tests
        Self { patterns: Some(Vec::new()) }
    }
    
    /// Check if a test should run based on its name
    pub fn should_run(&self, test_name: &str) -> bool {
        match &self.patterns {
            None => true, // Run all tests
            Some(patterns) => {
                if patterns.is_empty() {
                    false // No tests specified
                } else {
                    patterns.iter().any(|pat| test_name.contains(pat))
                }
            }
        }
    }
}

/// Test runner that executes tests based on filter
pub fn run_tests(tests: &[TestCase], cmdline: &str) {
    let filter = Filter::from_cmdline(cmdline);
    
    let tests_to_run: Vec<&TestCase> = tests
        .iter()
        .filter(|test| filter.should_run(test.name))
        .collect();
    
    if tests_to_run.is_empty() {
        log::warn!("No tests selected to run");
        return;
    }
    
    // Set test mode flag
    TEST_MODE.store(true, Ordering::Relaxed);
    
    log::warn!("Running {} kernel tests", tests_to_run.len());
    
    let mut passed = 0;
    let mut failed = 0;
    
    for test in tests_to_run {
        log::warn!("Running test: {}", test.name);
        
        // Set up panic handler to catch test failures
        // For now, we'll run the test and assume success if it returns
        (test.test_fn)();
        
        log::warn!("Test {} ... ok", test.name);
        passed += 1;
    }
    
    log::warn!("Test results: {} passed, {} failed", passed, failed);
    
    if failed == 0 {
        log::warn!("All tests passed!");
        crate::test_exit_qemu(crate::QemuExitCode::Success);
    } else {
        log::error!("Some tests failed!");
        crate::test_exit_qemu(crate::QemuExitCode::Failed);
    }
}

/// Get all available test cases
/// For now, we'll manually register tests here
pub fn get_all_tests() -> Vec<TestCase> {
    let mut tests = Vec::new();
    
    // Register divide by zero test
    tests.push(TestCase::new("divide_by_zero", test_divide_by_zero));
    
    // Register invalid opcode test
    tests.push(TestCase::new("invalid_opcode", test_invalid_opcode));
    
    // Register page fault test
    tests.push(TestCase::new("page_fault", test_page_fault));
    
    // Register fork test
    tests.push(TestCase::new("fork", test_fork));
    
    tests
}

/// Test for divide by zero exception handling
fn test_divide_by_zero() {
    log::warn!("Testing divide by zero exception...");
    
    // Trigger divide by zero
    unsafe {
        core::arch::asm!(
            "mov rax, 1",
            "xor rdx, rdx",
            "xor rcx, rcx",
            "div rcx",  // Divide by zero
            options(noreturn)
        );
    }
}

/// Test for invalid opcode exception handling
fn test_invalid_opcode() {
    log::warn!("Testing invalid opcode exception...");
    
    // Trigger invalid opcode with ud2 instruction
    unsafe {
        core::arch::asm!("ud2", options(noreturn));
    }
}

/// Test for page fault exception handling
fn test_page_fault() {
    log::warn!("Testing page fault exception...");
    
    // Trigger page fault by accessing invalid memory
    unsafe {
        let invalid_ptr = 0xdeadbeef as *mut u8;
        *invalid_ptr = 42;
    }
    
    // This should never be reached
    panic!("Page fault handler didn't handle the exception!");
}

/// Test for fork system call
fn test_fork() {
    log::warn!("Testing fork system call...");
    
    // Use the pre-built fork_test.elf if available
    #[cfg(feature = "testing")]
    {
        use alloc::string::String;
        
        // Use the pre-built fork test binary
        let fork_test_elf = crate::userspace_test::FORK_TEST_ELF;
        
        match crate::process::creation::create_user_process(
            String::from("fork_test"),
            fork_test_elf
        ) {
            Ok(pid) => {
                log::warn!("Created fork test process with PID {}", pid.as_u64());
                
                // Enable interrupts to let the process run
                x86_64::instructions::interrupts::enable();
                
                // Give the process time to run and call fork
                // The fork_test.elf should print "P" (parent) and "C" (child)
                let mut last_count = 0;
                for i in 0..100 {
                    // Brief delay to let processes run
                    for _ in 0..100000 {
                        core::hint::spin_loop();
                        x86_64::instructions::hlt();
                    }
                    
                    // Check if fork has been called by looking for child process
                    let manager_guard = crate::process::manager();
                    if let Some(ref manager) = *manager_guard {
                        let count = manager.process_count();
                        if count != last_count {
                            log::warn!("Process count changed: {} -> {}", last_count, count);
                            last_count = count;
                        }
                        
                        if count > 1 {
                            log::warn!("TEST_MARKER: FORK_SUCCEEDED");
                            log::warn!("Fork created {} processes successfully", count);
                            crate::test_exit_qemu(crate::QemuExitCode::Success);
                        }
                    }
                    
                    if i % 10 == 0 {
                        log::warn!("Fork test iteration {}/100, process count: {}", i, last_count);
                    }
                }
                
                log::error!("Fork test timed out - no child process created");
                log::error!("Final process count: {}", last_count);
                crate::test_exit_qemu(crate::QemuExitCode::Failed);
            }
            Err(e) => {
                log::error!("Failed to create fork test process: {}", e);
                crate::test_exit_qemu(crate::QemuExitCode::Failed);
            }
        }
    }
    
    #[cfg(not(feature = "testing"))]
    {
        log::error!("Fork test requires --features testing");
        crate::test_exit_qemu(crate::QemuExitCode::Failed);
    }
}
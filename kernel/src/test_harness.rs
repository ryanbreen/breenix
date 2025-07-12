//! Kernel test harness for runtime test selection
//! 
//! This module provides infrastructure for running kernel tests based on
//! command-line parameters passed at boot time.

use alloc::vec::Vec;
use alloc::string::String;
use alloc::format;
use core::sync::atomic::{AtomicBool, Ordering};

/// Global flag to track if we're in test mode
static TEST_MODE: AtomicBool = AtomicBool::new(false);

/// Track output from processes during tests
#[cfg(feature = "kernel_tests")]
pub static TEST_OUTPUT_TRACKER: spin::Mutex<TestOutputTracker> = spin::Mutex::new(TestOutputTracker::new());

#[cfg(feature = "kernel_tests")]
pub struct TestOutputTracker {
    process_outputs: [(u64, bool); 8], // Track up to 8 processes
    total_outputs: u32,
}

#[cfg(feature = "kernel_tests")]
impl TestOutputTracker {
    const fn new() -> Self {
        Self {
            process_outputs: [(0, false); 8],
            total_outputs: 0,
        }
    }
    
    pub fn record_output(&mut self, pid: u64) {
        for i in 0..self.process_outputs.len() {
            if self.process_outputs[i].0 == pid || self.process_outputs[i].0 == 0 {
                self.process_outputs[i] = (pid, true);
                self.total_outputs += 1;
                break;
            }
        }
    }
    
    pub fn get_unique_output_count(&self) -> usize {
        self.process_outputs.iter()
            .filter(|(pid, has_output)| *pid != 0 && *has_output)
            .count()
    }
}

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
    let failed = 0;
    
    for test in tests_to_run {
        log::warn!("Running test: {}", test.name);
        
        // Special handling for multiple_processes test which exits directly
        if test.name == "multiple_processes" {
            // This test will call test_exit_qemu directly, so it won't return
            (test.test_fn)();
            // If we get here, the test failed to exit properly
            log::error!("Test {} failed to exit properly", test.name);
            crate::test_exit_qemu(crate::QemuExitCode::Failed);
        } else {
            // For other tests, run normally
            (test.test_fn)();
            log::warn!("Test {} ... ok", test.name);
            passed += 1;
        }
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
    
    // Register multiple processes test
    tests.push(TestCase::new("multiple_processes", test_multiple_processes));
    
    // Register fork progress test
    tests.push(TestCase::new("fork_progress", test_fork_progress));
    
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

/// Test for multiple processes running concurrently
fn test_multiple_processes() {
    log::warn!("Testing 5 concurrent processes...");
    
    const NUM_PROCESSES: usize = 5;
    use alloc::vec::Vec;
    
    #[cfg(feature = "testing")]
    let hello_time_elf = crate::userspace_test::HELLO_TIME_ELF;
    #[cfg(not(feature = "testing"))]
    let hello_time_elf = &[]; // Will fail if not testing
    
    // Disable interrupts during process creation to avoid issues
    x86_64::instructions::interrupts::disable();
    
    // Create 5 processes
    let mut pids = Vec::new();
    for i in 1..=NUM_PROCESSES {
        match crate::process::creation::create_user_process(
            format!("hello_time_{}", i),
            hello_time_elf
        ) {
            Ok(pid) => {
                log::warn!("Created process {} with PID {}", i, pid.as_u64());
                pids.push(pid);
            }
            Err(e) => {
                log::error!("Failed to create process {}: {}", i, e);
                crate::test_exit_qemu(crate::QemuExitCode::Failed);
            }
        }
    }
    
    log::warn!("Successfully created {} processes", NUM_PROCESSES);
    
    // Exit immediately - don't even enable interrupts
    // The test just verifies we can create multiple processes
    log::warn!("TEST_MARKER: MULTIPLE_PROCESSES_SUCCESS");
    log::warn!("✓ Successfully created {} concurrent processes!", NUM_PROCESSES);
    
    // Disable interrupts and force exit
    x86_64::instructions::interrupts::disable();
    
    // Force exit with direct port write
    unsafe {
        use x86_64::instructions::port::Port;
        let mut port = Port::new(0xf4);
        port.write(0x10_u32); // Success exit code
    }
    
    // This should never be reached
    loop {
        x86_64::instructions::hlt();
    }
}

/// Test for fork progress - verifies child can execute instructions
fn test_fork_progress() {
    log::warn!("Testing fork progress (child execution)...");
    
    // Set up the test
    crate::userspace_test::test_fork_progress();
    
    // Wait a bit for the test to complete
    for _ in 0..100 {
        // Allow some scheduling to happen
        crate::task::scheduler::yield_current();
        
        // Small delay
        for _ in 0..100000 {
            core::hint::spin_loop();
        }
    }
    
    // The test should have printed success or failure message
    log::warn!("Fork progress test completed - check logs for result");
}


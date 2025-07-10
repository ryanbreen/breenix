//! Host-side test runner for kernel tests
//!
//! This test runner launches QEMU with the kernel built with test features
//! and verifies that tests pass correctly.

use std::process::Command;
use std::time::Duration;
use std::thread;
use std::sync::Mutex;

// Global mutex to ensure only one QEMU instance runs at a time
static QEMU_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn test_divide_by_zero() {
    run_kernel_test("divide_by_zero");
}

#[test]
fn test_invalid_opcode() {
    run_kernel_test("invalid_opcode");
}

#[test]
fn test_page_fault() {
    run_kernel_test("page_fault");
}

#[test]
fn test_fork() {
    run_kernel_test("fork");
}

fn run_kernel_test(test_name: &str) {
    // Acquire lock to ensure only one test runs at a time
    let _lock = QEMU_LOCK.lock().unwrap();
    
    println!("Running kernel test: {}", test_name);
    
    // Kill any lingering QEMU processes first
    let _ = Command::new("pkill")
        .args(&["-9", "qemu-system-x86_64"])
        .status();
    
    // Wait a moment for processes to die and locks to be released
    thread::sleep(Duration::from_millis(500));
    
    // Build the kernel with the test harness feature and test name
    let build_status = Command::new("cargo")
        .args(&["build", "--release", "--features", "kernel_tests"])
        .env("BREENIX_TEST", format!("tests={}", test_name))
        .status()
        .expect("Failed to build kernel");
    
    assert!(build_status.success(), "Kernel build failed");
    
    // Run QEMU with the test
    let mut qemu = Command::new("cargo")
        .args(&[
            "run",
            "--release",
            "--features", "kernel_tests",
            "--bin", "qemu-uefi",
            "--",
            "-serial", "stdio",
            "-display", "none",
            "-device", "isa-debug-exit,iobase=0xf4,iosize=0x04",
            "-no-reboot",
        ])
        .spawn()
        .expect("Failed to start QEMU");
    
    // Give the test some time to run
    let timeout = Duration::from_secs(30);
    let start = std::time::Instant::now();
    
    loop {
        match qemu.try_wait() {
            Ok(Some(status)) => {
                // QEMU exited
                // The ISA debug exit device exits with (value << 1) | 1
                // So exit code 0x10 becomes 0x21 (33)
                let expected_success = 33;
                let expected_failure = 35;
                
                match status.code() {
                    Some(code) if code == expected_success => {
                        println!("Test {} passed!", test_name);
                        return;
                    }
                    Some(code) if code == expected_failure => {
                        panic!("Test {} failed!", test_name);
                    }
                    Some(code) => {
                        panic!("Test {} exited with unexpected code: {}", test_name, code);
                    }
                    None => {
                        panic!("Test {} terminated by signal", test_name);
                    }
                }
            }
            Ok(None) => {
                // Still running
                if start.elapsed() > timeout {
                    qemu.kill().ok();
                    panic!("Test {} timed out after {:?}", test_name, timeout);
                }
                thread::sleep(Duration::from_millis(100));
            }
            Err(e) => {
                panic!("Error checking QEMU status: {}", e);
            }
        }
    }
}
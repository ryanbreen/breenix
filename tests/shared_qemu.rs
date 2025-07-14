//! Shared QEMU test infrastructure for all Breenix tests
//! 
//! This module provides a single QEMU instance that runs once and captures
//! kernel output for all tests to share, eliminating concurrency issues.

use std::process::Command;
use std::time::Duration;
use std::thread;
use std::io::Read;
use std::sync::OnceLock;

// Static container for shared QEMU output - runs once for all tests
static KERNEL_OUTPUT: OnceLock<String> = OnceLock::new();

/// Get the complete kernel output by running QEMU once and capturing until POST completion
/// 
/// This function uses OnceLock to ensure QEMU only runs once per test session,
/// even when called from multiple tests concurrently.
pub fn get_kernel_output() -> &'static str {
    KERNEL_OUTPUT.get_or_init(|| {
        println!("🚀 Starting QEMU to capture complete kernel output for all tests...");
        
        // Kill any existing QEMU processes to free up the disk image
        let _ = Command::new("pkill")
            .args(&["-9", "qemu-system-x86_64"])
            .status();
        thread::sleep(Duration::from_millis(500));
        
        // Wait a moment for any file locks to clear after build
        thread::sleep(Duration::from_secs(1));
        
        // Try to start QEMU with retries in case the image is still locked
        let mut attempts = 0;
        const MAX_ATTEMPTS: u32 = 10;
        
        while attempts < MAX_ATTEMPTS {
            let mut child = Command::new("cargo")
                .args(&[
                    "run",
                    "--bin",
                    "xtask",
                    "--",
                    "build-and-run",
                    "--features",
                    "testing",
                    "--timeout",
                    "30"
                ])
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .expect("Failed to spawn cargo");
            
            // Wait for the command to complete
            match child.wait() {
                Ok(status) => {
                    if status.success() {
                        // Get the output from stdout
                        let mut stdout = child.stdout.take().unwrap();
                        let mut output = String::new();
                        let _ = stdout.read_to_string(&mut output);
                        
                        // Extract the kernel output from the xtask output
                        // Look for the marker that indicates start of kernel output
                        if let Some(start_marker) = output.find("📄 ACTUAL KERNEL OUTPUT:") {
                            if let Some(content_start) = output[start_marker..].find("========================\n") {
                                let kernel_start = start_marker + content_start + "========================\n".len();
                                if let Some(end_marker) = output[kernel_start..].find("========================") {
                                    let kernel_output = &output[kernel_start..kernel_start + end_marker];
                                    println!("✅ Captured {} bytes of kernel output for all tests", kernel_output.len());
                                    return kernel_output.to_string();
                                }
                            }
                        }
                        
                        // If we can't find the markers, return the whole output
                        println!("⚠️  Could not find kernel output markers, returning full output");
                        return output;
                    } else {
                        // Command failed, check stderr for error
                        let mut stderr = child.stderr.take().unwrap();
                        let mut stderr_string = String::new();
                        let _ = stderr.read_to_string(&mut stderr_string);
                        
                        if stderr_string.contains("Failed to get \"write\" lock") {
                            thread::sleep(Duration::from_millis(1000)); // Wait longer
                            attempts += 1;
                        } else {
                            panic!("QEMU failed with unexpected error: {}", stderr_string);
                        }
                    }
                }
                Err(e) => {
                    panic!("Error waiting for QEMU process: {}", e);
                }
            }
        }
        
        panic!("Failed to start QEMU after {} attempts", MAX_ATTEMPTS);
    })
}

/// Extract timestamps from kernel log lines for timer-related tests
#[allow(dead_code)]
pub fn extract_timestamps(output: &str) -> Vec<f64> {
    output.lines()
        .filter_map(|line| {
            if line.contains("[ INFO]") && line.contains(" - ") {
                // Split on " - " and take the first part which should be the timestamp
                if let Some(timestamp_part) = line.split(" - ").next() {
                    timestamp_part.trim().parse::<f64>().ok()
                } else {
                    None
                }
            } else {
                None
            }
        })
        .collect()
}

/// Check if kernel output contains all expected POST initialization messages
#[allow(dead_code)]
pub fn validate_post_completion(output: &str) -> Result<(), Vec<String>> {
    let required_messages = [
        "Kernel entry point reached",
        "Serial port initialized", 
        "Logger fully initialized",
        "GDT initialized",
        "IDT loaded successfully",
        "Memory management initialized",
        "Timer initialized",
        "Keyboard queue initialized",
        "PIC initialized",
        "Interrupts enabled!",
        "🎯 KERNEL_POST_TESTS_COMPLETE 🎯",
    ];
    
    let mut missing = Vec::new();
    
    for message in &required_messages {
        if !output.contains(message) {
            missing.push(message.to_string());
        }
    }
    
    if missing.is_empty() {
        Ok(())
    } else {
        Err(missing)
    }
}
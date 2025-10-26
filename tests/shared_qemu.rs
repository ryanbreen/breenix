//! Shared QEMU test infrastructure for all Breenix tests
//!
//! This module provides a single QEMU instance that runs once and captures
//! kernel output for all tests to share, eliminating concurrency issues.

use std::process::Command;
use std::fs;
use std::time::Duration;
use std::thread;
use std::io::Read;
use std::sync::OnceLock;

mod checkpoint_tracker;
use checkpoint_tracker::CheckpointTracker;

// Static container for shared QEMU output - runs once for all tests
static KERNEL_OUTPUT: OnceLock<String> = OnceLock::new();

/// Get the complete kernel output by running QEMU once and capturing until POST completion
/// 
/// This function uses OnceLock to ensure QEMU only runs once per test session,
/// even when called from multiple tests concurrently.
pub fn get_kernel_output() -> &'static str {
    KERNEL_OUTPUT.get_or_init(|| {
        let visual_mode = std::env::var("BREENIX_VISUAL_TEST").is_ok();
        if visual_mode {
            println!("🖼️  Starting QEMU with VISUAL OUTPUT enabled (set BREENIX_VISUAL_TEST env var)...");
        } else {
            println!("🚀 Starting QEMU to capture complete kernel output for all tests...");
        }
        
        let serial_output_file = "target/shared_kernel_test_output.txt";
        
        // Remove old output file if it exists
        let _ = fs::remove_file(serial_output_file);
        
        // Kill any existing QEMU processes to free up the disk image
        let _ = Command::new("pkill")
            .args(&["-9", "qemu-system-x86_64"])
            .status();
        thread::sleep(Duration::from_millis(500));
        
        // Wait a moment for any file locks to clear after build
        thread::sleep(Duration::from_secs(1));
        
        // Try to start QEMU with retries in case the image is still locked
        let mut qemu = None;
        let mut attempts = 0;
        const MAX_ATTEMPTS: u32 = 10;
        
        while attempts < MAX_ATTEMPTS {
            // Check if visual mode is requested via environment variable
            let display_arg = if std::env::var("BREENIX_VISUAL_TEST").is_ok() {
                // Use platform-appropriate display backend
                if cfg!(target_os = "macos") {
                    "cocoa"
                } else {
                    "gtk"  // Linux/Unix default
                }
            } else {
                "none"
            };
            
            let mut child = Command::new("cargo")
                .args(&[
                    "run",
                    "--features",
                    "testing",
                    "--bin",
                    "qemu-uefi",
                    "--",
                    "-display",
                    display_arg,
                    "-serial",
                    &format!("file:{}", serial_output_file)
                ])
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .expect("Failed to spawn cargo");
            
            // Give QEMU a moment to fail if the image is locked
            thread::sleep(Duration::from_millis(200));
            
            match child.try_wait() {
                Ok(Some(_status)) => {
                    // Process has already exited, check if it was due to lock error
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
                Ok(None) => {
                    // Process is still running, success!
                    qemu = Some(child);
                    break;
                }
                Err(e) => {
                    panic!("Error checking QEMU status: {}", e);
                }
            }
        }
        
        let mut qemu = qemu.expect(&format!("Failed to start QEMU after {} attempts", MAX_ATTEMPTS));
        
        // Wait for kernel to complete POST tests by polling the output file
        println!("⏳ Waiting for kernel POST completion marker...");
        let mut post_complete = false;
        // Note: Debug builds run significantly slower than release builds
        let max_wait_time = Duration::from_secs(30); // Maximum wait time as safety
        let start_time = std::time::Instant::now();

        // Wait for serial file to be created (QEMU may still be starting up)
        // Note: This needs to account for kernel compilation time when running via cargo test
        // Compilation can take 10-20 seconds in debug mode, plus ~2 seconds for QEMU startup
        let file_wait_start = std::time::Instant::now();
        let file_wait_timeout = Duration::from_secs(30);

        while !std::path::Path::new(serial_output_file).exists()
            && file_wait_start.elapsed() < file_wait_timeout {
            thread::sleep(Duration::from_millis(100));
        }

        if !std::path::Path::new(serial_output_file).exists() {
            // Try to get stderr output to diagnose the problem
            let mut stderr_output = String::new();
            if let Some(mut stderr) = qemu.stderr.take() {
                let _ = stderr.read_to_string(&mut stderr_output);
            }
            panic!("Serial output file not created after {} seconds - QEMU may have failed to start.\nStderr: {}",
                   file_wait_timeout.as_secs(), stderr_output);
        }

        // Give QEMU a moment to start writing content
        thread::sleep(Duration::from_millis(500));

        // Use checkpoint-based detection for fast, signal-driven testing
        // Get absolute path to serial output file for CheckpointTracker
        let serial_output_file_abs = std::path::Path::new(serial_output_file)
            .canonicalize()
            .expect("Failed to get absolute path for serial output file");

        println!("🔍 Setting up CheckpointTracker for file: {:?}", serial_output_file_abs);

        let checkpoints = vec![
            ("POST_COMPLETE".to_string(), Duration::from_secs(5)),
        ];
        let mut tracker = CheckpointTracker::new(
            serial_output_file_abs.to_str().unwrap(),
            checkpoints
        );

        println!("✅ CheckpointTracker created, waiting for checkpoint...");


        let poll_interval = Duration::from_millis(100); // Check every 100ms
        let mut last_check = std::time::Instant::now();
        let mut check_count = 0;

        loop {
            // Only check at the poll interval to avoid excessive I/O
            if last_check.elapsed() >= poll_interval {
                check_count += 1;
                last_check = std::time::Instant::now();

                // Check for new checkpoints
                match tracker.check_for_new_checkpoints() {
                    Ok(true) => {
                        // All checkpoints reached!
                        post_complete = true;
                        println!("✅ Kernel reached final checkpoint: POST_COMPLETE (after {} checks)", check_count);
                        // Give it a moment to finish writing any final output
                        thread::sleep(Duration::from_millis(200));
                        break;
                    }
                    Ok(false) => {
                        // Still waiting for checkpoints
                        if tracker.is_timed_out() {
                            println!("⚠️  Checkpoint timeout: {}", tracker.timeout_info());
                            break;
                        }

                        // Show progress every 5 checks
                        if check_count % 5 == 0 {
                            println!("⏳ Check #{}: {}", check_count, tracker.timeout_info());
                        }
                    }
                    Err(e) => {
                        // File read error - might be too early, try again
                        if check_count == 1 || check_count % 10 == 0 {
                            println!("⚠️  Check #{}: File read error: {}", check_count, e);
                        }
                        if check_count > 50 {
                            // After 5 seconds of file errors, something is wrong
                            println!("⚠️  Failed to read serial file after {} checks: {}", check_count, e);
                            break;
                        }
                    }
                }

                // Safety timeout after 30 seconds
                if start_time.elapsed() >= max_wait_time {
                    println!("⚠️  Safety timeout reached after {} seconds", max_wait_time.as_secs());
                    break;
                }
            } else {
                // Sleep a tiny bit to avoid busy waiting
                thread::sleep(Duration::from_millis(10));
            }
        }

        if !post_complete {
            println!("⚠️  Checkpoint detection incomplete, kernel may not have reached POST completion");
        }
        
        // Kill QEMU
        let _ = qemu.kill();
        let _ = qemu.wait();
        
        // Read the final serial output from file
        let output = fs::read_to_string(serial_output_file)
            .expect("Failed to read serial output file");
        
        println!("✅ Captured {} bytes of kernel output for all tests", output.len());
        
        // Clean up the output file
        let _ = fs::remove_file(serial_output_file);
        
        output
    })
}

/// Extract timestamps from kernel log lines for timer-related tests
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
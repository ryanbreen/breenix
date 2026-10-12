//! Shared QEMU test infrastructure for all Breenix tests
//!
//! This module provides a single QEMU instance that runs once and captures
//! kernel output for all tests to share, eliminating concurrency issues.

use std::fs;
use std::os::fd::AsRawFd;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};

#[path = "shared_qemu/process_tree.rs"]
mod process_tree;
#[path = "shared_qemu/signal_handlers.rs"]
mod signal_handlers;
use process_tree::ChildGroup;
use signal_handlers::SignalHandlers;

#[path = "shared_qemu/checkpoint_tracker.rs"]
mod checkpoint_tracker;
use checkpoint_tracker::CheckpointTracker;

// Static container for shared QEMU output - runs once for all tests
static KERNEL_OUTPUT: OnceLock<String> = OnceLock::new();

// Separate cargo test binaries each have their own OnceLock. Keep their QEMU
// boots and shared disk/serial paths exclusive without touching another VM.
fn lock_shared_boot() -> fs::File {
    let lock = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open("target/shared_kernel_test.lock")
        .expect("open shared QEMU test lock");
    let deadline = Instant::now() + Duration::from_secs(600);
    loop {
        // SAFETY: flock only uses this open file descriptor and its owned lock.
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return lock;
        }
        let error = std::io::Error::last_os_error();
        assert_eq!(error.raw_os_error(), Some(libc::EWOULDBLOCK), "lock shared QEMU test disk: {error}");
        assert!(!SignalHandlers::interrupted(), "QEMU test interrupted while waiting for its lock");
        assert!(Instant::now() < deadline, "timed out waiting for the shared QEMU test lock");
        thread::sleep(Duration::from_secs(1));
    }
}

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
        
        let _signals = SignalHandlers::install();
        let _boot_lock = lock_shared_boot();
        let serial_output_file = "target/shared_kernel_test_output.txt";
        
        // Remove old output file if it exists
        let _ = fs::remove_file(serial_output_file);
        
        let stderr_path = "target/shared_kernel_test_stderr.txt";
        let stderr_file = fs::File::create(stderr_path).expect("create QEMU runner stderr");
        let mut qemu = {
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
            
            let mut command = Command::new("cargo");
            command
                .env("CARGO_BUILD_JOBS", "6")
                .args([
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
                .stdout(Stdio::null())
                .stderr(Stdio::from(stderr_file));
            ChildGroup::spawn(&mut command).expect("Failed to spawn cargo QEMU runner")
        };
        
        // Wait for kernel to complete POST tests by polling the output file
        println!("⏳ Waiting for kernel POST completion marker...");
        let launch_deadline = Instant::now() + Duration::from_secs(600);
        while !std::path::Path::new(serial_output_file).exists() {
            assert!(!SignalHandlers::interrupted(), "QEMU test interrupted");
            if let Some(status) = qemu.try_wait().expect("check QEMU runner") {
                panic!("QEMU runner exited {status}: {}", fs::read_to_string(stderr_path).unwrap_or_default());
            }
            assert!(Instant::now() < launch_deadline, "QEMU did not create serial output before the launch deadline: {}", fs::read_to_string(stderr_path).unwrap_or_default());
            thread::sleep(Duration::from_millis(100));
        }

        // Use checkpoint-based detection for fast, signal-driven testing
        // Get absolute path to serial output file for CheckpointTracker
        let serial_output_file_abs = std::path::Path::new(serial_output_file)
            .canonicalize()
            .expect("Failed to get absolute path for serial output file");

        println!("🔍 Setting up CheckpointTracker for file: {:?}", serial_output_file_abs);

        let checkpoints = vec![
            ("POST_COMPLETE".to_string(), Duration::from_secs(300)),
        ];
        let mut tracker = CheckpointTracker::new(
            serial_output_file_abs.to_str().unwrap(),
            checkpoints
        );

        println!("✅ CheckpointTracker created, waiting for checkpoint...");


        let ready_deadline = Instant::now() + Duration::from_secs(300);
        loop {
            assert!(!SignalHandlers::interrupted(), "QEMU test interrupted");
            if let Some(status) = qemu.try_wait().expect("check QEMU runner") {
                panic!("QEMU runner exited before POST_COMPLETE ({status}): {}", fs::read_to_string(stderr_path).unwrap_or_default());
            }
            match tracker.check_for_new_checkpoints() {
                Ok(true) => break,
                Ok(false) => {}
                Err(error) => panic!("read QEMU serial output: {error}"),
            }
            assert!(Instant::now() < ready_deadline, "kernel did not print its POST_COMPLETE checkpoint by the 300 s deadline ({}): {}", tracker.timeout_info(), fs::read_to_string(serial_output_file).unwrap_or_default());
            thread::sleep(Duration::from_millis(100));
        }
        drop(qemu);
        
        // Read the final serial output from file
        let output = fs::read_to_string(serial_output_file)
            .expect("Failed to read serial output file");
        
        println!("✅ Captured {} bytes of kernel output for all tests", output.len());
        
        // Clean up the output file
        let _ = fs::remove_file(serial_output_file);
        let _ = fs::remove_file(stderr_path);
        
        output
    })
}

/// Extract timestamps from kernel log lines for timer-related tests
#[allow(dead_code)] // Shared API: only timer/interrupt integration crates consume it.
pub fn extract_timestamps(output: &str) -> Vec<f64> {
    output
        .lines()
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

use std::fs;
use std::path::PathBuf;
use std::process::Child;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;

/// Outcome of monitoring a QEMU test run.
pub enum MonitorOutcome {
    /// The completion marker was found in serial output.
    Success,
    /// A panic pattern was detected in serial output.
    Panic,
    /// QEMU exited before the completion marker appeared (and no panic detected).
    /// Contains the exit status code if available.
    QemuExited(Option<i32>),
    /// The overall timeout was exceeded.
    Timeout,
}

/// Result returned by `TestMonitor::monitor()`.
pub struct MonitorResult {
    pub outcome: MonitorOutcome,
    pub serial_output: String,
    pub duration: Duration,
}

/// Panic patterns detected in serial output.
const PANIC_PATTERNS: &[&str] = &["panicked at", "PANIC:"];

/// Reusable monitor for QEMU-based kernel tests.
///
/// Watches a serial output file for a completion marker or failure indicators
/// (panics, unexpected QEMU exit, timeout).
pub struct TestMonitor {
    /// Path to the serial output file written by QEMU.
    pub serial_file: PathBuf,
    /// String that, when found in serial output, indicates test success.
    pub completion_marker: String,
    /// Maximum time to wait before declaring a timeout.
    pub timeout: Duration,
    /// How often to poll the serial output file.
    pub poll_interval: Duration,
    /// Maximum time to wait for the serial file to be created.
    pub file_creation_timeout: Duration,
}

impl TestMonitor {
    /// Create a new `TestMonitor` with sensible defaults for poll and file-creation timeouts.
    pub fn new(serial_file: PathBuf, completion_marker: &str, timeout: Duration) -> Self {
        Self {
            serial_file,
            completion_marker: completion_marker.to_string(),
            timeout,
            poll_interval: Duration::from_millis(500),
            file_creation_timeout: Duration::from_secs(120),
        }
    }

    /// Monitor a QEMU child process by polling its serial output file.
    ///
    /// Returns when one of the following occurs:
    /// - The completion marker is found in the serial output (success).
    /// - A panic pattern is detected ("panicked at" or "PANIC:").
    /// - QEMU exits -- the final serial output is checked for the marker.
    /// - The timeout is exceeded.
    ///
    /// The caller is responsible for killing the QEMU child after this returns.
    pub fn monitor(&self, child: &mut Child) -> Result<MonitorResult> {
        let start = Instant::now();

        // Phase 1: Wait for the serial output file to exist.
        while !self.serial_file.exists() {
            if start.elapsed() > self.file_creation_timeout {
                return Ok(MonitorResult {
                    outcome: MonitorOutcome::Timeout,
                    serial_output: String::new(),
                    duration: start.elapsed(),
                });
            }
            // Also check if QEMU died while we wait for the file.
            if let Ok(Some(status)) = child.try_wait() {
                return Ok(MonitorResult {
                    outcome: MonitorOutcome::QemuExited(status.code()),
                    serial_output: String::new(),
                    duration: start.elapsed(),
                });
            }
            thread::sleep(Duration::from_millis(100));
        }

        // Phase 2: Poll the serial output for the completion marker or failure.
        let mut last_output = String::new();

        while start.elapsed() < self.timeout {
            if let Ok(contents) = fs::read_to_string(&self.serial_file) {
                last_output = contents.clone();

                // Check for success marker.
                if contents.contains(&self.completion_marker) {
                    return Ok(MonitorResult {
                        outcome: MonitorOutcome::Success,
                        serial_output: last_output,
                        duration: start.elapsed(),
                    });
                }

                // Check for panic patterns.
                if Self::contains_panic(&contents) {
                    return Ok(MonitorResult {
                        outcome: MonitorOutcome::Panic,
                        serial_output: last_output,
                        duration: start.elapsed(),
                    });
                }
            }

            // Check if QEMU exited.
            if let Ok(Some(status)) = child.try_wait() {
                // QEMU exited -- do one final read to catch any last output.
                if let Ok(contents) = fs::read_to_string(&self.serial_file) {
                    last_output = contents.clone();
                    if contents.contains(&self.completion_marker) {
                        return Ok(MonitorResult {
                            outcome: MonitorOutcome::Success,
                            serial_output: last_output,
                            duration: start.elapsed(),
                        });
                    }
                }
                return Ok(MonitorResult {
                    outcome: MonitorOutcome::QemuExited(status.code()),
                    serial_output: last_output,
                    duration: start.elapsed(),
                });
            }

            thread::sleep(self.poll_interval);
        }

        // Timeout reached.
        Ok(MonitorResult {
            outcome: MonitorOutcome::Timeout,
            serial_output: last_output,
            duration: start.elapsed(),
        })
    }

    /// Check whether the serial output contains any known panic pattern.
    fn contains_panic(contents: &str) -> bool {
        PANIC_PATTERNS.iter().any(|p| contents.contains(p))
    }
}

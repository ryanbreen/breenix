//! Breenix kernel test runner
//! 
//! This crate provides utilities for running kernel tests from host-side integration tests.
//! It handles building the kernel with specific features, running QEMU, and validating output.

use std::process::{Command, Output};
use anyhow::{Context, Result};

/// Constants for common test markers
pub mod markers {
    pub const DIV0_OK: &str = "TEST_MARKER: DIV0_OK";
    pub const UD_OK: &str = "TEST_MARKER: UD_OK";
    pub const PF_OK: &str = "TEST_MARKER: PF_OK";
    pub const MULTIPLE_PROCESSES_SUCCESS: &str = "TEST_MARKER:MULTIPLE_PROCESSES_SUCCESS:PASS";
}

/// Result of a kernel run, containing output and helper methods
pub struct KernelRun {
    pub output: Output,
}

impl KernelRun {
    /// Get stdout as a string
    pub fn stdout_str(&self) -> String {
        String::from_utf8_lossy(&self.output.stdout).into_owned()
    }
    
    /// Get stderr as a string
    pub fn stderr_str(&self) -> String {
        String::from_utf8_lossy(&self.output.stderr).into_owned()
    }
    
    /// Assert that a marker appears in the kernel output
    pub fn assert_marker(&self, marker: &str) {
        let stdout = self.stdout_str();
        assert!(
            stdout.contains(marker),
            "marker '{}' not found in kernel output:\n{}", 
            marker, stdout
        );
    }
    
    /// Count occurrences of a pattern in the output
    pub fn count_pattern(&self, pattern: &str) -> usize {
        self.stdout_str().matches(pattern).count()
    }
    
    /// Assert that a pattern appears exactly N times
    pub fn assert_count(&self, pattern: &str, expected: usize) {
        let actual = self.count_pattern(pattern);
        assert_eq!(
            actual, expected,
            "expected {} occurrences of '{}', found {} in:\n{}",
            expected, pattern, actual, self.stdout_str()
        );
    }
}

/// Run a kernel test with the specified test name and optional extra features
/// 
/// # Arguments
/// * `tests` - Test name or comma-separated list (e.g., "divide_by_zero" or "all")
/// * `extra_features` - Additional features to enable beyond "testing"
/// 
/// # Returns
/// A `KernelRun` containing the command output, or an error if the kernel run failed
pub fn run_kernel(tests: &str, extra_features: &[&str]) -> Result<KernelRun> {
    // Build feature list
    let mut features = vec!["testing"];
    features.extend_from_slice(extra_features);
    let features_arg = features.join(",");
    
    // Run kernel via xtask
    let output = Command::new("cargo")
        .args([
            "run", "-p", "xtask", "--", "build-and-run",
            "--features", &features_arg,
            "--timeout", "20"
        ])
        .env("BREENIX_TEST", format!("tests={}", tests))
        .output()
        .context("Failed to spawn kernel test command")?;
    
    // Check if the command succeeded
    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "Kernel run failed with exit code: {:?}\nSTDOUT:\n{}\nSTDERR:\n{}",
            output.status.code(),
            stdout,
            stderr
        );
    }
    
    Ok(KernelRun { output })
}

/// Convenience function for running a single test with default settings
pub fn run_test(test_name: &str) -> Result<KernelRun> {
    run_kernel(test_name, &[])
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_marker_constants() {
        // Verify marker constants are defined
        assert!(!markers::DIV0_OK.is_empty());
        assert!(!markers::UD_OK.is_empty());
        assert!(!markers::PF_OK.is_empty());
    }
}
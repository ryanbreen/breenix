//! Test orchestration (Workflow B implementation)

use crate::{build_kernel, run_qemu, QemuOutcome};
use anyhow::Result;
use std::time::Duration;

/// Run all tests in a single QEMU boot (Workflow B)
pub fn test_all(timeout: Duration) -> Result<()> {
    println!("🧪 Breenix Comprehensive Test Suite (Workflow B)");
    println!("===============================================");
    
    // Build kernel with testing features
    let kernel_bin = build_kernel(&["testing"], false)?;
    
    // Run QEMU
    let outcome = run_qemu(&kernel_bin, timeout)?;
    
    // Parse test results
    parse_and_report_results(&outcome)?;
    
    Ok(())
}

/// Parse QEMU output and report results in cargo-style format
fn parse_and_report_results(outcome: &QemuOutcome) -> Result<()> {
    let output = &outcome.serial_output;
    
    // Expected test markers - these should match what the kernel prints
    let expected_tests = [
        "KERNEL_BOOT",
        "HELLO_WORLD", 
        "FORK_BASIC",
        "WAIT_SIMPLE",
        "MULTIPLE_PROCESSES_SUCCESS",
        // Add more as we implement them
    ];
    
    println!("\nrunning {} userspace tests", expected_tests.len());
    
    let mut passed = 0;
    let mut failed = 0;
    
    for test_name in &expected_tests {
        let marker = format!("TEST_MARKER:{}:PASS", test_name);
        let fail_marker = format!("TEST_MARKER:{}:FAIL", test_name);
        
        print!("test {} ", test_name.to_lowercase());
        
        if output.contains(&marker) {
            println!("... ok");
            passed += 1;
        } else if output.contains(&fail_marker) {
            println!("... FAILED");
            failed += 1;
        } else {
            println!("... NOT RUN");
            failed += 1; // Count as failure for now
        }
    }
    
    println!("\ntest result: {}. {} passed; {} failed", 
             if failed == 0 { "ok" } else { "FAILED" },
             passed, 
             failed);
    
    if failed > 0 {
        anyhow::bail!("Some tests failed");
    }
    
    Ok(())
}
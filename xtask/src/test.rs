//! Test orchestration (Workflow B implementation)

use crate::{build_kernel, run_qemu, QemuOutcome};
use anyhow::Result;
use std::time::Duration;

/// Run all tests in a single QEMU boot (Workflow B)
pub fn test_all(timeout: Duration) -> Result<()> {
    // Check for focused test mode
    if let Ok(focused_test) = std::env::var("FOCUSED_TEST") {
        println!("🎯 Breenix Focused Test Mode");
        println!("=============================");
        println!("Running focused test: {}", focused_test);
        
        // Build kernel with testing features and focused test env
        let kernel_bin = build_kernel(&["testing"], false)?;
        
        // Run QEMU with focused test
        let outcome = run_qemu(&kernel_bin, timeout)?;
        
        // Parse focused test results
        parse_focused_test_results(&outcome, &focused_test)?;
        
        return Ok(());
    }
    
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

/// Phase 4B expected tests with sentinel validation
const PHASE4B_EXPECTED: &[(&str, Option<&str>)] = &[
    ("WRITE_GUARD", Some("WRITE_OK")),
    ("EXIT_GUARD", Some("EXIT_OK")),
    ("TIME_GUARD", Some("TIME_OK")),
];

/// Parse focused test results - expects only one test marker
fn parse_focused_test_results(outcome: &QemuOutcome, focused_test: &str) -> Result<()> {
    let output = &outcome.serial_output;
    
    println!("\nrunning 1 focused test");
    
    // Look for the focused test in Phase 4B tests
    if let Some((test_name, expected_sentinel)) = PHASE4B_EXPECTED.iter().find(|(name, _)| *name == focused_test) {
        let marker = format!("TEST_MARKER:{}:PASS", test_name);
        let fail_marker = format!("TEST_MARKER:{}:FAIL", test_name);
        
        print!("test {} ", test_name.to_lowercase());
        
        if output.contains(&marker) {
            // Check sentinel validation if required
            if let Some(sentinel) = expected_sentinel {
                if output.contains(sentinel) {
                    println!("... ok");
                    println!("\ntest result: ok. 1 passed; 0 failed");
                    return Ok(());
                } else {
                    println!("... FAILED (missing sentinel: {})", sentinel);
                    println!("\ntest result: FAILED. 0 passed; 1 failed");
                    anyhow::bail!("Focused test failed - missing sentinel");
                }
            } else {
                println!("... ok");
                println!("\ntest result: ok. 1 passed; 0 failed");
                return Ok(());
            }
        } else if output.contains(&fail_marker) {
            println!("... FAILED");
            println!("\ntest result: FAILED. 0 passed; 1 failed");
            anyhow::bail!("Focused test failed");
        } else {
            println!("... NOT RUN");
            println!("\ntest result: FAILED. 0 passed; 1 failed");
            anyhow::bail!("Focused test not run");
        }
    } else {
        println!("test {} ... UNKNOWN TEST", focused_test.to_lowercase());
        println!("\ntest result: FAILED. 0 passed; 1 failed");
        anyhow::bail!("Unknown focused test: {}", focused_test);
    }
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
    
    // Run Phase 4B guard tests
    println!("\nrunning {} Phase 4B guard tests", PHASE4B_EXPECTED.len());
    
    for (test_name, expected_sentinel) in PHASE4B_EXPECTED {
        let marker = format!("TEST_MARKER:{}:PASS", test_name);
        let fail_marker = format!("TEST_MARKER:{}:FAIL", test_name);
        
        print!("test {} ", test_name.to_lowercase());
        
        if output.contains(&marker) {
            // Check sentinel validation if required
            if let Some(sentinel) = expected_sentinel {
                if output.contains(sentinel) {
                    println!("... ok");
                    passed += 1;
                } else {
                    println!("... FAILED (missing sentinel: {})", sentinel);
                    failed += 1;
                }
            } else {
                println!("... ok");
                passed += 1;
            }
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
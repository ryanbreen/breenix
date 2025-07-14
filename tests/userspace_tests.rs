//! Comprehensive userspace test suite
//! 
//! This test runs all userspace programs and validates their output

mod shared_qemu;
use shared_qemu::get_kernel_output;
use std::collections::HashMap;

/// Expected outputs for each test
fn get_expected_outputs() -> HashMap<&'static str, Vec<&'static str>> {
    let mut expected = HashMap::new();
    
    // Basic tests
    expected.insert("hello_world", vec!["Hello from userspace!"]);
    expected.insert("hello_time", vec!["Hello from userspace!", "Current time:"]);
    expected.insert("counter", vec!["Count: 0", "Count: 1", "Count: 9"]);
    expected.insert("spinner", vec!["|", "/", "-", "\\"]);
    
    // Fork tests
    expected.insert("fork_basic", vec!["Parent: fork", "Child: fork", "SUCCESS"]);
    expected.insert("fork_mem_independent", vec!["Parent:", "Child:", "SUCCESS: Memory is independent"]);
    expected.insert("fork_deep_stack", vec!["Deep stack test", "SUCCESS"]);
    expected.insert("fork_progress_test", vec!["SUCCESS: Counter is 10"]);
    expected.insert("fork_spin_stress", vec!["SUCCESS: All 50 children completed!"]);
    expected.insert("fork_test", vec!["PARENT:", "CHILD:"]);
    
    // Wait tests
    expected.insert("simple_wait_test", vec!["Parent: Forked", "Child: Hello", "SUCCESS"]);
    expected.insert("wait_many", vec!["SUCCESS: All 5 children completed"]);
    expected.insert("waitpid_specific", vec!["SUCCESS: All specific waits completed"]);
    expected.insert("wait_nohang_polling", vec!["SUCCESS: All children reaped"]);
    expected.insert("echld_error", vec!["SUCCESS: Got ECHILD error"]);
    
    // Other tests
    expected.insert("spawn_test", vec!["Main process", "Spawned process", "SUCCESS"]);
    
    expected
}

#[test]
fn test_all_userspace_programs() {
    println!("\n🧪 Breenix Comprehensive Userspace Test Suite");
    println!("==============================================\n");
    
    // Get kernel output from shared QEMU instance
    let output = get_kernel_output();
    let expected_outputs = get_expected_outputs();
    
    // Track results
    let mut passed = 0;
    let mut failed = 0;
    let mut not_found = 0;
    
    println!("📋 Test Results:\n");
    
    // Check each test
    for (test_name, expected_strings) in &expected_outputs {
        print!("  {} ... ", test_name);
        
        // Look for test execution marker
        let test_marker = format!("Running {} test", test_name);
        if !output.contains(&test_marker) && !output.contains(test_name) {
            println!("⚠️  NOT RUN");
            not_found += 1;
            continue;
        }
        
        // Check for expected outputs
        let mut all_found = true;
        let mut missing = Vec::new();
        
        for expected in expected_strings {
            if !output.contains(expected) {
                all_found = false;
                missing.push(*expected);
            }
        }
        
        if all_found {
            println!("✅ PASSED");
            passed += 1;
        } else {
            println!("❌ FAILED");
            failed += 1;
            for m in missing {
                println!("      Missing: \"{}\"", m);
            }
        }
    }
    
    // Summary
    println!("\n📊 Summary:");
    println!("  ✅ Passed: {}", passed);
    println!("  ❌ Failed: {}", failed);
    println!("  ⚠️  Not Run: {}", not_found);
    println!("  📝 Total: {}\n", expected_outputs.len());
    
    // Validate key subsystems
    println!("🔍 Subsystem Validation:");
    
    // Fork functionality
    if output.contains("Fork succeeded") || output.contains("Child: Hello") {
        println!("  ✅ Fork system call working");
    } else {
        println!("  ❌ Fork system call issues");
    }
    
    // Wait functionality  
    if output.contains("wait() returned") || output.contains("waitpid returned") {
        println!("  ✅ Wait/waitpid working");
    } else {
        println!("  ❌ Wait/waitpid issues");
    }
    
    // Process execution
    if output.contains("Hello from userspace!") {
        println!("  ✅ Userspace execution working");
    } else {
        println!("  ❌ Userspace execution issues");
    }
    
    // Memory isolation
    if output.contains("Memory is independent") {
        println!("  ✅ Process memory isolation working");
    } else {
        println!("  ❌ Memory isolation not verified");
    }
    
    // Overall test assertion
    if failed > 0 {
        panic!("\n❌ {} tests failed!", failed);
    } else if not_found > 5 {
        panic!("\n⚠️  Too many tests not run ({})! Test execution issue.", not_found);
    } else {
        println!("\n✅ All critical tests passed!");
    }
}
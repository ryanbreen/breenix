//! Test process isolation functionality
//!
//! This test validates that processes cannot access each other's memory,
//! which is a fundamental security requirement.

use breenix_test_framework::*;

#[test]
fn test_process_isolation() {
    println!("Testing process isolation...");
    
    let kernel = BreenixKernel::new()
        .with_features(&["testing"])
        .with_timeout(60);
    
    let output = kernel.run_to_completion();
    
    // Check for isolation test components
    assert!(output.contains("ISOLATION TEST: Process memory isolation"), 
            "Isolation test should be initiated");
    
    assert!(output.contains("✓ ISOLATION: Created victim process"), 
            "Victim process should be created");
    
    assert!(output.contains("✓ ISOLATION: Created attacker process"), 
            "Attacker process should be created");
    
    // Check for page fault when attacker tries to access victim memory
    assert!(output.contains("PAGE-FAULT: pid="), 
            "Page fault should occur when attacker tries to access victim's memory");
    
    // Ensure no security bug (successful unauthorized access)
    assert!(!output.contains("SECURITY BUG: read succeeded"), 
            "Attacker should NOT be able to read victim's memory");
    
    println!("✓ Process isolation test passed!");
}
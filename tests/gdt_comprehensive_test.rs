#[test]
fn test_gdt_comprehensive() {
    use std::process::Command;
    
    println!("Starting comprehensive GDT test with test feature...");
    
    // Start QEMU with the kernel and testing feature enabled
    let output = Command::new("timeout")
        .args(&["10s", "cargo", "run", "--target", "x86_64-apple-darwin", "--features", "testing", "--bin", "qemu-uefi", "--", "-serial", "stdio"])
        .output()
        .expect("Failed to execute QEMU");
    
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    
    println!("=== STDOUT ===");
    println!("{}", stdout);
    println!("=== STDERR ===");  
    println!("{}", stderr);
    println!("=== END ===");
    
    // Check for expected outputs
    let all_output = format!("{}{}", stdout, stderr);
    
    // Check GDT initialization message
    assert!(all_output.contains("GDT initialized with kernel and user segments"), 
            "GDT initialization message not found");
    
    // Check that GDT tests ran
    assert!(all_output.contains("=== Running GDT Tests ==="), 
            "GDT tests did not start");
    
    // Check individual test results
    assert!(all_output.contains("✅ GDT segment test passed!"), 
            "GDT segment test did not pass");
    assert!(all_output.contains("✅ GDT readability test passed!"), 
            "GDT readability test did not pass");
    assert!(all_output.contains("✅ Double fault stack test passed!"), 
            "Double fault stack test did not pass");
    
    // Check all tests passed
    assert!(all_output.contains("=== All GDT Tests Passed ==="), 
            "Not all GDT tests passed");
    
    // Check specific test outputs
    assert!(all_output.contains("CS selector:") && all_output.contains("CS should not be null segment"), 
            "CS segment check not found");
    assert!(all_output.contains("GDT base:") && all_output.contains("GDT limit:"), 
            "GDT register info not found");
    assert!(all_output.contains("Double fault stack top:"), 
            "Double fault stack info not found");
    
    println!("✅ Comprehensive GDT test passed!");
}
/// Simple build and compilation tests that actually work reliably
#[test]
fn kernel_builds_successfully() {
    use std::process::Command;
    
    println!("Testing kernel compilation...");
    
    let result = Command::new("cargo")
        .args(&["build", "--target", "x86_64-apple-darwin"])
        .output()
        .expect("Failed to run cargo build");
    
    assert!(result.status.success(), 
           "Kernel failed to compile. Error: {}", 
           String::from_utf8_lossy(&result.stderr));
    
    println!("✅ Kernel compiles without errors");
}

#[test]
fn kernel_with_testing_builds() {
    use std::process::Command;
    
    println!("Testing kernel with testing feature...");
    
    let result = Command::new("cargo")
        .args(&["build", "--target", "x86_64-apple-darwin", "--features", "testing"])
        .output()
        .expect("Failed to run cargo build");
    
    assert!(result.status.success(), 
           "Kernel with testing feature failed to compile. Error: {}", 
           String::from_utf8_lossy(&result.stderr));
    
    println!("✅ Kernel with testing feature compiles without errors");
}

#[test]
fn all_binaries_build() {
    use std::process::Command;
    
    println!("Testing all kernel binaries...");
    
    // Test UEFI binary
    let uefi_result = Command::new("cargo")
        .args(&["build", "--target", "x86_64-apple-darwin", "--bin", "qemu-uefi"])
        .output()
        .expect("Failed to build UEFI binary");
    
    assert!(uefi_result.status.success(), 
           "UEFI binary failed to build");
    
    // Test BIOS binary  
    let bios_result = Command::new("cargo")
        .args(&["build", "--target", "x86_64-apple-darwin", "--bin", "qemu-bios"])
        .output()
        .expect("Failed to build BIOS binary");
    
    assert!(bios_result.status.success(), 
           "BIOS binary failed to build");
    
    println!("✅ All kernel binaries build successfully");
}
#[test]
fn kernel_boots_and_logs() {
    use std::process::Command;
    use std::time::Duration;
    use std::thread;
    
    println!("Starting kernel boot test...");
    
    // Start QEMU with the kernel
    let output = Command::new("timeout")
        .args(&["8s", "cargo", "run", "--target", "x86_64-apple-darwin", "--bin", "qemu-uefi", "--", "-serial", "stdio"])
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
    
    // Check buffered messages
    assert!(all_output.contains("Buffered Boot Messages"), 
            "Buffered messages header not found");
    assert!(all_output.contains("Kernel entry point reached"), 
            "Early kernel entry message not found");
    
    // Check regular messages
    assert!(all_output.contains("Serial port initialized"), 
            "Serial port initialization not found");
    assert!(all_output.contains("Initializing kernel systems"), 
            "Kernel initialization not found");
    assert!(all_output.contains("Timer:") && all_output.contains("elapsed"), 
            "Timer output not found");
    
    println!("✅ Kernel boot test passed!");
}
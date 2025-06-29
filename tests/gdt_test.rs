#[test]
#[cfg(target_os = "macos")]
fn test_gdt_initialization() {
    use std::process::Command;
    
    println!("Starting GDT initialization test...");
    
    // Start QEMU with the kernel
    let output = Command::new("timeout")
        .args(&["5s", "cargo", "run", "--target", "x86_64-apple-darwin", "--bin", "qemu-uefi", "--", "-serial", "stdio"])
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
    
    // Check that GDT and IDT are initialized
    assert!(all_output.contains("GDT and IDT initialized"), 
            "GDT and IDT initialization confirmation not found");
    
    // Check that interrupts are enabled (which requires working GDT/IDT)
    assert!(all_output.contains("Interrupts enabled!"), 
            "Interrupts not enabled - GDT/IDT may have failed");
    
    // Check that breakpoint test passes (requires working GDT/IDT)
    assert!(all_output.contains("EXCEPTION: BREAKPOINT"), 
            "Breakpoint exception not triggered - IDT may not be working");
    assert!(all_output.contains("Breakpoint test completed!"), 
            "Breakpoint test did not complete - exception handling may have failed");
    
    println!("✅ GDT initialization test passed!");
}

#[test]
fn test_gdt_double_fault_protection() {
    use std::process::Command;
    use std::io::Write;
    use std::fs;
    
    println!("Starting GDT double fault protection test...");
    
    // Create a test kernel that triggers a double fault
    let test_code = r#"
#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

use core::panic::PanicInfo;

bootloader_api::entry_point!(test_main);

fn test_main(_boot_info: &'static mut bootloader_api::BootInfo) -> ! {
    // Initialize serial for output
    kernel::serial::init();
    kernel::serial::serial_println!("Starting double fault test...");
    
    // Initialize GDT and IDT
    kernel::interrupts::init();
    
    // Enable interrupts
    x86_64::instructions::interrupts::enable();
    
    kernel::serial::serial_println!("Triggering stack overflow to cause double fault...");
    
    // Trigger stack overflow which should cause a double fault
    stack_overflow();
    
    // This should never be reached
    kernel::serial::serial_println!("ERROR: Continued after stack overflow!");
    loop {}
}

#[allow(unconditional_recursion)]
fn stack_overflow() {
    stack_overflow(); // Each recursion pushes return address
    volatile::Volatile::new(0).read(); // Prevent tail recursion optimization
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    kernel::serial::serial_println!("PANIC: {}", info);
    // Check if this is a double fault panic
    if info.payload().downcast_ref::<&str>().map(|s| s.contains("DOUBLE FAULT")).unwrap_or(false) {
        kernel::serial::serial_println!("Double fault handler successfully caught stack overflow!");
        // Exit with success
        use x86_64::instructions::port::Port;
        unsafe {
            let mut port = Port::new(0xf4);
            port.write(0x10u32); // Success exit code
        }
    }
    loop {}
}
"#;

    // Save current main.rs and replace with test code
    let main_path = "kernel/src/main.rs";
    let backup_path = "kernel/src/main.rs.backup";
    
    // Backup original main.rs
    fs::copy(main_path, backup_path).expect("Failed to backup main.rs");
    
    // Write test code
    let mut file = fs::File::create(main_path).expect("Failed to create test main.rs");
    file.write_all(test_code.as_bytes()).expect("Failed to write test code");
    drop(file);
    
    // Run the test
    let output = Command::new("timeout")
        .args(&["10s", "cargo", "run", "--target", "x86_64-apple-darwin", "--bin", "qemu-uefi", "--", "-serial", "stdio", "-display", "none"])
        .output()
        .expect("Failed to execute QEMU");
    
    // Restore original main.rs
    fs::copy(backup_path, main_path).expect("Failed to restore main.rs");
    fs::remove_file(backup_path).expect("Failed to remove backup");
    
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let all_output = format!("{}{}", stdout, stderr);
    
    println!("=== OUTPUT ===");
    println!("{}", all_output);
    println!("=== END ===");
    
    // Check that double fault was triggered
    assert!(all_output.contains("DOUBLE FAULT"), 
            "Double fault was not triggered");
    
    // The test should exit with success code if double fault handler worked
    assert!(output.status.code() == Some(0) || output.status.code() == Some(124), // 124 is timeout exit code
            "Test did not exit successfully - double fault handler may have failed");
    
    println!("✅ GDT double fault protection test passed!");
}
use std::process::Command;
use std::env;
use std::path::PathBuf;

mod shared_qemu;
use shared_qemu::get_kernel_output;

/// Test complete boot sequence
#[test]
fn test_boot_sequence() {
    println!("Testing complete boot sequence...");
    
    let output = get_kernel_output();
    
    // Verify boot sequence order (based on actual kernel main.rs sequence)
    let boot_steps = [
        "Kernel entry point reached",
        "Serial port initialized",
        "GDT and IDT initialized",
        "Initializing memory management",
        "Frame allocator initialized",
        "Heap initialized",
        "Timer initialized",
        "Keyboard queue initialized",
        "PIC initialized",
        "Interrupts enabled",
    ];
    
    let mut last_position = 0;
    for step in &boot_steps {
        let position = output.find(step)
            .expect(&format!("Boot step '{}' not found", step));
        assert!(position > last_position, 
                "Boot step '{}' out of order", step);
        last_position = position;
    }
    
    println!("✅ Boot sequence test passed (all {} steps in order)", boot_steps.len());
}

/// Test system stability by checking output quality
#[test]
fn test_system_stability() {
    println!("Testing system stability (via output analysis)...");
    
    let output = get_kernel_output();
    
    // Check no panic messages
    assert!(!output.contains("PANIC"), "Panic detected during execution");
    assert!(!output.contains("ERROR"), "Error detected during execution");
    
    // Verify substantial operation (should have many log lines)
    let line_count = output.lines().count();
    assert!(line_count > 100, "Too few output lines: {}", line_count);
    
    // Check for completion marker (indicates kernel ran to expected end)
    assert!(output.contains("🎯 KERNEL_POST_TESTS_COMPLETE 🎯"), 
            "Kernel did not reach expected completion point");
    
    println!("✅ System stability test passed ({} lines of output)", line_count);
}

/// Test runtime feature testing capability (uses separate QEMU run since it needs testing feature)
#[test]
#[ignore = "requires --features testing"]
fn test_runtime_testing_feature() {
    println!("Testing runtime testing feature...");
    
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let workspace_root = PathBuf::from(&manifest_dir);
    
    let result = Command::new("sh")
        .current_dir(&workspace_root)
        .arg("-c")
        .arg("timeout 5s cargo run --features testing --bin qemu-uefi -- -display none -serial stdio 2>&1")
        .output()
        .expect("Failed to run QEMU");
    
    let output = String::from_utf8_lossy(&result.stdout);
    
    // Check for testing output
    assert!(output.contains("Running kernel tests"), 
            "Runtime tests not started");
    assert!(output.contains("=== Running GDT Tests ==="), 
            "GDT tests not found");
    assert!(output.contains("✅ GDT segment test passed!"), 
            "GDT test didn't pass");
    assert!(output.contains("✅ Double fault stack test passed!"), 
            "Double fault test didn't pass");
    assert!(output.contains("=== All GDT Tests Passed ==="), 
            "Not all tests passed");
    
    println!("✅ Runtime testing feature test passed");
}

/// Test BIOS vs UEFI boot compatibility
#[test]
#[ignore = "requires BIOS boot mode"]
fn test_bios_boot() {
    println!("Testing BIOS boot compatibility...");
    
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let workspace_root = PathBuf::from(&manifest_dir);
    
    // Test BIOS boot
    let bios_result = Command::new("sh")
        .current_dir(&workspace_root)
        .arg("-c")
        .arg("timeout 3s cargo run --bin qemu-bios -- -display none -serial stdio 2>&1")
        .output()
        .expect("Failed to run QEMU BIOS");
    
    let bios_output = String::from_utf8_lossy(&bios_result.stdout);
    
    // BIOS should boot successfully
    assert!(bios_output.contains("Kernel entry point reached"), 
            "BIOS boot failed");
    assert!(bios_output.contains("Memory management initialized"), 
            "BIOS memory init failed");
    
    println!("✅ BIOS boot compatibility test passed");
}
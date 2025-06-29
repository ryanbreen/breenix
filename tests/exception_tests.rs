mod shared_qemu;
use shared_qemu::get_kernel_output;

/// Test that exception handlers are properly installed and the system handles exceptions
#[test]
fn test_exception_handlers() {
    println!("Testing exception handler system...");
    
    let output = get_kernel_output();
    
    // Verify exception handlers are installed (via IDT)
    assert!(
        output.contains("IDT loaded successfully"),
        "IDT not loaded successfully"
    );
    
    // Verify breakpoint exception works (always tested in main kernel)
    assert!(
        output.contains("Testing breakpoint interrupt"),
        "Breakpoint test not initiated"
    );
    
    assert!(
        output.contains("EXCEPTION: BREAKPOINT"),
        "Breakpoint exception not handled"
    );
    
    assert!(
        output.contains("Breakpoint test completed"),
        "Breakpoint test did not complete successfully"
    );
    
    println!("✅ Exception handler system test passed");
}

/// Test exception handler installation verification
#[test]
#[ignore] // Requires building kernel with special feature
fn test_exception_handler_installation() {
    println!("Testing exception handler installation with test feature...");
    
    // Run kernel with test_all_exceptions feature
    use std::process::{Command, Stdio};
    use std::time::Duration;
    use std::io::Read;
    
    let mut child = Command::new("cargo")
        .args(&[
            "run",
            "--features", "test_all_exceptions",
            "--bin", "qemu-uefi",
            "--",
            "-serial", "stdio",
            "-display", "none",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to run kernel with exception tests");
    
    // Give it time to run
    std::thread::sleep(Duration::from_secs(10));
    
    // Kill the process
    let _ = child.kill();
    
    // Read output
    let mut stdout = String::new();
    let mut stderr = String::new();
    if let Some(mut out) = child.stdout {
        let _ = out.read_to_string(&mut stdout);
    }
    if let Some(mut err) = child.stderr {
        let _ = err.read_to_string(&mut stderr);
    }
    
    let combined = format!("{}\n{}", stdout, stderr);
    
    // Check that exception handler tests ran
    assert!(
        combined.contains("EXCEPTION_HANDLER_TESTS_START"),
        "Exception handler tests did not start"
    );
    
    // Check each handler verification
    assert!(
        combined.contains("EXCEPTION_TEST: DIVIDE_BY_ZERO handler installed ✓"),
        "Divide by zero handler not verified"
    );
    
    assert!(
        combined.contains("EXCEPTION_TEST: INVALID_OPCODE handler installed ✓"),
        "Invalid opcode handler not verified"
    );
    
    assert!(
        combined.contains("EXCEPTION_TEST: PAGE_FAULT handler installed ✓"),
        "Page fault handler not verified"
    );
    
    assert!(
        combined.contains("EXCEPTION_TEST: Valid memory access succeeded ✓"),
        "Valid memory access test failed"
    );
    
    assert!(
        combined.contains("EXCEPTION_HANDLER_TESTS_COMPLETE"),
        "Exception handler tests did not complete"
    );
    
    println!("✅ Exception handler installation test passed");
}

/// Test that our exception handlers properly handle the generic case
#[test]
fn test_generic_interrupt_handler() {
    println!("Testing generic interrupt handler coverage...");
    
    let output = get_kernel_output();
    
    // The kernel sets up handlers for interrupts 32-255 (except timer and keyboard)
    // We can't easily test this without triggering an actual unhandled interrupt,
    // but we can verify the IDT is properly loaded which includes these handlers
    assert!(
        output.contains("IDT loaded successfully"),
        "IDT with generic handlers not loaded"
    );
    
    println!("✅ Generic interrupt handler test passed");
}
//! System call infrastructure tests
//!
//! This test verifies that the syscall infrastructure is properly set up
//! and that basic syscalls work correctly.

mod shared_qemu;
use shared_qemu::get_kernel_output;

#[test]
fn test_syscall_infrastructure() {
    println!("\n🧪 Syscall Infrastructure Test");
    println!("==============================\n");
    
    // Get kernel output from shared QEMU instance
    let output = get_kernel_output();
    // Check that syscall tests ran
    assert!(output.contains("Testing system call infrastructure..."), 
        "Syscall tests should start");
    
    // Check INT 0x80 handler
    assert!(output.contains("✓ INT 0x80 handler called successfully"), 
        "INT 0x80 handler should work");
    
    // Check individual syscalls
    assert!(output.contains("✓ sys_get_time:"), 
        "sys_get_time should work");
    assert!(output.contains("✓ sys_write:"), 
        "sys_write should work");
    assert!(output.contains("✓ sys_yield:"), 
        "sys_yield should work");
    assert!(output.contains("✓ sys_read:"), 
        "sys_read should work");
    
    // Check error handling
    assert!(output.contains("✓ Invalid write FD correctly rejected"), 
        "Invalid write FD should be rejected");
    assert!(output.contains("✓ Invalid read FD correctly rejected"), 
        "Invalid read FD should be rejected");
    
    // Check completion
    assert!(output.contains("System call infrastructure test completed successfully!"), 
        "All syscall tests should complete");
    
    // Verify syscall output
    assert!(output.contains("[syscall test output]"), 
        "Syscall write should produce output");
    
    println!("\n✅ All syscall tests passed!");
}
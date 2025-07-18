mod shared_qemu;
use shared_qemu::get_kernel_output;

/// Simple test that the kernel builds and runs
#[test]
fn test_kernel_runs() {
    println!("Testing kernel execution...");
    
    // Get kernel output from shared QEMU instance
    let output = get_kernel_output();
    
    // Basic checks
    assert!(!output.is_empty(), "No output from kernel");
    assert!(output.contains("[ INFO]") || output.contains("entry") || output.contains("Kernel"), 
            "Expected kernel output not found");
    
    println!("✅ Kernel runs successfully");
}
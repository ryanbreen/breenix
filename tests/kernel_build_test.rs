use std::process::Command;
use std::env;
use std::path::PathBuf;

mod shared_qemu;
use shared_qemu::get_kernel_output;

/// Test that the kernel builds successfully with our custom target
#[test]
fn test_kernel_builds() {
    println!("Testing kernel build with custom target...");
    
    // Get the workspace root directory
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let workspace_root = PathBuf::from(&manifest_dir);
    let kernel_dir = workspace_root.join("kernel");
    let target_json = workspace_root.join("x86_64-breenix.json");
    
    // Build just the kernel crate with the custom target
    // We need to use nightly and build-std for custom targets
    let result = Command::new("cargo")
        .current_dir(&kernel_dir)
        .args(&[
            "+nightly",
            "build",
            "-Zbuild-std=core,compiler_builtins,alloc",
            "-Zbuild-std-features=compiler-builtins-mem",
            "--target",
            target_json.to_str().unwrap(),
            "--bin",
            "kernel"
        ])
        .output()
        .expect("Failed to run cargo build");
    
    if !result.status.success() {
        eprintln!("Build failed with stderr:");
        eprintln!("{}", String::from_utf8_lossy(&result.stderr));
    }
    
    assert!(result.status.success(), "Kernel build failed");
    println!("✅ Kernel builds successfully with custom target");
}

#[test]
fn test_kernel_builds_with_testing_feature() {
    println!("Testing kernel build with testing feature...");
    
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let workspace_root = PathBuf::from(&manifest_dir);
    let kernel_dir = workspace_root.join("kernel");
    let target_json = workspace_root.join("x86_64-breenix.json");
    
    let result = Command::new("cargo")
        .current_dir(&kernel_dir)
        .args(&[
            "+nightly",
            "build",
            "-Zbuild-std=core,compiler_builtins,alloc",
            "-Zbuild-std-features=compiler-builtins-mem",
            "--target",
            target_json.to_str().unwrap(),
            "--features",
            "testing",
            "--bin",
            "kernel"
        ])
        .output()
        .expect("Failed to run cargo build");
    
    if !result.status.success() {
        eprintln!("Build failed with stderr:");
        eprintln!("{}", String::from_utf8_lossy(&result.stderr));
    }
    
    assert!(result.status.success(), "Kernel build with testing feature failed");
    println!("✅ Kernel builds successfully with testing feature");
}

/// Test that we can run the kernel binary in QEMU using shared infrastructure
#[test]
fn test_kernel_runs_in_qemu() {
    println!("Testing kernel execution in QEMU...");
    
    // Get kernel output from shared QEMU instance
    let output = get_kernel_output();
    
    println!("=== QEMU Output Sample ===");
    for line in output.lines().take(20) {
        println!("{}", line);
    }
    println!("=== End Output Sample ===");
    
    // Look for any sign the kernel is running
    assert!(
        output.contains("Kernel entry point reached") || 
        output.contains("Serial port initialized") ||
        output.contains("[ INFO]") ||
        output.contains("Memory") ||
        output.contains("GDT") ||
        output.contains("Booting"),
        "Expected kernel output not found"
    );
    
    println!("✅ Kernel runs successfully in QEMU");
}
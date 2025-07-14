//! Breenix xtask - Build orchestration for integration tests
//!
//! This crate provides utilities for building the kernel with specific features
//! and running QEMU tests. It's designed to be used by integration tests to
//! avoid architecture conflicts when running `cargo test`.

use std::time::Duration;

mod build;
mod qemu;
mod test;

pub use build::*;
pub use qemu::*;
pub use test::*;

/// Helper function for simple kernel boot test (maintains API compatibility)
pub fn test_kernel_boots() {
    println!("🧪 Testing kernel boot using xtask infrastructure");
    
    // Build kernel with testing features
    let kernel_bin = build_kernel(&["testing"], false)
        .expect("Failed to build kernel");
    
    // Run QEMU with reasonable timeout
    let outcome = run_qemu(&kernel_bin, Duration::from_secs(15))
        .expect("Failed to run QEMU");
    
    // Basic checks
    assert!(outcome.serial_output.contains("Kernel entry point reached"), 
           "Kernel entry point not found in output");
    
    println!("✅ Kernel boot test passed!");
}
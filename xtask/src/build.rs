//! Kernel building functionality

use anyhow::{Context, Result};
use std::path::PathBuf;
use std::process::Command;

/// Build the kernel with specified features
/// Returns the path to the built kernel binary (not disk image)
pub fn build_kernel(features: &[&str], release: bool) -> Result<PathBuf> {
    let workspace_root = find_workspace_root()?;
    
    println!("🔨 Building kernel with features: {:?}", features);
    
    // Prepare feature flags
    let mut feature_list = vec!["testing"]; // Always include testing
    for feature in features {
        if *feature != "testing" {
            feature_list.push(feature);
        }
    }
    let features_arg = feature_list.join(",");
    
    // Build kernel
    let target_file = workspace_root.join("x86_64-breenix.json");
    let mut cmd = Command::new("cargo");
    cmd.current_dir(&workspace_root)
        .args(&[
            "+nightly",
            "build",
            "-p", "kernel", 
            "--target", &target_file.to_string_lossy(),
            "-Z", "build-std=core,alloc"
        ]);
    
    if release {
        cmd.arg("--release");
    }
    
    if !features_arg.is_empty() {
        cmd.args(&["--features", &features_arg]);
    }
    
    let output = cmd.output()
        .context("Failed to execute cargo build")?;
    
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Kernel build failed:\n{}", stderr);
    }
    
    // Find the built kernel binary
    let profile = if release { "release" } else { "debug" };
    let kernel_bin = workspace_root
        .join("target")
        .join("x86_64-breenix")
        .join(profile)
        .join("kernel");
    
    if !kernel_bin.exists() {
        anyhow::bail!("Kernel binary not found at: {}", kernel_bin.display());
    }
    
    println!("✅ Kernel built: {}", kernel_bin.display());
    
    // Debug: Check file timestamp
    if let Ok(metadata) = kernel_bin.metadata() {
        if let Ok(modified) = metadata.modified() {
            println!("🕐 Kernel modified: {:?}", modified);
        }
    }
    
    Ok(kernel_bin)
}

/// Find the workspace root by looking for Cargo.toml with [workspace]
fn find_workspace_root() -> Result<PathBuf> {
    let mut current = std::env::current_dir()
        .context("Failed to get current directory")?;
    
    loop {
        let cargo_toml = current.join("Cargo.toml");
        if cargo_toml.exists() {
            let content = std::fs::read_to_string(&cargo_toml)
                .context("Failed to read Cargo.toml")?;
            if content.contains("[workspace]") {
                return Ok(current);
            }
        }
        
        match current.parent() {
            Some(parent) => current = parent.to_path_buf(),
            None => anyhow::bail!("Could not find workspace root"),
        }
    }
}
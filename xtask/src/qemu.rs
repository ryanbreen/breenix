//! QEMU execution and output capture

use anyhow::{Context, Result};
use bootloader::DiskImageBuilder;
use ovmf_prebuilt::{Arch, FileType, Prebuilt, Source};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use std::fs;

/// Result of running QEMU
#[derive(Debug)]
pub struct QemuOutcome {
    pub exit_code: Option<i32>,
    pub serial_output: String,
    pub duration: Duration,
}

/// Run QEMU with the given kernel binary and capture serial output
pub fn run_qemu(kernel_bin: &Path, timeout: Duration) -> Result<QemuOutcome> {
    let start_time = Instant::now();
    
    // Create temporary file for serial output
    let temp_dir = std::env::temp_dir();
    let serial_file = temp_dir.join(format!("breenix_test_{}.log", std::process::id()));
    
    println!("🚀 Creating disk image from kernel: {}", kernel_bin.display());
    
    // Debug: Check kernel timestamp
    if let Ok(metadata) = kernel_bin.metadata() {
        if let Ok(modified) = metadata.modified() {
            println!("🕐 Using kernel modified: {:?}", modified);
        }
    }
    
    // Create disk image using bootloader crate
    let disk_builder = DiskImageBuilder::new(PathBuf::from(kernel_bin));
    let disk_image = temp_dir.join(format!("breenix_test_{}.img", std::process::id()));
    
    disk_builder.create_uefi_image(&disk_image)
        .context("Failed to create UEFI disk image")?;
    
    // Debug: Print exact disk image path for comparison
    println!("🔍 DISK IMAGE PATH: {}", disk_image.display());
    
    println!("🚀 Starting QEMU (timeout: {:?})", timeout);
    println!("💿 Disk image: {}", disk_image.display());
    println!("📄 Serial output: {}", serial_file.display());
    
    // Use ovmf-prebuilt for proper UEFI setup (like the working qemu-uefi.rs)
    let prebuilt = Prebuilt::fetch(Source::LATEST, &temp_dir.join("ovmf"))
        .context("Failed to fetch OVMF prebuilt")?;
    let ovmf_code = prebuilt.get_file(Arch::X64, FileType::Code);
    let ovmf_vars = prebuilt.get_file(Arch::X64, FileType::Vars);
    
    let mut qemu_cmd = Command::new("qemu-system-x86_64");
    qemu_cmd.args(&[
        "-drive", &format!("format=raw,file={}", disk_image.display()),
        "-drive", &format!("format=raw,if=pflash,readonly=on,file={}", ovmf_code.display()),
        "-drive", &format!("format=raw,if=pflash,file={}", ovmf_vars.display()),
        "-display", "none",
        "-serial", &format!("file:{}", serial_file.display()),
        "-no-reboot",
        "-no-shutdown"
    ]);
    
    let mut child = qemu_cmd
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("Failed to start QEMU")?;
    
    // Wait for completion or timeout
    let mut exit_code = None;
    let start = Instant::now();
    
    while start.elapsed() < timeout {
        match child.try_wait()? {
            Some(status) => {
                exit_code = status.code();
                break;
            }
            None => {
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
    
    // If still running, kill it
    if exit_code.is_none() {
        let _ = child.kill();
        let _ = child.wait();
        println!("⚠️  QEMU timed out after {:?}", timeout);
    }
    
    // Read serial output
    let serial_output = if serial_file.exists() {
        // Give filesystem a moment to flush
        std::thread::sleep(Duration::from_millis(200));
        match fs::read_to_string(&serial_file) {
            Ok(content) => content,
            Err(e) => {
                println!("⚠️  Failed to read serial output: {}", e);
                String::new()
            }
        }
    } else {
        println!("⚠️  Serial output file not found");
        String::new()
    };
    
    // Cleanup
    let _ = fs::remove_file(&serial_file);
    let _ = fs::remove_file(&disk_image);
    
    let duration = start_time.elapsed();
    println!("⏱️  QEMU ran for {:?}", duration);
    
    Ok(QemuOutcome {
        exit_code,
        serial_output,
        duration,
    })
}

/// Assert that a test marker is present in QEMU output
pub fn assert_marker(outcome: &QemuOutcome, marker: &str) {
    let full_marker = format!("TEST_MARKER:{}", marker);
    
    if outcome.serial_output.contains(&full_marker) {
        println!("✅ Found test marker: {}", marker);
    } else {
        println!("❌ Test marker NOT found: {}", marker);
        println!("📄 Serial output (last 1000 chars):");
        let output_tail = if outcome.serial_output.len() > 1000 {
            &outcome.serial_output[outcome.serial_output.len() - 1000..]
        } else {
            &outcome.serial_output
        };
        println!("{}", output_tail);
        
        panic!("Test failed: marker '{}' not found in QEMU output", marker);
    }
}
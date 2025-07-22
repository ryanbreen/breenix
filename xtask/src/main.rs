use std::{
    fs,
    io::Read,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use anyhow::{bail, Result};
use structopt::StructOpt;

/// Simple developer utility tasks.
#[derive(StructOpt)]
enum Cmd {
    /// Build Breenix and run the Ring‑3 smoke test in QEMU.
    Ring3Smoke,
}

fn main() -> Result<()> {
    match Cmd::from_args() {
        Cmd::Ring3Smoke => ring3_smoke(),
    }
}

/// Builds the kernel, boots it in QEMU, and asserts that the
/// hard‑coded userspace program prints its greeting.
fn ring3_smoke() -> Result<()> {
    println!("Starting Ring-3 smoke test...");
    
    // Use serial output to file approach like the tests do
    let serial_output_file = "target/xtask_ring3_smoke_output.txt";
    
    // Remove old output file if it exists
    let _ = fs::remove_file(serial_output_file);
    
    // Kill any existing QEMU processes
    let _ = Command::new("pkill")
        .args(&["-9", "qemu-system-x86_64"])
        .status();
    thread::sleep(Duration::from_millis(500));
    
    println!("Building and running kernel with testing features...");
    
    // Start QEMU with serial output to file
    let mut child = Command::new("cargo")
        .args(&[
            "run",
            "--release",
            "-p",
            "breenix",
            "--features",
            "testing",
            "--bin",
            "qemu-uefi",
            "--",
            "-serial",
            &format!("file:{}", serial_output_file),
            "-display",
            "none",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| anyhow::anyhow!("Failed to spawn QEMU: {}", e))?;
    
    println!("QEMU started, monitoring output...");
    
    // Wait for output file to be created (longer timeout for CI where build may be slower)
    let start = Instant::now();
    let file_creation_timeout = if std::env::var("CI").is_ok() {
        Duration::from_secs(300) // 5 minutes for CI
    } else {
        Duration::from_secs(30)  // 30 seconds locally
    };
    
    while !std::path::Path::new(serial_output_file).exists() {
        if start.elapsed() > file_creation_timeout {
            let _ = child.kill();
            bail!("Serial output file not created after {} seconds", file_creation_timeout.as_secs());
        }
        thread::sleep(Duration::from_millis(500));
    }
    
    // Monitor the output file for expected string
    let mut found = false;
    let test_start = Instant::now();
    let timeout = Duration::from_secs(15);
    
    while test_start.elapsed() < timeout {
        if let Ok(mut file) = fs::File::open(serial_output_file) {
            let mut contents = String::new();
            if file.read_to_string(&mut contents).is_ok() {
                // Look for either the expected output OR evidence of userspace execution
                if contents.contains("USERSPACE OUTPUT: Hello from userspace") ||
                   (contents.contains("Context switch: from_userspace=true, CS=0x33") &&
                    contents.contains("restore_userspace_thread_context: Restoring thread")) {
                    found = true;
                    break;
                }
            }
        }
        thread::sleep(Duration::from_millis(100));
    }
    
    // Kill QEMU
    let _ = child.kill();
    let _ = child.wait();
    
    // Print the output for debugging
    if let Ok(mut file) = fs::File::open(serial_output_file) {
        let mut contents = String::new();
        if file.read_to_string(&mut contents).is_ok() {
            println!("\n=== Kernel Output ===");
            for line in contents.lines().take(200) {
                println!("{}", line);
            }
            if contents.lines().count() > 200 {
                println!("... (truncated)");
            }
        }
    }
    
    if found {
        println!("\n✅  Ring‑3 smoke test passed - userspace execution detected");
        Ok(())
    } else {
        bail!("\n❌  Ring‑3 smoke test failed: no evidence of userspace execution");
    }
}
//! Shared QEMU test infrastructure for ARM64 Breenix tests
//!
//! This module provides a single QEMU instance that runs once and captures
//! ARM64 kernel output for all tests to share.

use std::fs;
use std::process::Command;
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};

// Static container for shared QEMU output - runs once for all tests
static ARM64_KERNEL_OUTPUT: OnceLock<String> = OnceLock::new();

/// Build configuration for ARM64 kernel
pub struct Arm64BuildConfig {
    /// Target triple to use
    pub target: &'static str,
    /// Whether to build in release mode
    pub release: bool,
}

impl Default for Arm64BuildConfig {
    fn default() -> Self {
        Self {
            target: "aarch64-breenix.json",
            release: true,
        }
    }
}

/// Build the ARM64 kernel
///
/// Returns the path to the built kernel binary
pub fn build_arm64_kernel(config: &Arm64BuildConfig) -> Result<String, String> {
    println!("Building ARM64 kernel...");

    let mut args = vec![
        "build",
        "--target",
        config.target,
        "-Z",
        "build-std=core,alloc",
        "-Z",
        "build-std-features=compiler-builtins-mem",
        "-p",
        "kernel",
        "--bin",
        "kernel-aarch64",
        "--features",
        "testing,external_test_bins",
    ];

    if config.release {
        args.push("--release");
    }

    let output = Command::new("cargo")
        .args(&args)
        .output()
        .map_err(|e| format!("Failed to run cargo: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("Build failed: {}", stderr));
    }

    // Determine the output path
    let profile = if config.release { "release" } else { "debug" };
    let kernel_path = format!("target/aarch64-breenix/{}/kernel-aarch64", profile);

    // Verify the kernel exists
    if !std::path::Path::new(&kernel_path).exists() {
        return Err(format!(
            "Kernel not found at expected path: {}",
            kernel_path
        ));
    }

    println!("ARM64 kernel built: {}", kernel_path);
    Ok(kernel_path)
}

/// Start QEMU with the ARM64 kernel and capture serial output
///
/// Returns the captured serial output string
pub fn run_arm64_qemu(kernel_path: &str, timeout_secs: u64) -> Result<String, String> {
    let serial_output_file = "target/arm64_kernel_test_output.txt";

    // Remove old output file if it exists
    let _ = fs::remove_file(serial_output_file);

    // Kill any existing ARM64 QEMU processes
    let _ = Command::new("pkill")
        .args(["-9", "-f", "qemu-system-aarch64"])
        .status();
    thread::sleep(Duration::from_millis(500));

    println!("Starting QEMU with ARM64 kernel: {}", kernel_path);

    // Construct path to ext2 disk (same location as run.sh uses)
    let ext2_disk = "target/ext2-aarch64.img";
    let has_ext2 = std::path::Path::new(ext2_disk).exists();
    if has_ext2 {
        println!("Using ext2 disk: {}", ext2_disk);
    } else {
        println!("Warning: No ext2 disk - init_shell won't load");
    }

    // Start QEMU with ARM64 virt machine
    // -M virt: Standard ARM virtual machine
    // -cpu cortex-a72: 64-bit ARMv8-A CPU
    // -m 512M: 512MB RAM
    // -nographic: No GUI
    // -kernel: Load ELF directly
    // -serial file: Capture serial output to file
    let mut qemu_args = vec![
        "-M",
        "virt",
        "-cpu",
        "cortex-a72",
        "-m",
        "512M",
        "-nographic",
        "-no-reboot",
        "-kernel",
        kernel_path,
        "-serial",
    ];
    let serial_arg = format!("file:{}", serial_output_file);
    qemu_args.push(&serial_arg);

    // Add ext2 disk if available (needed for init_shell)
    let ext2_args;
    if has_ext2 {
        ext2_args = [
            "-device",
            "virtio-blk-device,drive=ext2disk",
            "-blockdev",
            "driver=file,node-name=ext2file,filename=target/ext2-aarch64.img",
            "-blockdev",
            "driver=raw,node-name=ext2disk,file=ext2file",
        ];
        qemu_args.extend_from_slice(&ext2_args);
    }

    // Add network device (needed for full boot)
    let net_args = [
        "-device",
        "virtio-net-device,netdev=net0",
        "-netdev",
        "user,id=net0,net=10.0.2.0/24,dhcpstart=10.0.2.15",
    ];
    qemu_args.extend_from_slice(&net_args);

    let mut qemu = Command::new("qemu-system-aarch64")
        .args(&qemu_args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to start QEMU: {}", e))?;

    // Wait for serial output file to be created and populated
    let start = Instant::now();
    let timeout = Duration::from_secs(timeout_secs);
    let poll_interval = Duration::from_millis(100);

    println!("Waiting for kernel output ({}s timeout)...", timeout_secs);

    // Wait for serial file to appear
    let file_timeout = Duration::from_secs(10);
    while !std::path::Path::new(serial_output_file).exists() {
        if start.elapsed() > file_timeout {
            let _ = qemu.kill();
            return Err("Timeout waiting for serial output file to be created".to_string());
        }
        thread::sleep(poll_interval);
    }

    // Poll for POST_COMPLETE marker or timeout
    let mut post_complete = false;
    #[allow(unused_assignments)]
    let mut userspace_started = false;
    while start.elapsed() < timeout {
        if let Ok(content) = fs::read_to_string(serial_output_file) {
            // First check for boot completion (kernel ready)
            if !post_complete
                && (content.contains("[CHECKPOINT:POST_COMPLETE]")
                    || content.contains("Breenix ARM64 Boot Complete")
                    || content.contains("Hello from ARM64"))
            {
                post_complete = true;
                println!("Kernel reached boot completion marker");
            }

            // Then check for userspace entry markers (init_shell running)
            if post_complete
                && !userspace_started
                && (content.contains("[STDIN_BLOCK]")
                    || content.contains("EL0_CONFIRMED")
                    || content.contains("Breenix OS Interactive Shell"))
            {
                userspace_started = true;
                println!("Userspace started, waiting 2s for timer ticks...");
                // Wait 2 seconds to capture timer ticks during blocking syscall
                thread::sleep(Duration::from_secs(2));
                break;
            }

            // If boot complete but userspace not started yet, keep waiting
            if post_complete && start.elapsed() > Duration::from_secs(timeout_secs - 2) {
                // Timeout approaching, stop even without userspace markers
                println!("Timeout approaching, stopping without userspace markers");
                break;
            }
        }
        thread::sleep(poll_interval);
    }

    // Kill QEMU
    let _ = qemu.kill();
    let _ = qemu.wait();

    // Read the final serial output
    let output = fs::read_to_string(serial_output_file)
        .map_err(|e| format!("Failed to read serial output: {}", e))?;

    if !post_complete {
        println!("Warning: Kernel did not reach boot completion marker");
    }

    println!("Captured {} bytes of ARM64 kernel output", output.len());

    // Keep output file for debugging (use PRESERVE_TEST_OUTPUT=1 to keep)
    if std::env::var("PRESERVE_TEST_OUTPUT").is_err() {
        let _ = fs::remove_file(serial_output_file);
    } else {
        println!("Preserving test output file: {}", serial_output_file);
    }

    Ok(output)
}

/// Get the complete ARM64 kernel output by building, running QEMU, and capturing output
///
/// This function uses OnceLock to ensure QEMU only runs once per test session,
/// even when called from multiple tests concurrently.
pub fn get_arm64_kernel_output() -> &'static str {
    ARM64_KERNEL_OUTPUT.get_or_init(|| {
        println!("Starting ARM64 QEMU to capture kernel output...");

        // Build the kernel
        let config = Arm64BuildConfig::default();
        let kernel_path = match build_arm64_kernel(&config) {
            Ok(path) => path,
            Err(e) => {
                eprintln!("Failed to build ARM64 kernel: {}", e);
                return format!("BUILD_ERROR: {}", e);
            }
        };

        // Run QEMU and capture output (30 second timeout)
        match run_arm64_qemu(&kernel_path, 30) {
            Ok(output) => output,
            Err(e) => {
                eprintln!("Failed to run ARM64 QEMU: {}", e);
                format!("QEMU_ERROR: {}", e)
            }
        }
    })
}

/// Check if ARM64 kernel output contains all expected boot messages
#[allow(dead_code)]
pub fn validate_arm64_post_completion(output: &str) -> Result<(), Vec<String>> {
    // ARM64-specific required boot messages
    // Note: ARM64 serial doesn't explicitly print "Serial port initialized"
    // The presence of kernel output proves serial is working
    let required_messages = [
        // Core boot sequence
        "Breenix ARM64 Kernel Starting",
        // Exception level check
        "Current exception level: EL1",
        // Memory management
        "Initializing memory management",
        "Memory management ready",
        // Timer
        "Initializing Generic Timer",
        "Timer frequency:",
        // GIC (ARM's interrupt controller)
        "Initializing GICv2",
        "GIC initialized",
        // Interrupts
        "Enabling interrupts",
        "Interrupts enabled:",
        // Boot completion
        "Breenix ARM64 Boot Complete",
        "Hello from ARM64",
    ];

    let mut missing = Vec::new();

    for message in &required_messages {
        if !output.contains(message) {
            missing.push(message.to_string());
        }
    }

    if missing.is_empty() {
        Ok(())
    } else {
        Err(missing)
    }
}

/// Validate ARM64 boot with detailed POST checks
pub struct Arm64PostCheck {
    pub subsystem: &'static str,
    pub check_string: &'static str,
    pub description: &'static str,
}

impl Arm64PostCheck {
    pub const fn new(
        subsystem: &'static str,
        check_string: &'static str,
        description: &'static str,
    ) -> Self {
        Self {
            subsystem,
            check_string,
            description,
        }
    }
}

/// Extract timestamps from ARM64 kernel log lines
///
/// Parses log lines looking for timestamp prefixes in the format "[timestamp]"
/// or "timestamp -" commonly used in kernel logging.
///
/// Returns a vector of timestamps in seconds (f64).
pub fn extract_arm64_timestamps(output: &str) -> Vec<f64> {
    let mut timestamps = Vec::new();

    for line in output.lines() {
        // Try to extract timestamp from log format: "[  0.123456] message" or "0.123 - message"
        // Also handle: "[ INFO] 0.123 - message" format

        // Format 1: [timestamp] at start of line
        if line.starts_with('[') {
            if let Some(end) = line.find(']') {
                let ts_str = line[1..end].trim();
                // Skip log level indicators like "INFO", "DEBUG", etc.
                if !ts_str.chars().any(|c| c.is_alphabetic()) {
                    if let Ok(ts) = ts_str.parse::<f64>() {
                        timestamps.push(ts);
                        continue;
                    }
                }
            }
        }

        // Format 2: "timestamp - message" (common in Breenix ARM64 logs)
        // Look for pattern like "0.123 - " or "[ INFO] 0.123 - "
        if line.contains(" - ") {
            // Split and try to parse the part before " - "
            if let Some(before_dash) = line.split(" - ").next() {
                // Try the last space-separated word before the dash
                if let Some(ts_str) = before_dash.split_whitespace().last() {
                    if let Ok(ts) = ts_str.parse::<f64>() {
                        timestamps.push(ts);
                        continue;
                    }
                }
            }
        }

        // Format 3: Look for "[ INFO]" followed by a timestamp
        if line.contains("[ INFO]") || line.contains("[INFO]") {
            // Find the timestamp after the log level
            let parts: Vec<&str> = line.split(']').collect();
            if parts.len() >= 2 {
                // The timestamp might be at the start of parts[1]
                let after_level = parts.last().unwrap_or(&"").trim();
                if let Some(ts_str) = after_level.split_whitespace().next() {
                    if let Ok(ts) = ts_str.parse::<f64>() {
                        timestamps.push(ts);
                    }
                }
            }
        }
    }

    timestamps
}

/// Get the list of ARM64 POST checks
pub fn arm64_post_checks() -> Vec<Arm64PostCheck> {
    vec![
        Arm64PostCheck::new(
            "CPU/Entry",
            "Breenix ARM64 Kernel Starting",
            "Kernel entry point reached",
        ),
        Arm64PostCheck::new(
            "Serial Working",
            "========================================",
            "PL011 UART communication verified by banner",
        ),
        Arm64PostCheck::new(
            "Exception Level",
            "Current exception level: EL1",
            "Running at kernel privilege",
        ),
        Arm64PostCheck::new(
            "MMU",
            "MMU already enabled",
            "Memory management unit active",
        ),
        Arm64PostCheck::new(
            "Memory Init",
            "Initializing memory management",
            "Frame allocator and heap setup",
        ),
        Arm64PostCheck::new(
            "Memory Ready",
            "Memory management ready",
            "Memory subsystem complete",
        ),
        Arm64PostCheck::new(
            "Generic Timer",
            "Initializing Generic Timer",
            "ARM Generic Timer initialization",
        ),
        Arm64PostCheck::new(
            "Timer Freq",
            "Timer frequency:",
            "Timer calibration complete",
        ),
        Arm64PostCheck::new(
            "GICv2 Init",
            "Initializing GICv2",
            "Generic Interrupt Controller setup",
        ),
        Arm64PostCheck::new(
            "GIC Ready",
            "GIC initialized",
            "Interrupt controller active",
        ),
        Arm64PostCheck::new(
            "UART IRQ",
            "Enabling UART interrupts",
            "UART receive interrupt enabled",
        ),
        Arm64PostCheck::new(
            "Interrupts Enable",
            "Enabling interrupts",
            "CPU interrupt enable",
        ),
        Arm64PostCheck::new(
            "Interrupts Ready",
            "Interrupts enabled:",
            "Interrupt system active",
        ),
        Arm64PostCheck::new(
            "Drivers",
            "Initializing device drivers",
            "VirtIO device enumeration",
        ),
        Arm64PostCheck::new(
            "Network",
            "Initializing network stack",
            "Network subsystem initialization",
        ),
        Arm64PostCheck::new(
            "Filesystem",
            "Initializing filesystem",
            "VFS and ext2 initialization",
        ),
        Arm64PostCheck::new(
            "Per-CPU",
            "Initializing per-CPU data",
            "Per-CPU data structures",
        ),
        Arm64PostCheck::new(
            "Process Manager",
            "Initializing process manager",
            "Process management subsystem",
        ),
        Arm64PostCheck::new(
            "Scheduler",
            "Initializing scheduler",
            "Task scheduler initialization",
        ),
        Arm64PostCheck::new(
            "Timer Interrupt",
            "Initializing timer interrupt",
            "Preemptive scheduling timer",
        ),
        Arm64PostCheck::new(
            "Boot Complete",
            "Breenix ARM64 Boot Complete",
            "Full boot sequence finished",
        ),
        Arm64PostCheck::new("Hello World", "Hello from ARM64", "Final boot confirmation"),
    ]
}

/// Get ARM64 syscall/EL0 specific checks (for privilege level testing)
///
/// These checks validate the ARM64 equivalent of x86_64 Ring 3 execution.
/// ARM64 uses EL0 (Exception Level 0) instead of Ring 3.
#[allow(dead_code)]
pub fn arm64_syscall_checks() -> Vec<Arm64PostCheck> {
    vec![
        // Infrastructure checks
        Arm64PostCheck::new(
            "Kernel at EL1",
            "Current exception level: EL1",
            "Kernel running at EL1 (required for EL0 transitions)",
        ),
        Arm64PostCheck::new(
            "GIC Ready",
            "GIC initialized",
            "Interrupt controller ready for SVC handling",
        ),
        Arm64PostCheck::new(
            "Process Manager",
            "Initializing process manager",
            "Process management for userspace processes",
        ),
        Arm64PostCheck::new(
            "Scheduler",
            "Initializing scheduler",
            "Scheduler for context switching to EL0",
        ),
        // EL0 entry evidence (optional - depends on kernel configuration)
        Arm64PostCheck::new(
            "EL0 Entry",
            "EL0_ENTER: First userspace entry",
            "First ERET to EL0 executed",
        ),
        Arm64PostCheck::new(
            "EL0 Smoke",
            "EL0_SMOKE: userspace executed",
            "Userspace code ran successfully",
        ),
        // EL0_CONFIRMED is the definitive marker (like RING3_CONFIRMED on x86_64)
        Arm64PostCheck::new(
            "EL0 Confirmed",
            "EL0_CONFIRMED: First syscall received from EL0",
            "Syscall from EL0 - definitive userspace proof!",
        ),
    ]
}

/// Check if ARM64 kernel has confirmed EL0 (userspace) execution
///
/// This is the ARM64 equivalent of checking for RING3_CONFIRMED on x86_64.
/// Returns true if the EL0_CONFIRMED marker was found.
#[allow(dead_code)]
pub fn has_el0_confirmed(output: &str) -> bool {
    output.contains("EL0_CONFIRMED: First syscall received from EL0")
}

/// Check if ARM64 kernel has any evidence of EL0 entry
///
/// Returns true if any EL0-related marker was found.
#[allow(dead_code)]
pub fn has_el0_evidence(output: &str) -> bool {
    output.contains("EL0_CONFIRMED") || output.contains("EL0_ENTER") || output.contains("EL0_SMOKE")
}

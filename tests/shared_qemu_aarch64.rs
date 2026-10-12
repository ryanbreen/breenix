//! Shared QEMU test infrastructure for ARM64 Breenix tests
//!
//! This module provides a single QEMU instance that runs once and captures
//! ARM64 kernel output for all tests to share.

use std::fs;
use std::process::Command;
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};

#[path = "shared_qemu/process_tree.rs"]
mod process_tree;
#[path = "shared_qemu/signal_handlers.rs"]
mod signal_handlers;
use process_tree::ChildGroup;
use signal_handlers::SignalHandlers;
#[path = "shared_qemu/boot_lock.rs"]
mod boot_lock;
use boot_lock::BootLock;

// Static container for shared QEMU output - runs once for all tests
static ARM64_KERNEL_OUTPUT: OnceLock<Result<String, String>> = OnceLock::new();

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
            target: "aarch64-breenix-kernel.json",
            release: true,
        }
    }
}

/// Build the ARM64 kernel
///
/// Returns the path to the built kernel binary
pub fn build_arm64_kernel(config: &Arm64BuildConfig) -> Result<String, String> {
    println!("Building ARM64 kernel...");
    let _signals = SignalHandlers::install();

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
    ];

    if config.release {
        args.push("--release");
    }

    let log_path =
        std::env::temp_dir().join(format!("breenix-arm64-build-{}.log", std::process::id()));
    let log = fs::File::create(&log_path).map_err(|e| format!("create build log: {e}"))?;
    let mut command = Command::new("cargo");
    command
        .env("CARGO_BUILD_JOBS", "6")
        .args(&args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::from(log));
    let mut build =
        ChildGroup::spawn(&mut command).map_err(|e| format!("Failed to run cargo: {e}"))?;
    let deadline = Instant::now() + Duration::from_secs(900);
    let status = loop {
        if SignalHandlers::interrupted() {
            return Err("ARM64 build interrupted".to_string());
        }
        if let Some(status) = build.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        if Instant::now() >= deadline {
            return Err("ARM64 build timed out after 900 seconds".to_string());
        }
        thread::sleep(Duration::from_millis(100));
    };
    drop(build);
    let diagnostics = fs::read_to_string(&log_path).unwrap_or_default();
    let _ = fs::remove_file(log_path);
    if !status.success() {
        return Err(format!("Build failed: {diagnostics}"));
    }

    // Determine the output path
    let profile = if config.release { "release" } else { "debug" };
    let kernel_path = format!("target/aarch64-breenix-kernel/{}/kernel-aarch64", profile);

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
    let _signals = SignalHandlers::install();
    let mut boot_lock =
        BootLock::acquire("target/shared_arm64_test.lock").map_err(|e| e.to_string())?;
    let serial_output_file = "target/arm64_kernel_test_output.txt";

    // Remove old output file if it exists
    let _ = fs::remove_file(serial_output_file);

    println!("Starting QEMU with ARM64 kernel: {}", kernel_path);

    // Check for ext2 disk with userspace programs
    let ext2_disk = "target/ext2-aarch64.img";
    let has_ext2 = std::path::Path::new(ext2_disk).exists();
    if has_ext2 {
        println!("Using ext2 disk: {}", ext2_disk);
    } else {
        println!("Warning: ext2 disk not found - userspace tests will be limited");
    }

    // Build QEMU arguments
    // -M virt: Standard ARM virtual machine
    // -cpu cortex-a72: 64-bit ARMv8-A CPU
    // -m 512M: 512MB RAM
    // -nographic: No GUI
    // -kernel: Load ELF directly
    // -serial file: Capture serial output to file
    // -device virtio-blk: ext2 disk with userspace programs (if available)
    let serial_arg = format!("file:{}", serial_output_file);
    let ext2_drive_spec = format!("if=none,id=ext2,format=raw,file={}", ext2_disk);

    let mut qemu_cmd = Command::new("qemu-system-aarch64");
    qemu_cmd
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .arg("-M")
        .arg("virt")
        .arg("-cpu")
        .arg("cortex-a72")
        .arg("-m")
        .arg("512M")
        .arg("-nographic")
        .arg("-no-reboot")
        .arg("-kernel")
        .arg(kernel_path)
        .arg("-device")
        .arg("virtio-gpu-device")
        .arg("-device")
        .arg("virtio-keyboard-device")
        .arg("-serial")
        .arg(&serial_arg);

    // Add ext2 disk if available (contains init_shell and other userspace programs)
    if has_ext2 {
        qemu_cmd
            .arg("-device")
            .arg("virtio-blk-device,drive=ext2")
            .arg("-drive")
            .arg(&ext2_drive_spec);
    }

    let mut qemu = boot_lock
        .spawn(&mut qemu_cmd)
        .map_err(|e| format!("Failed to start QEMU: {}", e))?;

    // Wait for serial output file to be created and populated
    let start = Instant::now();
    let timeout = Duration::from_secs(timeout_secs);
    let poll_interval = Duration::from_millis(100);

    println!("Waiting for kernel output ({}s timeout)...", timeout_secs);

    // Wait for serial file to appear
    let file_timeout = Duration::from_secs(10);
    while !std::path::Path::new(serial_output_file).exists() {
        if SignalHandlers::interrupted() {
            return Err("QEMU test interrupted".to_string());
        }
        if let Some(status) = qemu.try_wait().map_err(|e| e.to_string())? {
            return Err(format!("QEMU exited before serial output: {status}"));
        }
        if start.elapsed() > file_timeout {
            return Err("Timeout waiting for serial output file to be created".to_string());
        }
        thread::sleep(poll_interval);
    }

    loop {
        if SignalHandlers::interrupted() {
            return Err("QEMU test interrupted".to_string());
        }
        if arm64_ready(&fs::read_to_string(serial_output_file).map_err(|e| e.to_string())?) {
            break;
        }
        if let Some(status) = qemu.try_wait().map_err(|e| e.to_string())? {
            return Err(format!(
                "QEMU exited before ARM64 boot completion and EL0 syscall: {status}"
            ));
        }
        if start.elapsed() >= timeout {
            return Err(format!(
                "ARM64 kernel did not print its boot completion and EL0 syscall lines: {}",
                fs::read_to_string(serial_output_file).unwrap_or_default()
            ));
        }
        thread::sleep(poll_interval);
    }
    drop(qemu);
    let output = fs::read_to_string(serial_output_file)
        .map_err(|e| format!("Failed to read serial output: {e}"))?;

    println!("Captured {} bytes of ARM64 kernel output", output.len());

    // Clean up output file
    let _ = fs::remove_file(serial_output_file);

    Ok(output)
}

fn arm64_ready(output: &str) -> bool {
    // ARM64 has no POST_COMPLETE checkpoint. Require both of its own complete
    // lines, including the syscall entry's proof that the guest reached EL0.
    let complete = |expected: &str| {
        output
            .split_inclusive('\n')
            .any(|line| line.ends_with('\n') && line.trim() == expected)
    };
    complete("Breenix ARM64 Boot Complete!")
        && complete("EL0_SYSCALL: First syscall from userspace (SPSR confirms EL0)")
}

#[cfg(test)]
mod readiness_tests {
    #[test]
    fn requires_both_producers_complete_lines() {
        let boot = "  Breenix ARM64 Boot Complete!\n";
        let syscall = "EL0_SYSCALL: First syscall from userspace (SPSR confirms EL0)\n";
        assert!(!super::arm64_ready(boot));
        assert!(!super::arm64_ready(syscall));
        assert!(!super::arm64_ready(&format!(
            "{boot}kernel::test_exec: will emit {syscall}"
        )));
        assert!(!super::arm64_ready(&format!(
            "{boot}{}",
            syscall.trim_end()
        )));
        assert!(super::arm64_ready(&format!("{boot}{syscall}")));
    }
}

/// Get the complete ARM64 kernel output by building, running QEMU, and capturing output
///
/// This function uses OnceLock to ensure QEMU only runs once per test session,
/// even when called from multiple tests concurrently.
pub fn get_arm64_kernel_output() -> &'static str {
    let result = ARM64_KERNEL_OUTPUT.get_or_init(|| {
        let config = Arm64BuildConfig::default();
        build_arm64_kernel(&config).and_then(|path| run_arm64_qemu(&path, 300))
    });
    match result {
        Ok(output) => output,
        Err(error) => panic!("ARM64 shared QEMU initialization failed: {error}"),
    }
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
        // EL0_SYSCALL is the definitive marker (like RING3_CONFIRMED on x86_64)
        Arm64PostCheck::new(
            "EL0 Confirmed",
            "EL0_SYSCALL: First syscall from userspace",
            "Syscall from EL0 - definitive userspace proof!",
        ),
    ]
}

/// Check if ARM64 kernel has confirmed EL0 (userspace) execution
///
/// This is the ARM64 equivalent of checking for RING3_CONFIRMED on x86_64.
/// Returns true if the EL0_SYSCALL marker was found.
#[allow(dead_code)]
pub fn has_el0_confirmed(output: &str) -> bool {
    output.contains("EL0_SYSCALL: First syscall from userspace")
}

/// Check if ARM64 kernel has any evidence of EL0 entry
///
/// Returns true if any EL0-related marker was found.
#[allow(dead_code)]
pub fn has_el0_evidence(output: &str) -> bool {
    output.contains("EL0_SYSCALL:") || output.contains("EL0_ENTER") || output.contains("EL0_SMOKE")
}

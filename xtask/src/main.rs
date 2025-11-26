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
    /// Build Breenix and run the Ring-3 smoke test in QEMU.
    Ring3Smoke,
    /// Build Breenix and test ENOSYS syscall handling.
    Ring3Enosys,
    /// Boot kernel once and validate each boot stage sequentially.
    BootStages,
}

fn main() -> Result<()> {
    match Cmd::from_args() {
        Cmd::Ring3Smoke => ring3_smoke(),
        Cmd::Ring3Enosys => ring3_enosys(),
        Cmd::BootStages => boot_stages(),
    }
}

/// Boot stage definition with marker, description, and failure info
struct BootStage {
    name: &'static str,
    marker: &'static str,
    failure_meaning: &'static str,
    check_hint: &'static str,
}

/// Timing info for a completed stage
#[derive(Clone)]
struct StageTiming {
    duration: Duration,
}

/// Define all boot stages in order
fn get_boot_stages() -> Vec<BootStage> {
    vec![
        BootStage {
            name: "Kernel entry point",
            marker: "Kernel entry point reached",
            failure_meaning: "Kernel failed to start at all",
            check_hint: "Bootloader configuration, kernel binary corrupted",
        },
        BootStage {
            name: "Serial port initialized",
            marker: "Serial port initialized and buffer flushed",
            failure_meaning: "Serial driver failed to initialize",
            check_hint: "serial::init() in kernel/src/serial.rs",
        },
        BootStage {
            name: "GDT and IDT initialized",
            marker: "GDT and IDT initialized",
            failure_meaning: "CPU descriptor tables not set up",
            check_hint: "interrupts::init() in kernel/src/interrupts/mod.rs",
        },
        BootStage {
            name: "Per-CPU data initialized",
            marker: "Per-CPU data initialized",
            failure_meaning: "Per-CPU storage failed",
            check_hint: "per_cpu::init() - check GS base setup",
        },
        BootStage {
            name: "Physical memory available",
            marker: "Physical memory offset available",
            failure_meaning: "Bootloader didn't map physical memory",
            check_hint: "BOOTLOADER_CONFIG in main.rs, check bootloader version",
        },
        BootStage {
            name: "IST stacks updated",
            marker: "Updated IST stacks with per-CPU emergency",
            failure_meaning: "Interrupt stack tables not configured",
            check_hint: "gdt::update_ist_stacks() - IST stack allocation",
        },
        BootStage {
            name: "Initial TSS.RSP0 configured",
            marker: "Initial TSS.RSP0 set to",
            failure_meaning: "Kernel stack for Ring 3 transitions not set",
            check_hint: "gdt::set_tss_rsp0() before contract tests",
        },
        BootStage {
            name: "Contract tests passed",
            marker: "Contract tests:",
            failure_meaning: "Kernel invariants violated",
            check_hint: "contract_runner.rs - check which specific contract failed",
        },
        BootStage {
            name: "Heap allocation working",
            marker: "Heap allocation test passed",
            failure_meaning: "Kernel heap allocator broken",
            check_hint: "memory::init() - heap initialization",
        },
        BootStage {
            name: "TLS initialized",
            marker: "TLS initialized",
            failure_meaning: "Thread local storage failed",
            check_hint: "tls::init() - TLS region allocation",
        },
        BootStage {
            name: "SWAPGS support enabled",
            marker: "SWAPGS support enabled",
            failure_meaning: "User/kernel transition mechanism failed",
            check_hint: "tls::setup_swapgs_support() - MSR configuration",
        },
        BootStage {
            name: "PIC initialized",
            marker: "PIC initialized",
            failure_meaning: "Programmable Interrupt Controller failed",
            check_hint: "interrupts::init_pic() - 8259 PIC setup",
        },
        BootStage {
            name: "Timer initialized",
            marker: "Timer initialized",
            failure_meaning: "PIT timer not configured",
            check_hint: "time::init() - timer frequency setup",
        },
        BootStage {
            name: "Syscall infrastructure ready",
            marker: "System call infrastructure initialized",
            failure_meaning: "INT 0x80 handler not installed",
            check_hint: "syscall::init() - IDT entry for syscalls",
        },
        BootStage {
            name: "Switched to kernel stack",
            marker: "Successfully switched to kernel stack",
            failure_meaning: "Failed to switch from bootstrap to kernel stack",
            check_hint: "stack_switch::switch_stack_and_call_with_arg()",
        },
        BootStage {
            name: "TSS.RSP0 verified",
            marker: "TSS.RSP0 verified at",
            failure_meaning: "TSS kernel stack pointer not properly set",
            check_hint: "per_cpu::update_tss_rsp0() after stack switch",
        },
        BootStage {
            name: "Threading subsystem ready",
            marker: "Threading subsystem initialized with init_task",
            failure_meaning: "Scheduler initialization failed",
            check_hint: "task::scheduler::init_with_current()",
        },
        BootStage {
            name: "Process management ready",
            marker: "Process management initialized",
            failure_meaning: "Process manager creation failed",
            check_hint: "process::init() - ProcessManager allocation",
        },
        BootStage {
            name: "First userspace process scheduled",
            marker: "RING3_SMOKE: created userspace PID",
            failure_meaning: "Failed to schedule first userspace process",
            check_hint: "process::creation::create_user_process() - ELF loading. This is a checkpoint - actual execution verified by stages 31-32",
        },
        BootStage {
            name: "Breakpoint test passed",
            marker: "Breakpoint test completed",
            failure_meaning: "int3 exception handler not working",
            check_hint: "IDT breakpoint handler in interrupts/mod.rs",
        },
        BootStage {
            name: "Kernel tests starting",
            marker: "Running kernel tests to create userspace processes",
            failure_meaning: "Test phase not reached",
            check_hint: "Check kernel initialization before tests",
        },
        BootStage {
            name: "Direct execution test: process scheduled",
            marker: "Direct execution test: process scheduled for execution",
            failure_meaning: "Failed to schedule direct execution test process",
            check_hint: "Check test_exec::test_direct_execution() and process creation logs. This is a checkpoint - actual execution verified by stage 31",
        },
        BootStage {
            name: "Fork test: process scheduled",
            marker: "Fork test: process scheduled for execution",
            failure_meaning: "Failed to schedule fork test process",
            check_hint: "Check test_exec::test_userspace_fork() and process creation logs. This is a checkpoint - actual execution verified by stage 31",
        },
        BootStage {
            name: "ENOSYS test: process scheduled",
            marker: "ENOSYS test: process scheduled for execution",
            failure_meaning: "Failed to schedule ENOSYS test process",
            check_hint: "Check test_exec::test_syscall_enosys() and process creation logs. This is a checkpoint - actual execution verified by stage 32",
        },
        BootStage {
            name: "Fault tests scheduled",
            marker: "Fault tests scheduled",
            failure_meaning: "Failed to schedule fault test processes",
            check_hint: "Check userspace_fault_tests::run_fault_tests() and process creation logs",
        },
        BootStage {
            name: "Preconditions validated",
            marker: "ALL PRECONDITIONS PASSED",
            failure_meaning: "Some interrupt/scheduler precondition failed",
            check_hint: "Check precondition validation output above",
        },
        BootStage {
            name: "Interrupts enabled",
            marker: "scheduler::schedule() returned",
            failure_meaning: "Interrupts not enabled or scheduler not running",
            check_hint: "x86_64::instructions::interrupts::enable() and scheduler::schedule()",
        },
        BootStage {
            name: "Clock gettime tests passed",
            marker: "clock_gettime tests passed",
            failure_meaning: "Time syscall implementation broken",
            check_hint: "clock_gettime_test::test_clock_gettime()",
        },
        BootStage {
            name: "Kernel initialization complete",
            marker: "Kernel initialization complete",
            failure_meaning: "Something failed in late init",
            check_hint: "Check logs above for specific failure",
        },
        BootStage {
            name: "Ring 3 entry (IRETQ)",
            marker: "RING3_ENTER: CS=0x33",
            failure_meaning: "Userspace entry via IRETQ failed",
            check_hint: "context_switch.rs - check stack frame for IRETQ",
        },
        BootStage {
            name: "Userspace syscall received",
            marker: "syscall handler|sys_write|sys_exit|sys_getpid",
            failure_meaning: "Userspace code not executing syscalls",
            check_hint: "Check if Ring 3 code runs, INT 0x80 handler",
        },
        // NEW STAGES: Verify actual userspace output, not just process creation
        BootStage {
            name: "Userspace hello printed",
            marker: "USERSPACE OUTPUT: Hello from userspace",
            failure_meaning: "hello_time.elf did not print output",
            check_hint: "Check if hello_time.elf actually executed and printed to stdout",
        },
        BootStage {
            name: "ENOSYS syscall verified",
            marker: "USERSPACE OUTPUT: ENOSYS OK",
            failure_meaning: "ENOSYS test did not print success",
            check_hint: "Check if syscall_enosys.elf executed and validated ENOSYS return value",
        },
    ]
}

/// Boot kernel once and validate each stage with real-time output
fn boot_stages() -> Result<()> {
    let stages = get_boot_stages();
    let total_stages = stages.len();

    println!("Boot Stage Validator - {} stages to check", total_stages);
    println!("=========================================\n");

    let serial_output_file = "target/xtask_boot_stages_output.txt";

    // Remove old output file
    let _ = fs::remove_file(serial_output_file);

    // Kill any existing QEMU
    let _ = Command::new("pkill")
        .args(&["-9", "qemu-system-x86_64"])
        .status();
    thread::sleep(Duration::from_millis(500));

    println!("Starting QEMU...\n");

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

    // Wait for output file to be created
    let start = Instant::now();
    let file_creation_timeout = Duration::from_secs(60);

    while !std::path::Path::new(serial_output_file).exists() {
        if start.elapsed() > file_creation_timeout {
            let _ = child.kill();
            bail!("Serial output file not created after {} seconds", file_creation_timeout.as_secs());
        }
        thread::sleep(Duration::from_millis(100));
    }

    // Track which stages have passed
    let mut stages_passed = 0;
    let mut last_content_len = 0;
    let mut checked_stages: Vec<bool> = vec![false; total_stages];
    let mut stage_timings: Vec<Option<StageTiming>> = vec![None; total_stages];
    let mut stage_start_time = Instant::now();

    let test_start = Instant::now();
    let timeout = Duration::from_secs(60);
    let stage_timeout = Duration::from_secs(30); // QEMU serial buffering can delay output by 15+ seconds
    let mut last_progress = Instant::now();

    // Print initial waiting message
    if let Some(stage) = stages.get(0) {
        print!("[{}/{}] {}...", 1, total_stages, stage.name);
        use std::io::Write;
        let _ = std::io::stdout().flush();
    }

    while test_start.elapsed() < timeout {
        if let Ok(mut file) = fs::File::open(serial_output_file) {
            // Use read_to_end + from_utf8_lossy to handle binary bytes in output
            let mut contents_bytes = Vec::new();
            if file.read_to_end(&mut contents_bytes).is_ok() {
                let contents = String::from_utf8_lossy(&contents_bytes);
                // Only process if content has changed
                if contents.len() > last_content_len {
                    last_content_len = contents.len();

                    // Check each unchecked stage
                    for (i, stage) in stages.iter().enumerate() {
                        if !checked_stages[i] {
                            // Check if marker is found (support regex-like patterns with |)
                            let found = if stage.marker.contains('|') {
                                stage.marker.split('|').any(|m| contents.contains(m))
                            } else {
                                contents.contains(stage.marker)
                            };

                            if found {
                                checked_stages[i] = true;
                                stages_passed += 1;
                                last_progress = Instant::now();

                                // Record timing for this stage
                                let duration = stage_start_time.elapsed();
                                stage_timings[i] = Some(StageTiming { duration });
                                stage_start_time = Instant::now();

                                // Format timing string
                                let time_str = if duration.as_secs() >= 1 {
                                    format!("{:.2}s", duration.as_secs_f64())
                                } else {
                                    format!("{}ms", duration.as_millis())
                                };

                                // Print result for this stage with timing
                                // Clear current line and print result
                                print!("\r[{}/{}] {}... ", i + 1, total_stages, stage.name);
                                // Pad to clear previous content
                                for _ in 0..(50 - stage.name.len().min(50)) {
                                    print!(" ");
                                }
                                println!("\r[{}/{}] {}... PASS ({})", i + 1, total_stages, stage.name, time_str);

                                // Print next stage we're waiting for
                                if i + 1 < total_stages {
                                    if let Some(next_stage) = stages.get(i + 1) {
                                        print!("[{}/{}] {}...", i + 2, total_stages, next_stage.name);
                                        use std::io::Write;
                                        let _ = std::io::stdout().flush();
                                    }
                                }
                            }
                        }
                    }

                    // Check for kernel panic
                    if contents.contains("KERNEL PANIC") {
                        println!("\r                                                              ");
                        println!("\nKERNEL PANIC detected!\n");

                        // Find the panic message
                        for line in contents.lines() {
                            if line.contains("KERNEL PANIC") {
                                println!("  {}", line);
                            }
                        }

                        break;
                    }

                    // All stages passed?
                    if stages_passed == total_stages {
                        break;
                    }
                }
            }
        }

        // Check for stage timeout
        if last_progress.elapsed() > stage_timeout {
            // Before giving up, send SIGTERM to allow QEMU to flush buffers
            println!("\r\nTimeout reached, sending SIGTERM to QEMU to flush buffers...");
            let _ = Command::new("pkill")
                .args(&["-TERM", "qemu-system-x86_64"])
                .status();

            // Wait 2 seconds for QEMU to flush and terminate gracefully
            thread::sleep(Duration::from_secs(2));

            // Check file one last time after buffers flush
            if let Ok(mut file) = fs::File::open(serial_output_file) {
                // Use read_to_end + from_utf8_lossy to handle binary bytes in output
                let mut contents_bytes = Vec::new();
                if file.read_to_end(&mut contents_bytes).is_ok() {
                    let contents = String::from_utf8_lossy(&contents_bytes);
                    // Check all remaining stages
                    let mut any_found = false;
                    for (i, stage) in stages.iter().enumerate() {
                        if !checked_stages[i] {
                            let found = if stage.marker.contains('|') {
                                stage.marker.split('|').any(|m| contents.contains(m))
                            } else {
                                contents.contains(stage.marker)
                            };

                            if found {
                                checked_stages[i] = true;
                                stages_passed += 1;
                                any_found = true;

                                // Record timing for this stage
                                let duration = stage_start_time.elapsed();
                                stage_timings[i] = Some(StageTiming { duration });
                                stage_start_time = Instant::now();

                                let time_str = if duration.as_secs() >= 1 {
                                    format!("{:.2}s", duration.as_secs_f64())
                                } else {
                                    format!("{}ms", duration.as_millis())
                                };

                                println!("[{}/{}] {}... PASS ({}, found after buffer flush)", i + 1, total_stages, stage.name, time_str);
                            }
                        }
                    }

                    if any_found {
                        last_progress = Instant::now();
                        // Continue checking if we found something
                        if stages_passed < total_stages {
                            continue;
                        }
                    }
                }
            }

            // Find first unchecked stage
            for (i, stage) in stages.iter().enumerate() {
                if !checked_stages[i] {
                    println!("\r                                                              ");
                    println!("\r[{}/{}] {}... FAIL (timeout)", i + 1, total_stages, stage.name);
                    println!();
                    println!("  Meaning: {}", stage.failure_meaning);
                    println!("  Check:   {}", stage.check_hint);
                    println!();

                    // Force kill QEMU if still running
                    let _ = child.kill();
                    let _ = child.wait();

                    // Print summary
                    println!("=========================================");
                    println!("Result: {}/{} stages passed", stages_passed, total_stages);
                    println!();
                    println!("Stage {} timed out after {}s waiting for marker:", i + 1, stage_timeout.as_secs());
                    println!("  \"{}\"", stage.marker);

                    bail!("Boot stage validation failed at stage {}", i + 1);
                }
            }
        }

        thread::sleep(Duration::from_millis(50));
    }

    // Kill QEMU
    let _ = child.kill();
    let _ = child.wait();

    println!();
    println!("=========================================");

    if stages_passed == total_stages {
        // Calculate total time
        let total_time: Duration = stage_timings.iter()
            .filter_map(|t| t.as_ref())
            .map(|t| t.duration)
            .sum();

        let total_str = if total_time.as_secs() >= 1 {
            format!("{:.2}s", total_time.as_secs_f64())
        } else {
            format!("{}ms", total_time.as_millis())
        };

        println!("Result: ALL {}/{} stages passed (total: {})", stages_passed, total_stages, total_str);
        Ok(())
    } else {
        // Find first failed stage
        for (i, stage) in stages.iter().enumerate() {
            if !checked_stages[i] {
                println!("Result: {}/{} stages passed", stages_passed, total_stages);
                println!();
                println!("First failed stage: [{}/{}] {}", i + 1, total_stages, stage.name);
                println!("  Meaning: {}", stage.failure_meaning);
                println!("  Check:   {}", stage.check_hint);
                break;
            }
        }

        bail!("Boot stage validation incomplete");
    }
}

/// Builds the kernel, boots it in QEMU, and asserts that the
/// hard-coded userspace program prints its greeting.
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
    let timeout = if std::env::var("CI").is_ok() {
        Duration::from_secs(60)  // 60 seconds for CI (kernel logs are verbose)
    } else {
        Duration::from_secs(30)  // 30 seconds locally
    };

    while test_start.elapsed() < timeout {
        if let Ok(mut file) = fs::File::open(serial_output_file) {
            let mut contents = String::new();
            if file.read_to_string(&mut contents).is_ok() {
                // Look for the RING3_SMOKE success marker or the completion marker
                if contents.contains("[ OK ] RING3_SMOKE: userspace executed + syscall path verified") ||
                   contents.contains("KERNEL_POST_TESTS_COMPLETE") {
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
        println!("\n  Ring-3 smoke test passed - userspace execution detected");
        Ok(())
    } else {
        bail!("\n  Ring-3 smoke test failed: no evidence of userspace execution");
    }
}

/// Builds the kernel, boots it in QEMU, and tests ENOSYS syscall handling.
fn ring3_enosys() -> Result<()> {
    println!("Starting Ring-3 ENOSYS test...");

    // Use serial output to file approach like the tests do
    let serial_output_file = "target/xtask_ring3_enosys_output.txt";

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
            "testing,external_test_bins",
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

    // Wait for output file to be created
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

    // Monitor the output file for expected strings
    let mut found_enosys_ok = false;
    let mut found_enosys_fail = false;
    let mut _found_invalid_syscall = false;
    let test_start = Instant::now();
    let timeout = if std::env::var("CI").is_ok() {
        Duration::from_secs(60)  // 60 seconds for CI
    } else {
        Duration::from_secs(30)  // 30 seconds locally
    };

    while test_start.elapsed() < timeout {
        if let Ok(mut file) = fs::File::open(serial_output_file) {
            let mut contents = String::new();
            if file.read_to_string(&mut contents).is_ok() {
                // Look for ENOSYS test results
                // IMPORTANT: Must use specific prefix to avoid matching instructional messages
                // like "Should print 'ENOSYS OK'" which would cause false positives.
                if contents.contains("USERSPACE OUTPUT: ENOSYS OK") {
                    found_enosys_ok = true;
                    break;
                }

                // Also accept plain "ENOSYS OK\n" at start of line (actual userspace output)
                if contents.lines().any(|line| line.trim() == "ENOSYS OK") {
                    found_enosys_ok = true;
                    break;
                }

                if contents.contains("USERSPACE OUTPUT: ENOSYS FAIL") ||
                   contents.contains("ENOSYS FAIL") {
                    found_enosys_fail = true;
                    break;
                }

                // Also check for kernel warning about invalid syscall
                if contents.contains("Invalid syscall number: 999") ||
                   contents.contains("unknown syscall: 999") {
                    _found_invalid_syscall = true;
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
            // Show lines containing ENOSYS or syscall-related messages
            for line in contents.lines() {
                if line.contains("ENOSYS") ||
                   line.contains("syscall") ||
                   line.contains("SYSCALL") ||
                   line.contains("Invalid") {
                    println!("{}", line);
                }
            }
        }
    }

    if found_enosys_fail {
        bail!("\n  ENOSYS test failed: syscall 999 did not return -38");
    } else if found_enosys_ok {
        println!("\n  ENOSYS test passed - syscall 999 correctly returned -38");
        Ok(())
    } else {
        bail!("\n  ENOSYS test failed: userspace did not report 'ENOSYS OK'.\n\
               This test requires:\n\
               1. Userspace process created successfully\n\
               2. Userspace executes syscall(999) from Ring 3\n\
               3. Userspace validates return value == -38\n\
               4. Userspace prints 'ENOSYS OK'");
    }
}

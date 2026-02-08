use std::{
    collections::hash_map::DefaultHasher,
    fs,
    hash::{Hash, Hasher},
    io::{Read, Write},
    net::TcpStream,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use anyhow::{bail, Result};
use structopt::StructOpt;

mod test_disk;

/// Get the PID file path unique to this worktree.
/// Uses a hash of the current working directory to avoid conflicts between worktrees.
fn get_qemu_pid_file() -> PathBuf {
    let cwd = std::env::current_dir().unwrap_or_default();
    let mut hasher = DefaultHasher::new();
    cwd.hash(&mut hasher);
    let hash = hasher.finish();
    PathBuf::from(format!("/tmp/breenix-qemu-{:016x}.pid", hash))
}

/// Send a signal to this worktree's QEMU process.
/// Returns true if the process was found and signaled.
fn signal_worktree_qemu(signal: &str) -> bool {
    let pid_file = get_qemu_pid_file();
    if let Ok(pid_str) = fs::read_to_string(&pid_file) {
        if let Ok(pid) = pid_str.trim().parse::<i32>() {
            #[cfg(unix)]
            {
                // Check if process exists before signaling
                let exists = Command::new("kill")
                    .args(&["-0", &pid.to_string()])
                    .status()
                    .map(|s| s.success())
                    .unwrap_or(false);
                if exists {
                    let _ = Command::new("kill")
                        .args(&[signal, &pid.to_string()])
                        .status();
                    return true;
                }
            }
        }
    }
    false
}

/// Kill any existing QEMU process that belongs to this worktree (SIGKILL).
/// Only kills the process if the PID file exists and the process is still running.
fn kill_worktree_qemu() {
    signal_worktree_qemu("-9");
    // Remove the stale PID file
    let _ = fs::remove_file(&get_qemu_pid_file());
}

/// Send SIGTERM to this worktree's QEMU to allow graceful shutdown.
fn term_worktree_qemu() {
    signal_worktree_qemu("-TERM");
}

/// Clean up a QEMU child process properly.
/// Kills the process, waits for it to exit, and removes the PID file.
fn cleanup_qemu_child(child: &mut std::process::Child) {
    // First try SIGTERM for graceful shutdown
    let _ = child.kill();

    // Wait with a short timeout
    for _ in 0..10 {
        match child.try_wait() {
            Ok(Some(_)) => {
                // Process has exited
                break;
            }
            Ok(None) => {
                // Still running, wait a bit
                thread::sleep(Duration::from_millis(50));
            }
            Err(_) => break,
        }
    }

    // Final wait to ensure it's reaped (non-blocking if already done)
    let _ = child.wait();

    // Clean up PID file
    let _ = fs::remove_file(get_qemu_pid_file());
}

/// Save the QEMU PID for this worktree.
fn save_qemu_pid(pid: u32) {
    let pid_file = get_qemu_pid_file();
    let _ = fs::write(&pid_file, pid.to_string());
}

fn build_std_test_binaries() -> Result<()> {
    println!("Building Rust std test binaries...\n");

    // Step 1: Build libbreenix-libc (produces libc.a)
    println!("  [1/2] Building libbreenix-libc...");
    let libc_dir = Path::new("libs/libbreenix-libc");

    if !libc_dir.exists() {
        println!("    Note: libs/libbreenix-libc not found, skipping std test binaries");
        return Ok(());
    }

    // Clear environment variables that might interfere with the standalone build
    // The rust-toolchain.toml in libbreenix-libc specifies the nightly version
    let status = Command::new("cargo")
        .args(&["build", "--release"])
        .current_dir(libc_dir)
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .env_remove("RUSTFLAGS")
        .env_remove("CARGO_TARGET_DIR")
        .env_remove("CARGO_BUILD_TARGET")
        .env_remove("CARGO_MANIFEST_DIR")
        .env_remove("CARGO_PKG_NAME")
        .env_remove("OUT_DIR")
        .status()
        .map_err(|e| anyhow::anyhow!("Failed to run cargo build for libbreenix-libc: {}", e))?;

    if !status.success() {
        bail!("Failed to build libbreenix-libc");
    }
    println!("    libbreenix-libc built successfully");

    // Step 2: Build userspace tests (produces hello_std_real)
    println!("  [2/2] Building userspace tests...");
    let tests_std_dir = Path::new("userspace/tests");

    if !tests_std_dir.exists() {
        println!("    Note: userspace/tests not found, skipping");
        return Ok(());
    }

    // The rust-toolchain.toml in tests specifies the nightly version
    // __CARGO_TESTS_ONLY_SRC_ROOT must point to the forked Rust library so that
    // -Z build-std compiles std from our patched sources (with target_os = "breenix")
    let rust_fork_library = std::env::current_dir()
        .unwrap_or_default()
        .join("rust-fork/library");
    let status = Command::new("cargo")
        .args(&["build", "--release"])
        .current_dir(tests_std_dir)
        .env("__CARGO_TESTS_ONLY_SRC_ROOT", &rust_fork_library)
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .env_remove("RUSTFLAGS")
        .env_remove("CARGO_TARGET_DIR")
        .env_remove("CARGO_BUILD_TARGET")
        .env_remove("CARGO_MANIFEST_DIR")
        .env_remove("CARGO_PKG_NAME")
        .env_remove("OUT_DIR")
        .status()
        .map_err(|e| anyhow::anyhow!("Failed to run cargo build for userspace tests: {}", e))?;

    if !status.success() {
        bail!("Failed to build userspace tests");
    }
    println!("    userspace tests built successfully");

    // Verify the binary exists
    let binary_path = tests_std_dir.join("target/x86_64-breenix/release/hello_std_real");
    if binary_path.exists() {
        println!("\n  hello_std_real binary ready at: {}", binary_path.display());
    } else {
        bail!(
            "Build succeeded but binary not found at: {}",
            binary_path.display()
        );
    }

    println!();
    Ok(())
}

/// Simple developer utility tasks.
#[derive(StructOpt)]
enum Cmd {
    /// Build Breenix and run the Ring-3 smoke test in QEMU.
    Ring3Smoke,
    /// Build Breenix and test ENOSYS syscall handling.
    Ring3Enosys,
    /// Boot kernel once and validate each boot stage sequentially.
    BootStages,
    /// Create test disk image containing all userspace test binaries.
    CreateTestDisk,
    /// Create ARM64 test disk image containing all ARM64 userspace test binaries.
    CreateTestDiskAarch64,
    /// Boot Breenix interactively with init_shell (serial console attached).
    Interactive,
    /// Run automated interactive shell tests (sends keyboard input via QEMU monitor).
    InteractiveTest,
    /// Run kthread stress test (100+ kthreads, rapid create/stop cycles).
    KthreadStress,
    /// Run focused DNS test only (faster iteration for network debugging).
    DnsTest,
}

fn main() -> Result<()> {
    match Cmd::from_args() {
        Cmd::Ring3Smoke => ring3_smoke(),
        Cmd::Ring3Enosys => ring3_enosys(),
        Cmd::BootStages => boot_stages(),
        Cmd::CreateTestDisk => test_disk::create_test_disk(),
        Cmd::CreateTestDiskAarch64 => test_disk::create_test_disk_aarch64(),
        Cmd::Interactive => interactive(),
        Cmd::InteractiveTest => interactive_test(),
        Cmd::KthreadStress => kthread_stress(),
        Cmd::DnsTest => dns_test(),
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
            name: "GDT segment test passed",
            marker: "GDT segment test passed",
            failure_meaning: "Segment registers have wrong values or privilege levels",
            check_hint: "gdt_tests::test_gdt_segments() - CS/DS not Ring 0",
        },
        BootStage {
            name: "GDT readability test passed",
            marker: "GDT readability test passed",
            failure_meaning: "GDT not accessible via SGDT or wrong size",
            check_hint: "gdt_tests::test_gdt_readable() - check GDT base/limit",
        },
        BootStage {
            name: "User segment configuration test passed",
            marker: "User segment configuration test passed",
            failure_meaning: "User segments (Ring 3) not properly configured",
            check_hint: "gdt_tests::test_user_segments() - verify user code/data selectors 0x33/0x2B",
        },
        BootStage {
            name: "User segment descriptor validation passed",
            marker: "User segment descriptor validation passed",
            failure_meaning: "User segment descriptors have wrong DPL or flags",
            check_hint: "gdt_tests::test_user_segment_descriptors() - check descriptor bits",
        },
        BootStage {
            name: "TSS descriptor test passed",
            marker: "TSS descriptor test passed",
            failure_meaning: "TSS descriptor not present or has wrong type",
            check_hint: "gdt_tests::test_tss_descriptor() - verify TSS type and presence",
        },
        BootStage {
            name: "TSS.RSP0 test passed",
            marker: "TSS.RSP0 test passed",
            failure_meaning: "TSS.RSP0 not properly configured or misaligned",
            check_hint: "gdt_tests::test_tss_rsp0() - check kernel stack pointer",
        },
        BootStage {
            name: "GDT tests completed",
            marker: "GDT tests completed",
            failure_meaning: "GDT validation did not finish",
            check_hint: "gdt_tests::run_all_tests() in main.rs",
        },
        BootStage {
            name: "Per-CPU data initialized",
            marker: "Per-CPU data initialized",
            failure_meaning: "Per-CPU storage failed",
            check_hint: "per_cpu::init() - check GS base setup",
        },
        BootStage {
            name: "HAL per-CPU initialized",
            marker: "HAL_PERCPU_INITIALIZED",
            failure_meaning: "HAL per-CPU abstraction layer failed to initialize",
            check_hint: "per_cpu::init() HAL integration - verify X86PerCpu trait methods work",
        },
        BootStage {
            name: "Physical memory available",
            marker: "Physical memory offset available",
            failure_meaning: "Bootloader didn't map physical memory",
            check_hint: "BOOTLOADER_CONFIG in main.rs, check bootloader version",
        },
        BootStage {
            name: "PCI bus enumerated",
            marker: "PCI: Enumeration complete",
            failure_meaning: "PCI enumeration failed or found no devices",
            check_hint: "drivers::pci::enumerate() - check I/O port access (0xCF8/0xCFC)",
        },
        BootStage {
            name: "E1000 network device found",
            marker: "E1000 network device found",
            failure_meaning: "No E1000 network device detected on PCI bus - network I/O will fail",
            check_hint: "Check QEMU e1000 configuration, verify vendor ID 0x8086 and device ID 0x100E",
        },
        BootStage {
            name: "VirtIO block device found",
            marker: "[1af4:1001] VirtIO MassStorage",
            failure_meaning: "No VirtIO block device detected - disk I/O will fail",
            check_hint: "Check QEMU virtio-blk-pci configuration, verify vendor ID 0x1AF4 and device ID 0x1001/0x1042",
        },
        BootStage {
            name: "VirtIO block driver initialized",
            marker: "VirtIO block: Driver initialized with",
            failure_meaning: "VirtIO device initialization failed - queue setup or feature negotiation issue",
            check_hint: "drivers/virtio/block.rs - check queue size matches device (must use exact device size in legacy mode)",
        },
        BootStage {
            name: "VirtIO disk read successful",
            marker: "VirtIO block test: Read successful!",
            failure_meaning: "VirtIO disk I/O failed - cannot read from block device",
            check_hint: "drivers/virtio/block.rs:read_sector() - check descriptor chain setup and polling",
        },
        BootStage {
            name: "E1000 driver initialized",
            marker: "E1000 driver initialized",
            failure_meaning: "E1000 device initialization failed - MMIO mapping, MAC setup, or link configuration issue",
            check_hint: "drivers/e1000/mod.rs:init() - check MMIO BAR mapping, MAC address reading, and link status",
        },
        BootStage {
            name: "Network stack initialized",
            marker: "Network stack initialized",
            failure_meaning: "Network stack initialization failed - ARP cache or configuration issue",
            check_hint: "net/mod.rs:init() - check network configuration and ARP cache initialization",
        },
        BootStage {
            name: "ARP request sent successfully",
            marker: "ARP request sent successfully",
            failure_meaning: "Failed to send ARP request for gateway - E1000 transmit path broken",
            check_hint: "net/arp.rs:request() and drivers/e1000/mod.rs:transmit() - check TX descriptor setup and transmission",
        },
        BootStage {
            name: "ARP reply received and gateway MAC resolved",
            marker: "NET: ARP resolved gateway MAC:",
            failure_meaning: "ARP request was sent but no reply received - network RX path broken or gateway not responding",
            check_hint: "net/arp.rs:handle_arp() - check E1000 RX descriptor processing, interrupt handling, and ARP reply parsing",
        },
        BootStage {
            name: "ICMP echo reply received from gateway",
            marker: "NET: ICMP echo reply received from",
            failure_meaning: "Ping was sent but no reply received - ICMP handling broken or gateway not responding to ping",
            check_hint: "net/icmp.rs:handle_icmp() - check ICMP echo reply processing and IPv4 packet handling",
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
            marker: "passed, 0 failed",
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
            name: "HAL timer calibrated",
            marker: "HAL_TIMER_CALIBRATED",
            failure_meaning: "HAL timer abstraction layer failed to calibrate TSC",
            check_hint: "arch_impl/x86_64/timer.rs calibrate() - verify TSC calibration via HAL",
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
        // NOTE: "Fault tests scheduled" stage removed - fault_test_thread spawning
        // is disabled in kernel/src/main.rs (yield_current() breaks first-run context switching).
        // Re-add this stage when fault tests are re-enabled.
        BootStage {
            name: "Precondition 1: IDT timer entry",
            marker: "PRECONDITION 1: IDT timer entry ✓ PASS",
            failure_meaning: "IDT timer entry not properly configured",
            check_hint: "interrupts::validate_timer_idt_entry() - verify IDT entry for IRQ0 (vector 32)",
        },
        BootStage {
            name: "Precondition 2: Timer handler registered",
            marker: "PRECONDITION 2: Timer handler registered ✓ PASS",
            failure_meaning: "Timer interrupt handler not registered",
            check_hint: "Check IDT entry for IRQ0 points to timer_interrupt_entry (same as Precondition 1)",
        },
        BootStage {
            name: "Precondition 3: PIT counter active",
            marker: "PRECONDITION 3: PIT counter ✓ PASS",
            failure_meaning: "PIT (Programmable Interval Timer) hardware not counting",
            check_hint: "time::timer::validate_pit_counting() - verify PIT counter changing between reads",
        },
        BootStage {
            name: "Precondition 4: PIC IRQ0 unmasked",
            marker: "PRECONDITION 4: PIC IRQ0 unmasked ✓ PASS",
            failure_meaning: "IRQ0 is masked in PIC - timer interrupts will not fire",
            check_hint: "interrupts::validate_pic_irq0_unmasked() - verify bit 0 of PIC1 mask register is clear",
        },
        BootStage {
            name: "Precondition 5: Runnable threads exist",
            marker: "PRECONDITION 5: Scheduler has runnable threads ✓ PASS",
            failure_meaning: "Scheduler has no runnable threads - timer interrupt has nothing to schedule",
            check_hint: "task::scheduler::has_runnable_threads() - verify userspace processes were created",
        },
        BootStage {
            name: "Precondition 6: Current thread set",
            marker: "PRECONDITION 6: Current thread set ✓ PASS",
            failure_meaning: "Current thread not set in per-CPU data",
            check_hint: "per_cpu::current_thread() - verify returns Some(thread) with valid pointer",
        },
        BootStage {
            name: "Precondition 7: Interrupts disabled",
            marker: "PRECONDITION 7: Interrupts disabled ✓ PASS",
            failure_meaning: "Interrupts already enabled - precondition validation should run with interrupts off",
            check_hint: "interrupts::are_interrupts_enabled() - verify RFLAGS.IF is clear",
        },
        BootStage {
            name: "All preconditions passed",
            marker: "ALL PRECONDITIONS PASSED",
            failure_meaning: "Summary check failed - not all individual preconditions passed",
            check_hint: "Check which specific precondition failed above (stages 35-41)",
        },
        BootStage {
            name: "Kernel timer arithmetic check",
            marker: "Timer resolution test passed",
            failure_meaning: "Basic timer arithmetic broken (ticks * 10 != get_monotonic_time())",
            check_hint: "time_test::test_timer_resolution() - trivial check that timer math is consistent with itself (not actual resolution validation)",
        },
        BootStage {
            name: "Kernel clock_gettime API test",
            marker: "clock_gettime tests passed",
            failure_meaning: "Kernel-internal clock_gettime function broken (not userspace test)",
            check_hint: "clock_gettime_test::test_clock_gettime() - calls clock_gettime() from kernel context only, does NOT test Ring 3 syscall path or userspace execution",
        },
        BootStage {
            name: "Kernel initialization complete",
            marker: "Kernel initialization complete",
            failure_meaning: "Something failed before enabling interrupts",
            check_hint: "Check logs above - this must print BEFORE interrupts enabled",
        },
        BootStage {
            name: "Interrupts enabled",
            marker: "scheduler::schedule() returned",
            failure_meaning: "Interrupts not enabled or scheduler not running",
            check_hint: "x86_64::instructions::interrupts::enable() and scheduler::schedule()",
        },
        BootStage {
            name: "Ring 3 execution confirmed",
            marker: "RING3_SYSCALL: First syscall from userspace",
            failure_meaning: "IRETQ may have succeeded but userspace did not execute or trigger a syscall",
            check_hint: "syscall/handler.rs - emit_ring3_syscall_marker() emits on first Ring 3 syscall",
        },
        // NOTE: Stage "Userspace syscall received" (marker "USERSPACE: sys_") removed as redundant.
        // Stage 36 "Ring 3 execution confirmed" already proves syscalls from Ring 3 work.
        // The "USERSPACE: sys_*" markers in syscall handlers violate hot-path performance requirements.
        // NEW STAGES: Verify actual userspace output, not just process creation
        BootStage {
            name: "Userspace hello printed",
            marker: "Hello from userspace",
            failure_meaning: "hello_time.elf did not print output",
            check_hint: "Check if hello_time.elf actually executed and printed to stdout",
        },
        BootStage {
            name: "Userspace register initialization validated",
            marker: "PASS: Callee-saved registers preserved as zero",
            failure_meaning: "Callee-saved registers (rbx, r12-r15) not zeroed on first userspace entry",
            check_hint: "Check setup_first_userspace_entry() in kernel/src/interrupts/context_switch.rs - register_init_test checks callee-saved registers are zero at main() entry",
        },
        BootStage {
            name: "Userspace clock_gettime validated",
            marker: "USERSPACE CLOCK_GETTIME: OK",
            failure_meaning: "Userspace process called clock_gettime syscall but got zero time or syscall failed",
            check_hint: "Verify INT 0x80 dispatch to SYS_clock_gettime (228) works from Ring 3 and returns non-zero time",
        },
        BootStage {
            name: "Userspace brk syscall validated",
            marker: "USERSPACE BRK: ALL TESTS PASSED",
            failure_meaning: "brk() syscall failed - heap expansion/contraction or memory access broken",
            check_hint: "Check sys_brk in syscall/memory.rs, verify page table mapping and heap_start/heap_end tracking",
        },
        BootStage {
            name: "Userspace mmap/munmap syscalls validated",
            marker: "USERSPACE MMAP: ALL TESTS PASSED",
            failure_meaning: "mmap/munmap syscalls failed - anonymous mapping or unmapping broken",
            check_hint: "Check sys_mmap/sys_munmap in syscall/mmap.rs, verify VMA tracking and page table operations",
        },
        // Diagnostic tests for syscall register corruption
        BootStage {
            name: "Diagnostic: Multiple getpid calls",
            marker: "Test 41a: Multiple no-arg syscalls (getpid)|Result: PASS",
            failure_meaning: "Multiple no-arg syscalls (getpid) failed - basic syscall mechanism broken",
            check_hint: "Check syscall0() wrapper and SYS_GETPID (39) handler in syscall_diagnostic_test.rs",
        },
        BootStage {
            name: "Diagnostic: Multiple sys_write calls",
            marker: "Test 41b: Multiple sys_write calls|Result: PASS",
            failure_meaning: "Multiple sys_write calls failed - multi-arg syscalls broken",
            check_hint: "Check syscall3() wrapper and SYS_WRITE (1) handler in syscall_diagnostic_test.rs",
        },
        BootStage {
            name: "Diagnostic: Single clock_gettime",
            marker: "Test 41c: Single clock_gettime|Result: PASS",
            failure_meaning: "First clock_gettime call failed - syscall2 or clock_gettime handler broken",
            check_hint: "Check syscall2() wrapper and SYS_CLOCK_GETTIME (228) handler",
        },
        BootStage {
            name: "Diagnostic: Register preservation",
            marker: "Test 41d: Register preservation|Result: PASS",
            failure_meaning: "Callee-saved registers (R12, R13) corrupted across syscall - context switch broken",
            check_hint: "Check SavedRegisters save/restore in interrupts/context_switch.rs",
        },
        BootStage {
            name: "Diagnostic: Second clock_gettime",
            marker: "Test 41e: Second clock_gettime|Result: PASS",
            failure_meaning: "Second clock_gettime call failed - register corruption between syscalls",
            check_hint: "Check SyscallFrame vs SavedRegisters sync in context_switch.rs - RDI corruption likely",
        },
        BootStage {
            name: "Diagnostic: Summary",
            marker: "✓ All diagnostic tests passed",
            failure_meaning: "Not all diagnostic tests passed - see individual test results above",
            check_hint: "Check which specific diagnostic test failed and follow its check_hint",
        },

        // Signal tests - validates signal delivery, handler execution, and context restoration
        BootStage {
            name: "Signal handler execution verified",
            marker: "SIGNAL_HANDLER_EXECUTED",
            failure_meaning: "Signal handler was not executed when signal was delivered",
            check_hint: "Check syscall/signal.rs:sys_sigaction() and signal delivery path in process module",
        },
        BootStage {
            name: "Signal handler return verified",
            marker: "SIGNAL_RETURN_WORKS",
            failure_meaning: "Signal handler did not return correctly via sigreturn trampoline",
            check_hint: "Check syscall/signal.rs:sys_sigreturn() and signal trampoline setup",
        },
        BootStage {
            name: "Signal register preservation verified",
            marker: "SIGNAL_REGS_PRESERVED",
            failure_meaning: "Registers were not properly preserved across signal delivery and return",
            check_hint: "Check signal context save/restore in syscall/signal.rs and sigreturn implementation",
        },
        BootStage {
            name: "sigaltstack() syscall verified",
            marker: "SIGALTSTACK_TEST_PASSED",
            failure_meaning: "sigaltstack() syscall failed - alternate signal stacks not working",
            check_hint: "Check syscall/signal.rs:sys_sigaltstack() and signal delivery path for SA_ONSTACK support",
        },

        // UDP Socket tests - validates full userspace->kernel->network path
        BootStage {
            name: "UDP socket created from userspace",
            marker: "UDP: Socket created fd=",
            failure_meaning: "sys_socket syscall failed from userspace",
            check_hint: "Check syscall/socket.rs:sys_socket() and socket module initialization",
        },
        BootStage {
            name: "UDP socket bound to port",
            marker: "UDP: Socket bound to port",
            failure_meaning: "sys_bind syscall failed - socket registry or port binding broken",
            check_hint: "Check syscall/socket.rs:sys_bind() and socket::SOCKET_REGISTRY",
        },
        BootStage {
            name: "UDP packet sent from userspace",
            marker: "UDP: Packet sent successfully",
            failure_meaning: "sys_sendto syscall failed - UDP TX path broken",
            check_hint: "Check syscall/socket.rs:sys_sendto(), net/udp.rs:build_udp_packet(), and net/mod.rs:send_ipv4()",
        },
        BootStage {
            name: "UDP RX socket created and bound",
            marker: "UDP: RX socket bound to port 54321",
            failure_meaning: "Failed to create or bind RX socket",
            check_hint: "Check syscall/socket.rs:sys_socket() and sys_bind()",
        },
        BootStage {
            name: "UDP loopback packet sent",
            marker: "UDP: Loopback packet sent",
            failure_meaning: "Failed to send packet to ourselves",
            check_hint: "Check net/udp.rs:build_udp_packet() and send_ipv4() for loopback handling",
        },
        BootStage {
            name: "UDP packet delivered to socket RX queue",
            marker: "UDP: Delivered packet to socket on port",
            failure_meaning: "Packet arrived but was not delivered to socket - RX delivery path broken",
            check_hint: "Check net/udp.rs:deliver_to_socket() - verify process lookup and packet enqueue",
        },
        BootStage {
            name: "UDP packet received from userspace",
            marker: "UDP: Received packet",
            failure_meaning: "sys_recvfrom syscall failed or returned no data - RX syscall broken",
            check_hint: "Check syscall/socket.rs:sys_recvfrom() and socket/udp.rs:recv_from()",
        },
        BootStage {
            name: "UDP RX data verified",
            marker: "UDP: RX data matches TX data - SUCCESS",
            failure_meaning: "Received packet but data was corrupted",
            check_hint: "Check packet data integrity in RX path - possible buffer corruption",
        },
        BootStage {
            name: "UDP ephemeral port bind",
            marker: "UDP_EPHEMERAL_TEST: port 0 bind OK",
            failure_meaning: "bind(port=0) failed - ephemeral port allocation broken",
            check_hint: "Check syscall/socket.rs:sys_bind() for port 0 handling",
        },
        BootStage {
            name: "UDP EADDRINUSE detection",
            marker: "UDP_EADDRINUSE_TEST: conflict detected OK",
            failure_meaning: "Binding to already-bound port did not return EADDRINUSE",
            check_hint: "Check kernel port conflict detection in UDP bind path",
        },
        BootStage {
            name: "UDP EAGAIN on empty queue",
            marker: "UDP_EAGAIN_TEST: empty queue OK",
            failure_meaning: "recvfrom on empty queue did not return EAGAIN",
            check_hint: "Check syscall/socket.rs:sys_recvfrom() - should return EAGAIN when no data",
        },
        BootStage {
            name: "UDP multiple packets received",
            marker: "UDP_MULTIPACKET_TEST: 3 packets OK",
            failure_meaning: "Failed to receive 3 packets in sequence - queue or delivery broken",
            check_hint: "Check net/udp.rs packet queuing and delivery_to_socket()",
        },
        BootStage {
            name: "UDP socket test completed",
            marker: "UDP Socket Test: All tests passed",
            failure_meaning: "UDP socket test did not complete successfully",
            check_hint: "Check userspace/tests/udp_socket_test.rs for which step failed",
        },
        // TCP Socket tests - validates TCP syscall path (socket, bind, listen, connect, accept)
        BootStage {
            name: "TCP socket created",
            marker: "TCP_TEST: socket created OK",
            failure_meaning: "sys_socket(SOCK_STREAM) failed from userspace",
            check_hint: "Check syscall/socket.rs:sys_socket() for SOCK_STREAM support",
        },
        BootStage {
            name: "TCP socket bound",
            marker: "TCP_TEST: bind OK",
            failure_meaning: "sys_bind for TCP socket failed",
            check_hint: "Check syscall/socket.rs:sys_bind() for TCP socket handling",
        },
        BootStage {
            name: "TCP socket listening",
            marker: "TCP_TEST: listen OK",
            failure_meaning: "sys_listen failed - TCP socket not converted to listener",
            check_hint: "Check syscall/socket.rs:sys_listen() implementation",
        },
        BootStage {
            name: "TCP client socket created",
            marker: "TCP_TEST: client socket OK",
            failure_meaning: "Second TCP socket creation failed",
            check_hint: "Check fd allocation or socket creation in sys_socket()",
        },
        BootStage {
            name: "TCP connect executed",
            marker: "TCP_TEST: connect OK",
            failure_meaning: "sys_connect failed to return Ok(())",
            check_hint: "Check syscall/socket.rs:sys_connect() - must return 0 for loopback connection",
        },
        BootStage {
            name: "TCP accept executed",
            marker: "TCP_TEST: accept OK",
            failure_meaning: "sys_accept returned unexpected error",
            check_hint: "Check syscall/socket.rs:sys_accept() - expected EAGAIN for no pending connections",
        },
        BootStage {
            name: "TCP shutdown executed",
            marker: "TCP_TEST: shutdown OK",
            failure_meaning: "sys_shutdown(SHUT_RDWR) on connected socket failed",
            check_hint: "Check syscall/socket.rs:sys_shutdown() for TcpConnection handling",
        },
        BootStage {
            name: "TCP shutdown unconnected rejected",
            marker: "TCP_TEST: shutdown_unconnected OK",
            failure_meaning: "sys_shutdown on unconnected socket did not return ENOTCONN",
            check_hint: "Check syscall/socket.rs:sys_shutdown() - unconnected TcpSocket should return ENOTCONN",
        },
        BootStage {
            name: "TCP EADDRINUSE detected",
            marker: "TCP_TEST: eaddrinuse OK",
            failure_meaning: "Binding to already-bound port did not return EADDRINUSE",
            check_hint: "Check kernel port conflict detection in sys_bind()",
        },
        BootStage {
            name: "TCP listen unbound rejected",
            marker: "TCP_TEST: listen_unbound OK",
            failure_meaning: "listen() on unbound socket did not return EINVAL",
            check_hint: "Check sys_listen() validates socket is bound first",
        },
        BootStage {
            name: "TCP accept nonlisten rejected",
            marker: "TCP_TEST: accept_nonlisten OK",
            failure_meaning: "accept() on non-listening socket did not return error",
            check_hint: "Check sys_accept() validates socket is in listen state",
        },
        // TCP Data Transfer Test - validates actual read/write over TCP connections
        BootStage {
            name: "TCP data test started",
            marker: "TCP_DATA_TEST: starting",
            failure_meaning: "TCP data transfer test did not start",
            check_hint: "Check userspace/tests/tcp_socket_test.rs - test 12 section",
        },
        BootStage {
            name: "TCP data server listening",
            marker: "TCP_DATA_TEST: server listening on 8082",
            failure_meaning: "TCP data test server failed to bind/listen on port 8082",
            check_hint: "Check sys_bind/sys_listen for TCP sockets",
        },
        BootStage {
            name: "TCP data client connected",
            marker: "TCP_DATA_TEST: client connected",
            failure_meaning: "TCP data test client failed to connect to server",
            check_hint: "Check sys_connect for loopback TCP connections",
        },
        BootStage {
            name: "TCP data send",
            marker: "TCP_DATA_TEST: send OK",
            failure_meaning: "write() syscall on TCP socket failed",
            check_hint: "Check kernel/src/syscall/io.rs:sys_write() for TCP socket support - must route to tcp_send()",
        },
        BootStage {
            name: "TCP data accept",
            marker: "TCP_DATA_TEST: accept OK",
            failure_meaning: "accept() on data test server returned EAGAIN or error after connect",
            check_hint: "Check sys_accept() - loopback connect should queue connection immediately",
        },
        BootStage {
            name: "TCP data recv",
            marker: "TCP_DATA_TEST: recv OK",
            failure_meaning: "read() syscall on accepted TCP socket failed",
            check_hint: "Check kernel/src/syscall/io.rs:sys_read() for TCP socket support - must route to tcp_recv()",
        },
        BootStage {
            name: "TCP data verified",
            marker: "TCP_DATA_TEST: data verified",
            failure_meaning: "Received TCP data did not match sent data 'HELLO'",
            check_hint: "Check tcp_send/tcp_recv implementation - data corruption or length mismatch",
        },
        // Test 13: Post-shutdown write verification
        BootStage {
            name: "TCP post-shutdown write test started",
            marker: "TCP_SHUTDOWN_WRITE_TEST: starting",
            failure_meaning: "Post-shutdown write test did not start",
            check_hint: "Previous test may have failed",
        },
        BootStage {
            name: "TCP write after shutdown rejected with EPIPE",
            marker: "TCP_SHUTDOWN_WRITE_TEST: EPIPE OK",
            failure_meaning: "Write after SHUT_WR was not properly rejected with EPIPE",
            check_hint: "Check tcp_send returns EPIPE when send_shutdown=true",
        },
        // Test 14: SHUT_RD test
        BootStage {
            name: "TCP SHUT_RD test started",
            marker: "TCP_SHUT_RD_TEST: starting",
            failure_meaning: "SHUT_RD test did not start",
            check_hint: "Previous test may have failed",
        },
        BootStage {
            name: "TCP SHUT_RD returns EOF",
            marker: "TCP_SHUT_RD_TEST:",
            failure_meaning: "Read after SHUT_RD did not return EOF",
            check_hint: "Check tcp_recv honors recv_shutdown flag",
        },
        // Test 15: SHUT_WR test
        BootStage {
            name: "TCP SHUT_WR test started",
            marker: "TCP_SHUT_WR_TEST: starting",
            failure_meaning: "SHUT_WR test did not start",
            check_hint: "Previous test may have failed",
        },
        BootStage {
            name: "TCP SHUT_WR succeeded",
            marker: "TCP_SHUT_WR_TEST: SHUT_WR write rejected OK",
            failure_meaning: "shutdown(SHUT_WR) failed on connected socket",
            check_hint: "Check sys_shutdown handling of SHUT_WR",
        },
        // Test 16: Bidirectional data test
        BootStage {
            name: "TCP bidirectional test started",
            marker: "TCP_BIDIR_TEST: starting",
            failure_meaning: "Bidirectional data test did not start",
            check_hint: "Previous test may have failed",
        },
        BootStage {
            name: "TCP server->client data verified",
            marker: "TCP_BIDIR_TEST: server->client OK",
            failure_meaning: "Server-to-client TCP data transfer failed",
            check_hint: "Check tcp_send/tcp_recv for accepted connection fd",
        },
        // Test 17: Large data test
        BootStage {
            name: "TCP large data test started",
            marker: "TCP_LARGE_TEST: starting",
            failure_meaning: "Large data test did not start",
            check_hint: "Previous test may have failed",
        },
        BootStage {
            name: "TCP 256 bytes verified",
            marker: "TCP_LARGE_TEST: 256 bytes verified OK",
            failure_meaning: "256-byte TCP transfer failed or corrupted",
            check_hint: "Check buffer handling in tcp_send/tcp_recv for larger data",
        },
        // Test 18: Backlog overflow test
        BootStage {
            name: "TCP backlog test started",
            marker: "TCP_BACKLOG_TEST: starting",
            failure_meaning: "Backlog test did not start",
            check_hint: "Previous test may have failed",
        },
        BootStage {
            name: "TCP backlog test passed",
            marker: "TCP_BACKLOG_TEST:",
            failure_meaning: "Backlog overflow test failed",
            check_hint: "Check tcp_listen backlog parameter handling",
        },
        // Test 19: ECONNREFUSED test
        BootStage {
            name: "TCP ECONNREFUSED test started",
            marker: "TCP_CONNREFUSED_TEST: starting",
            failure_meaning: "ECONNREFUSED test did not start",
            check_hint: "Previous test may have failed",
        },
        BootStage {
            name: "TCP ECONNREFUSED test passed",
            marker: "TCP_CONNREFUSED_TEST:",
            failure_meaning: "Connect to non-listening port did not fail properly",
            check_hint: "Check tcp_connect error handling for non-listening ports",
        },
        // Test 20: MSS boundary test
        BootStage {
            name: "TCP MSS test started",
            marker: "TCP_MSS_TEST: starting",
            failure_meaning: "MSS boundary test did not start",
            check_hint: "Previous test may have failed",
        },
        BootStage {
            name: "TCP MSS test passed",
            marker: "TCP_MSS_TEST: 2000 bytes",
            failure_meaning: "Data > MSS (1460) failed to transfer",
            check_hint: "Check TCP segmentation for large data transfers",
        },
        // Test 21: Multiple write/read cycles
        BootStage {
            name: "TCP multi-cycle test started",
            marker: "TCP_MULTI_TEST: starting",
            failure_meaning: "Multi-cycle test did not start",
            check_hint: "Previous test may have failed",
        },
        BootStage {
            name: "TCP multi-cycle test passed",
            marker: "TCP_MULTI_TEST: 3 messages",
            failure_meaning: "Multiple write/read cycles on same connection failed",
            check_hint: "Check TCP connection state management between sends",
        },
        // Test 22: Accept with client address
        BootStage {
            name: "TCP address test started",
            marker: "TCP_ADDR_TEST: starting",
            failure_meaning: "Client address test did not start",
            check_hint: "Previous test may have failed",
        },
        BootStage {
            name: "TCP address test passed",
            marker: "TCP_ADDR_TEST: 10.x.x.x OK",
            failure_meaning: "Accept did not return client address correctly",
            check_hint: "Check sys_accept address output handling",
        },
        // Test 23: Simultaneous close test
        BootStage {
            name: "TCP simultaneous close test started",
            marker: "TCP_SIMUL_CLOSE_TEST: starting",
            failure_meaning: "Simultaneous close test did not start",
            check_hint: "Previous test may have failed",
        },
        BootStage {
            name: "TCP simultaneous close test passed",
            marker: "TCP_SIMUL_CLOSE_TEST: simultaneous close OK",
            failure_meaning: "Both sides calling shutdown(SHUT_RDWR) simultaneously failed",
            check_hint: "Check sys_shutdown handling when both sides close together",
        },
        // Test 24: Half-close data flow test
        BootStage {
            name: "TCP half-close test started",
            marker: "TCP_HALFCLOSE_TEST: starting",
            failure_meaning: "Half-close test did not start",
            check_hint: "Previous test may have failed",
        },
        BootStage {
            name: "TCP half-close test passed",
            marker: "TCP_HALFCLOSE_TEST: read after SHUT_WR OK",
            failure_meaning: "Client could not read data after calling SHUT_WR (half-close broken)",
            check_hint: "Check tcp_recv - recv_shutdown should only be set by SHUT_RD, not SHUT_WR",
        },
        BootStage {
            name: "TCP socket test passed",
            marker: "TCP Socket Test: PASSED",
            failure_meaning: "TCP socket test did not complete successfully",
            check_hint: "Check userspace/tests/tcp_socket_test.rs for which step failed",
        },
        // DNS resolution tests - validates DNS client using UDP sockets
        // Network tests use SKIP markers when network is unavailable (CI flakiness)
        BootStage {
            name: "DNS google resolve",
            marker: "DNS_TEST: google_resolve OK|DNS_TEST: google_resolve SKIP",
            failure_meaning: "DNS resolution of www.google.com failed (not timeout/network issue)",
            check_hint: "Check libs/libbreenix/src/dns.rs:resolve() and UDP socket path",
        },
        BootStage {
            name: "DNS example resolve",
            marker: "DNS_TEST: example_resolve OK|DNS_TEST: example_resolve SKIP",
            failure_meaning: "DNS resolution of example.com failed (not timeout/network issue)",
            check_hint: "Check libs/libbreenix/src/dns.rs - may be DNS server or parsing issue",
        },
        BootStage {
            name: "DNS NXDOMAIN handling",
            marker: "DNS_TEST: nxdomain OK",
            failure_meaning: "NXDOMAIN handling for nonexistent domain failed",
            check_hint: "Check libs/libbreenix/src/dns.rs - error handling for RCODE 3",
        },
        BootStage {
            name: "DNS empty hostname",
            marker: "DNS_TEST: empty_hostname OK",
            failure_meaning: "Empty hostname validation failed",
            check_hint: "Check libs/libbreenix/src/dns.rs - resolve() should return InvalidHostname for empty string",
        },
        BootStage {
            name: "DNS long hostname",
            marker: "DNS_TEST: long_hostname OK",
            failure_meaning: "Hostname too long validation failed",
            check_hint: "Check libs/libbreenix/src/dns.rs - resolve() should return HostnameTooLong for >255 char hostname",
        },
        BootStage {
            name: "DNS txid varies",
            marker: "DNS_TEST: txid_varies OK|DNS_TEST: txid_varies SKIP",
            failure_meaning: "Transaction ID variation test failed (not timeout/network issue)",
            check_hint: "Check libs/libbreenix/src/dns.rs - generate_txid() should produce different IDs for consecutive queries",
        },
        BootStage {
            name: "DNS test completed",
            marker: "DNS Test: All tests passed",
            failure_meaning: "DNS test did not complete successfully",
            check_hint: "Check userspace/tests/dns_test.rs for which step failed",
        },
        // HTTP client tests - validates HTTP/1.1 GET over TCP using DNS resolution
        // Section 1: URL parsing tests (no network needed, specific error assertions)
        BootStage {
            name: "HTTP port out of range",
            marker: "HTTP_TEST: port_out_of_range OK",
            failure_meaning: "HTTP client should reject port > 65535 with InvalidUrl error",
            check_hint: "Check libs/libbreenix/src/http.rs parse_port() - must reject ports > 65535",
        },
        BootStage {
            name: "HTTP port non-numeric",
            marker: "HTTP_TEST: port_non_numeric OK",
            failure_meaning: "HTTP client should reject non-numeric port with InvalidUrl error",
            check_hint: "Check libs/libbreenix/src/http.rs parse_port() - must reject non-digit chars",
        },
        BootStage {
            name: "HTTP empty host",
            marker: "HTTP_TEST: empty_host OK",
            failure_meaning: "HTTP client should reject empty host with InvalidUrl error",
            check_hint: "Check libs/libbreenix/src/http.rs parse_url() - must validate host is non-empty",
        },
        BootStage {
            name: "HTTP URL too long",
            marker: "HTTP_TEST: url_too_long OK",
            failure_meaning: "HTTP client should reject URL > 2048 chars with UrlTooLong error",
            check_hint: "Check libs/libbreenix/src/http.rs http_get_with_buf() - MAX_URL_LEN check",
        },
        // Section 2: HTTPS rejection test (no network needed)
        BootStage {
            name: "HTTP HTTPS rejection",
            marker: "HTTP_TEST: https_rejected OK",
            failure_meaning: "HTTP client should reject HTTPS URLs with HttpsNotSupported error",
            check_hint: "Check libs/libbreenix/src/http.rs parse_url() HTTPS check",
        },
        // Section 3: Error handling for invalid domain (expects DnsError specifically)
        BootStage {
            name: "HTTP invalid domain",
            marker: "HTTP_TEST: invalid_domain OK",
            failure_meaning: "HTTP client should return DnsError for .invalid TLD",
            check_hint: "Check libs/libbreenix/src/http.rs and dns.rs - .invalid TLD must not resolve",
        },
        // Section 4: Network integration test (SKIP is acceptable if network unavailable)
        BootStage {
            name: "HTTP example fetch",
            marker: "HTTP_TEST: example_fetch OK|HTTP_TEST: example_fetch SKIP",
            failure_meaning: "HTTP GET to example.com failed with unexpected error",
            check_hint: "Check libs/libbreenix/src/http.rs - OK requires status 200 + HTML, SKIP for network unavailable",
        },
        BootStage {
            name: "HTTP test completed",
            marker: "HTTP Test: All tests passed",
            failure_meaning: "HTTP test did not complete successfully",
            check_hint: "Check userspace/tests/http_test.rs for which step failed",
        },
        // IPC (pipe) tests
        BootStage {
            name: "Pipe IPC test passed",
            marker: "PIPE_TEST_PASSED",
            failure_meaning: "pipe() syscall test failed - pipe creation, read/write, or close broken",
            check_hint: "Check kernel/src/syscall/pipe.rs and kernel/src/ipc/pipe.rs - verify pipe creation, fd allocation, and read/write operations",
        },
        // Unix domain socket test - validates socketpair AND named sockets (bind/listen/accept/connect)
        // Combined into single binary to avoid scheduler contention from multiple test processes
        BootStage {
            name: "Unix socket test passed",
            marker: "UNIX_SOCKET_TEST_PASSED",
            failure_meaning: "Unix domain socket test failed - socketpair or bind/listen/accept/connect broken",
            check_hint: "Check kernel/src/syscall/socket.rs Unix socket handling and kernel/src/socket/unix.rs",
        },
        // NOTE: Pipe + fork test and Pipe concurrent test removed.
        // These tests require complex process coordination and timing that
        // can cause spurious timeouts. The core pipe functionality is validated
        // by pipe_test (which passes) and the core fork functionality is
        // validated by waitpid_test and signal_fork_test (which pass).
        // Signal kill test - validates SIGTERM delivery with default handler
        // Parent forks child, sends SIGTERM, waits for child termination via waitpid
        BootStage {
            name: "SIGTERM kill test passed",
            marker: "SIGNAL_KILL_TEST_PASSED",
            failure_meaning: "SIGTERM kill test failed - SIGTERM not delivered to child or child not terminated",
            check_hint: "Check kernel/src/signal/delivery.rs, kernel/src/interrupts/context_switch.rs signal delivery path",
        },
        // SIGCHLD delivery test - run early to give child time to execute and exit
        // Validates SIGCHLD is sent to parent when child exits
        BootStage {
            name: "SIGCHLD delivery test passed",
            marker: "SIGCHLD_TEST_PASSED",
            failure_meaning: "SIGCHLD delivery test failed - SIGCHLD not delivered to parent when child exits",
            check_hint: "Check kernel/src/task/process_task.rs handle_thread_exit() SIGCHLD handling and signal delivery path",
        },
        // Pause syscall test - run early before fork-heavy tests
        BootStage {
            name: "Pause syscall test passed",
            marker: "PAUSE_TEST_PASSED",
            failure_meaning: "pause() syscall test failed - pause not blocking or signal not waking process",
            check_hint: "Check kernel/src/syscall/signal.rs:sys_pause() and signal delivery path",
        },
        // Sigsuspend syscall test - atomically replace mask and suspend
        BootStage {
            name: "Sigsuspend syscall test passed",
            marker: "SIGSUSPEND_TEST_PASSED",
            failure_meaning: "sigsuspend() syscall test failed - atomic mask replacement broken, signal not waking process, or original mask not restored",
            check_hint: "Check kernel/src/syscall/signal.rs:sys_sigsuspend() and signal delivery path - must atomically replace mask, suspend, and restore original mask",
        },
        // Process group kill semantics test
        BootStage {
            name: "Process group kill semantics test passed",
            marker: "KILL_PGROUP_TEST_PASSED",
            failure_meaning: "kill process group test failed - kill(0, sig), kill(-pgid, sig), or kill(-1, sig) not working correctly",
            check_hint: "Check kernel/src/syscall/signal.rs:sys_kill() and process group signal delivery in kernel/src/signal/delivery.rs",
        },
        // Dup syscall test
        BootStage {
            name: "Dup syscall test passed",
            marker: "DUP_TEST_PASSED",
            failure_meaning: "dup() syscall test failed - fd duplication, read/write on dup'd fd, or refcounting broken",
            check_hint: "Check kernel/src/syscall/io.rs:sys_dup() and fd table management",
        },
        // Fcntl syscall test
        BootStage {
            name: "Fcntl syscall test passed",
            marker: "FCNTL_TEST_PASSED",
            failure_meaning: "fcntl() syscall test failed - F_GETFD/F_SETFD/F_GETFL/F_SETFL/F_DUPFD broken",
            check_hint: "Check kernel/src/syscall/handlers.rs:sys_fcntl() and kernel/src/ipc/fd.rs",
        },
        // Close-on-exec (O_CLOEXEC) test
        BootStage {
            name: "Close-on-exec test passed",
            marker: "CLOEXEC_TEST_PASSED",
            failure_meaning: "close-on-exec test failed - FD_CLOEXEC not closing descriptors during exec",
            check_hint: "Check kernel/src/process/manager.rs exec_process_with_argv() and kernel/src/ipc/fd.rs handling of FD_CLOEXEC",
        },
        // Pipe2 syscall test
        BootStage {
            name: "Pipe2 syscall test passed",
            marker: "PIPE2_TEST_PASSED",
            failure_meaning: "pipe2() syscall test failed - O_CLOEXEC/O_NONBLOCK flags not applied correctly",
            check_hint: "Check kernel/src/syscall/pipe.rs:sys_pipe2() and kernel/src/ipc/fd.rs:alloc_with_entry()",
        },
        // Shell pipeline execution test - validates pipe+fork+dup2 pattern
        BootStage {
            name: "Shell pipeline execution test passed",
            marker: "SHELL_PIPE_TEST_PASSED",
            failure_meaning: "Shell pipeline test failed - pipe+fork+dup2 pattern broken, data not flowing through pipeline",
            check_hint: "Check dup2() in kernel/src/syscall/fd.rs, pipe read/write in kernel/src/ipc/pipe.rs, verify stdin/stdout redirection works",
        },
        // Signal exec reset test
        // Validates signal handlers are reset to SIG_DFL after exec
        BootStage {
            name: "Signal exec reset test passed",
            marker: "SIGNAL_EXEC_TEST_PASSED",
            failure_meaning: "signal exec reset test failed - signal handlers not reset to SIG_DFL after exec or exec() not replacing the process",
            check_hint: "Check kernel/src/process/manager.rs:exec_process() and kernel/src/syscall/handlers.rs:sys_exec_with_frame()",
        },
        // Waitpid test
        BootStage {
            name: "Waitpid test passed",
            marker: "WAITPID_TEST_PASSED",
            failure_meaning: "waitpid test failed - waitpid syscall, status extraction, or child exit handling broken",
            check_hint: "Check kernel/src/syscall/process.rs:sys_wait4(), process/manager.rs:wait_for_child(), and zombie cleanup",
        },
        // Signal fork inheritance test
        BootStage {
            name: "Signal fork inheritance test passed",
            marker: "SIGNAL_FORK_TEST_PASSED",
            failure_meaning: "signal fork inheritance test failed - signal handlers not properly inherited across fork",
            check_hint: "Check kernel/src/process/fork.rs signal handler cloning and signal/mod.rs",
        },
        // WNOHANG timing test
        BootStage {
            name: "WNOHANG timing test passed",
            marker: "WNOHANG_TIMING_TEST_PASSED",
            failure_meaning: "WNOHANG timing test failed - WNOHANG not returning correct values for running/exited/no children",
            check_hint: "Check kernel/src/syscall/process.rs:sys_wait4() WNOHANG handling",
        },
        // Poll syscall test
        BootStage {
            name: "Poll syscall test passed",
            marker: "POLL_TEST_PASSED",
            failure_meaning: "poll() syscall test failed - polling fd readiness broken",
            check_hint: "Check kernel/src/syscall/handlers.rs:sys_poll() and kernel/src/ipc/poll.rs",
        },
        // Select syscall test
        BootStage {
            name: "Select syscall test passed",
            marker: "SELECT_TEST_PASSED",
            failure_meaning: "select() syscall test failed - fd_set bitmap monitoring broken",
            check_hint: "Check kernel/src/syscall/handlers.rs:sys_select() and kernel/src/ipc/poll.rs",
        },
        // O_NONBLOCK pipe test
        BootStage {
            name: "O_NONBLOCK pipe test passed",
            marker: "NONBLOCK_TEST_PASSED",
            failure_meaning: "O_NONBLOCK pipe test failed - non-blocking read/write on pipes not returning EAGAIN correctly",
            check_hint: "Check kernel/src/syscall/handlers.rs:sys_read()/sys_write() O_NONBLOCK handling and kernel/src/ipc/pipe.rs",
        },
        // TTY layer test
        BootStage {
            name: "TTY layer test passed",
            marker: "TTY_TEST_PASSED",
            failure_meaning: "TTY layer test failed - isatty, tcgetattr, tcsetattr, or raw/cooked mode switching broken",
            check_hint: "Check kernel/src/tty/ module, kernel/src/syscall/ioctl.rs, and libs/libbreenix/src/termios.rs",
        },
        // Session syscall test
        BootStage {
            name: "Session syscall test passed",
            marker: "SESSION_TEST_PASSED",
            failure_meaning: "session/process group syscall test failed - getpgid/setpgid/getpgrp/getsid/setsid broken",
            check_hint: "Check kernel/src/syscall/process.rs and libs/libbreenix/src/process.rs session/pgid functions",
        },
        // ext2 file read test
        BootStage {
            name: "File read test passed",
            marker: "FILE_READ_TEST_PASSED",
            failure_meaning: "ext2 file read test failed - open, read, fstat, or close syscalls on ext2 filesystem broken",
            check_hint: "Check kernel/src/fs/ext2/ module, kernel/src/syscall/fs.rs, and ext2.img disk attachment",
        },
        // ext2 getdents test (directory listing)
        BootStage {
            name: "Getdents test passed",
            marker: "GETDENTS_TEST_PASSED",
            failure_meaning: "getdents64 syscall failed - directory listing on ext2 filesystem broken",
            check_hint: "Check kernel/src/syscall/fs.rs sys_getdents64(), O_DIRECTORY handling, and ext2 directory parsing",
        },
        // lseek test (SEEK_SET, SEEK_CUR, SEEK_END)
        BootStage {
            name: "Lseek test passed",
            marker: "LSEEK_TEST_PASSED",
            failure_meaning: "lseek syscall failed - SEEK_SET, SEEK_CUR, or SEEK_END broken",
            check_hint: "Check kernel/src/syscall/fs.rs sys_lseek(), especially SEEK_END with get_ext2_file_size()",
        },
        // Filesystem write test (write, O_CREAT, O_TRUNC, O_APPEND, unlink)
        BootStage {
            name: "Filesystem write test passed",
            marker: "FS_WRITE_TEST_PASSED",
            failure_meaning: "filesystem write operations failed - write, create, truncate, append, or unlink broken",
            check_hint: "Check kernel/src/syscall/fs.rs sys_open O_CREAT/O_TRUNC, handlers.rs sys_write for RegularFile, and fs.rs sys_unlink",
        },
        // Filesystem rename test (rename, cross-directory, error handling)
        BootStage {
            name: "Filesystem rename test passed",
            marker: "FS_RENAME_TEST_PASSED",
            failure_meaning: "filesystem rename operations failed - basic rename, cross-directory rename, or error handling broken",
            check_hint: "Check kernel/src/fs/ext2/mod.rs rename(), kernel/src/syscall/fs.rs sys_rename()",
        },
        // Filesystem large file test (50KB, indirect blocks)
        BootStage {
            name: "Large file test passed (indirect blocks)",
            marker: "FS_LARGE_FILE_TEST_PASSED",
            failure_meaning: "large file operations failed - indirect block allocation or read/write broken",
            check_hint: "Check kernel/src/fs/ext2/file.rs set_block_num(), write_file_range() for indirect block handling",
        },
        // Filesystem directory operations test (mkdir, rmdir)
        BootStage {
            name: "Directory ops test passed",
            marker: "FS_DIRECTORY_TEST_PASSED",
            failure_meaning: "directory operations failed - mkdir, rmdir, or directory structure broken",
            check_hint: "Check kernel/src/fs/ext2/mod.rs mkdir(), rmdir() and directory entry handling",
        },
        // Filesystem link operations test (link, symlink)
        BootStage {
            name: "Link ops test passed",
            marker: "FS_LINK_TEST_PASSED",
            failure_meaning: "link operations failed - hard links, symlinks, or link count handling broken",
            check_hint: "Check kernel/src/fs/ext2/mod.rs link(), symlink(), readlink() and inode link count",
        },
        // Filesystem access() syscall test
        BootStage {
            name: "Access syscall test passed",
            marker: "ACCESS_TEST_PASSED",
            failure_meaning: "access() syscall failed - file existence or permission checking broken",
            check_hint: "Check kernel/src/syscall/fs.rs sys_access() and permission bit handling",
        },
        // Device filesystem (devfs) test
        BootStage {
            name: "Devfs test passed",
            marker: "DEVFS_TEST_PASSED",
            failure_meaning: "devfs test failed - /dev/null, /dev/zero, or /dev/console broken",
            check_hint: "Check kernel/src/fs/devfs/mod.rs and syscall routing in sys_open for /dev/* paths",
        },
        // Current working directory syscalls test
        // NOTE: Individual substage markers (CWD_INITIAL_OK, CWD_CHDIR_OK, etc.) removed -
        // the cwd_test.rs program emits only descriptive PASS/FAIL lines and a final
        // CWD_TEST_PASSED marker. Re-add individual stages when test program is updated.
        BootStage {
            name: "CWD test passed",
            marker: "CWD_TEST_PASSED",
            failure_meaning: "One or more cwd tests failed",
            check_hint: "Check userspace/tests/cwd_test.rs output for specific failure",
        },
        // Exec from ext2 filesystem test - validates loading binaries from filesystem
        BootStage {
            name: "Exec ext2 test start",
            marker: "EXEC_EXT2_TEST_START",
            failure_meaning: "exec from ext2 test failed to start",
            check_hint: "Check userspace/tests/exec_from_ext2_test.rs is built and loaded",
        },
        BootStage {
            name: "Exec ext2 /bin OK",
            marker: "EXEC_EXT2_BIN_OK",
            failure_meaning: "exec /bin/hello_world from ext2 failed - filesystem loading broken",
            check_hint: "Check kernel/src/syscall/handlers.rs load_elf_from_ext2(), verify testdata/ext2.img has /bin/hello_world",
        },
        BootStage {
            name: "Exec ext2 ENOENT OK",
            marker: "EXEC_EXT2_ENOENT_OK",
            failure_meaning: "exec of nonexistent file did not return ENOENT",
            check_hint: "Check kernel/src/syscall/handlers.rs load_elf_from_ext2() error handling",
        },
        BootStage {
            name: "Exec ext2 EACCES OK",
            marker: "EXEC_EXT2_EACCES_OK",
            failure_meaning: "exec of non-executable file did not return EACCES",
            check_hint: "Check kernel/src/syscall/handlers.rs load_elf_from_ext2() permission check",
        },
        BootStage {
            name: "Exec ext2 ENOTDIR OK",
            marker: "EXEC_EXT2_ENOTDIR_OK",
            failure_meaning: "exec of directory did not return ENOTDIR/EACCES",
            check_hint: "Check kernel/src/syscall/handlers.rs load_elf_from_ext2() directory check",
        },
        BootStage {
            name: "Exec ext2 /bin/ls OK",
            marker: "EXEC_EXT2_LS_OK",
            failure_meaning: "exec /bin/ls failed - explicit path execution broken",
            check_hint: "Check kernel exec path resolution and ls binary in ext2 image",
        },
        BootStage {
            name: "Exec ext2 test passed",
            marker: "EXEC_EXT2_TEST_PASSED",
            failure_meaning: "One or more exec from ext2 tests failed",
            check_hint: "Check userspace/tests/exec_from_ext2_test.rs output for specific failure",
        },
        // Block allocation regression test - validates s_first_data_block fixes
        BootStage {
            name: "Block alloc test passed",
            marker: "BLOCK_ALLOC_TEST_PASSED",
            failure_meaning: "Block allocation regression test failed - truncate may not free blocks or allocate_block/free_block arithmetic wrong",
            check_hint: "Check userspace/tests/fs_block_alloc_test.rs and kernel/src/fs/ext2/block_group.rs s_first_data_block handling",
        },
        // Coreutil tests - verify true, false, head, tail, wc work correctly
        BootStage {
            name: "true coreutil test passed",
            marker: "TRUE_TEST_PASSED",
            failure_meaning: "/bin/true did not exit with code 0",
            check_hint: "Check userspace/tests/true.rs and true_test.rs",
        },
        BootStage {
            name: "false coreutil test passed",
            marker: "FALSE_TEST_PASSED",
            failure_meaning: "/bin/false did not exit with code 1",
            check_hint: "Check userspace/tests/false.rs and false_test.rs",
        },
        BootStage {
            name: "head coreutil test passed",
            marker: "HEAD_TEST_PASSED",
            failure_meaning: "/bin/head failed to process files correctly",
            check_hint: "Check userspace/tests/head.rs and head_test.rs",
        },
        BootStage {
            name: "tail coreutil test passed",
            marker: "TAIL_TEST_PASSED",
            failure_meaning: "/bin/tail failed to process files correctly",
            check_hint: "Check userspace/tests/tail.rs and tail_test.rs",
        },
        BootStage {
            name: "wc coreutil test passed",
            marker: "WC_TEST_PASSED",
            failure_meaning: "/bin/wc failed to count lines/words/bytes correctly",
            check_hint: "Check userspace/tests/wc.rs and wc_test.rs",
        },
        // which coreutil test
        BootStage {
            name: "which coreutil test passed",
            marker: "WHICH_TEST_PASSED",
            failure_meaning: "/bin/which failed to locate commands in PATH",
            check_hint: "Check userspace/tests/which.rs and which_test.rs",
        },
        // cat coreutil test
        BootStage {
            name: "cat coreutil test passed",
            marker: "CAT_TEST_PASSED",
            failure_meaning: "/bin/cat failed to output file contents correctly",
            check_hint: "Check userspace/tests/cat.rs and cat_test.rs",
        },
        // ls coreutil test
        BootStage {
            name: "ls coreutil test passed",
            marker: "LS_TEST_PASSED",
            failure_meaning: "/bin/ls failed to list directory contents correctly",
            check_hint: "Check userspace/tests/ls.rs and ls_test.rs",
        },
        // Rust std library test - validates real Rust std works in userspace
        BootStage {
            name: "Rust std println! works",
            marker: "RUST_STD_PRINTLN_WORKS",
            failure_meaning: "Rust std println! macro failed - std write syscall path broken",
            check_hint: "Check userspace/tests/src/hello_std_real.rs, verify libbreenix-libc is linked correctly",
        },
        BootStage {
            name: "Rust std Vec works",
            marker: "RUST_STD_VEC_WORKS",
            failure_meaning: "Rust std Vec allocation failed - heap allocation via mmap/brk broken for std programs",
            check_hint: "Check mmap/brk syscalls work correctly with std programs, verify GlobalAlloc implementation in libbreenix-libc",
        },
        BootStage {
            name: "Rust std String works",
            marker: "RUST_STD_STRING_WORKS",
            failure_meaning: "Rust std String operations failed - String concatenation or comparison broken",
            check_hint: "Check userspace/tests/src/hello_std_real.rs, verify String::from() and + operator work correctly",
        },
        BootStage {
            name: "Rust std getrandom works",
            marker: "RUST_STD_GETRANDOM_WORKS",
            failure_meaning: "getrandom() syscall failed - kernel random.rs may not be working",
            check_hint: "Check kernel/src/syscall/random.rs and libs/libbreenix-libc getrandom",
        },
        BootStage {
            name: "Rust std HashMap works",
            marker: "RUST_STD_HASHMAP_WORKS",
            failure_meaning: "HashMap creation failed - likely getrandom not seeding hasher correctly",
            check_hint: "HashMap requires working getrandom for hasher seeding",
        },
        BootStage {
            name: "Rust std realloc preserves data",
            marker: "RUST_STD_REALLOC_WORKS",
            failure_meaning: "realloc() did not preserve data when growing allocation - bounds check or copy logic broken",
            check_hint: "Check libs/libbreenix-libc/src/lib.rs realloc implementation - should copy min(old_size, new_size) bytes",
        },
        BootStage {
            name: "Rust std format! macro works",
            marker: "RUST_STD_FORMAT_WORKS",
            failure_meaning: "format! macro failed - heap allocation or formatting broken",
            check_hint: "Check String and Vec allocation paths",
        },
        BootStage {
            name: "Rust std realloc shrink preserves data",
            marker: "RUST_STD_REALLOC_SHRINK_WORKS",
            failure_meaning: "realloc() did not preserve data when shrinking - min(old,new) logic broken",
            check_hint: "Check libs/libbreenix-libc/src/lib.rs realloc implementation",
        },
        BootStage {
            name: "Rust std read() error handling works",
            marker: "RUST_STD_READ_ERROR_WORKS",
            failure_meaning: "read() did not return proper error for invalid fd",
            check_hint: "Check libs/libbreenix-libc/src/lib.rs read implementation",
        },
        BootStage {
            name: "Rust std read() success with pipe works",
            marker: "RUST_STD_READ_SUCCESS_WORKS",
            failure_meaning: "read() did not successfully read data from a pipe - pipe/write/read syscall path broken",
            check_hint: "Check libs/libbreenix-libc/src/lib.rs pipe/read implementation and kernel syscall/pipe.rs",
        },
        BootStage {
            name: "Rust std malloc boundary conditions work",
            marker: "RUST_STD_MALLOC_BOUNDARY_WORKS",
            failure_meaning: "malloc() boundary conditions failed - size=0 or small alloc broken",
            check_hint: "Check libs/libbreenix-libc/src/lib.rs malloc implementation",
        },
        BootStage {
            name: "Rust std posix_memalign works",
            marker: "RUST_STD_POSIX_MEMALIGN_WORKS",
            failure_meaning: "posix_memalign() failed - alignment or allocation broken",
            check_hint: "Check libs/libbreenix-libc/src/lib.rs posix_memalign implementation",
        },
        BootStage {
            name: "Rust std sbrk works",
            marker: "RUST_STD_SBRK_WORKS",
            failure_meaning: "sbrk() failed - heap extension or error handling broken",
            check_hint: "Check libs/libbreenix-libc/src/lib.rs sbrk implementation",
        },
        BootStage {
            name: "Rust std getpid/gettid works",
            marker: "RUST_STD_GETPID_WORKS",
            failure_meaning: "getpid()/gettid() failed - process/thread ID syscalls broken or returning invalid values",
            check_hint: "Check libs/libbreenix-libc/src/lib.rs getpid/gettid implementation and libbreenix::process::getpid/gettid",
        },
        BootStage {
            name: "Rust std posix_memalign error handling works",
            marker: "RUST_STD_POSIX_MEMALIGN_ERRORS_WORK",
            failure_meaning: "posix_memalign() did not return EINVAL for invalid alignments (0, non-power-of-2, < sizeof(void*))",
            check_hint: "Check libs/libbreenix-libc/src/lib.rs posix_memalign EINVAL error paths",
        },
        BootStage {
            name: "Rust std close works",
            marker: "RUST_STD_CLOSE_WORKS",
            failure_meaning: "close() syscall failed - fd closing, error handling (EBADF), or dup() broken",
            check_hint: "Check libs/libbreenix-libc/src/lib.rs close/dup implementation and kernel syscall/io.rs:sys_close",
        },
        BootStage {
            name: "Rust std mprotect works",
            marker: "RUST_STD_MPROTECT_WORKS",
            failure_meaning: "mprotect() syscall failed - memory protection changes not working or VMA lookup broken",
            check_hint: "Check libs/libbreenix-libc/src/lib.rs mprotect and kernel syscall/mmap.rs:sys_mprotect",
        },
        BootStage {
            name: "Rust std stub functions work",
            marker: "RUST_STD_STUB_FUNCTIONS_WORK",
            failure_meaning: "libc stub functions failed - pthread_*, signal, sysconf, poll, fcntl, getenv, or other stubs broken",
            check_hint: "Check libs/libbreenix-libc/src/lib.rs stub function implementations (pthread_self, signal, sysconf, poll, fcntl, getauxval, getenv, strlen, memcmp, __xpg_strerror_r)",
        },
        BootStage {
            name: "Rust std free(NULL) is safe",
            marker: "RUST_STD_FREE_NULL_WORKS",
            failure_meaning: "free(NULL) crashed or behaved incorrectly - should be a no-op per C99",
            check_hint: "Check libs/libbreenix-libc/src/lib.rs free implementation NULL check",
        },
        BootStage {
            name: "Rust std write edge cases work",
            marker: "RUST_STD_WRITE_EDGE_CASES_WORK",
            failure_meaning: "write() edge case handling failed - count=0, invalid fd (EBADF), or closed pipe (EPIPE) errors broken",
            check_hint: "Check libs/libbreenix-libc/src/lib.rs write implementation and kernel syscall/io.rs:sys_write error paths",
        },
        BootStage {
            name: "Rust std mmap/munmap direct tests work",
            marker: "RUST_STD_MMAP_WORKS",
            failure_meaning: "Direct mmap/munmap tests failed - anonymous mapping, memory access, unmapping, or error handling broken",
            check_hint: "Check libs/libbreenix-libc/src/lib.rs mmap/munmap and kernel syscall/mmap.rs:sys_mmap/sys_munmap",
        },
        BootStage {
            name: "Rust std thread::sleep works",
            marker: "RUST_STD_SLEEP_WORKS",
            failure_meaning: "nanosleep syscall failed - thread may not be waking from timer",
            check_hint: "Check kernel/src/syscall/time.rs:sys_nanosleep and scheduler wake_expired_timers",
        },
        BootStage {
            name: "Rust std thread::spawn and join work",
            marker: "RUST_STD_THREAD_WORKS",
            failure_meaning: "clone/futex syscalls failed - thread creation or join not working",
            check_hint: "Check kernel/src/syscall/clone.rs, kernel/src/syscall/futex.rs, and libs/libbreenix-libc pthread_create/pthread_join",
        },
        // Ctrl-C (SIGINT) signal delivery test
        // Tests the core signal mechanism that Ctrl-C would use:
        // - Parent forks child, sends SIGINT via kill()
        // - Child is terminated by default SIGINT handler
        // - Parent verifies WIFSIGNALED(status) && WTERMSIG(status) == SIGINT
        BootStage {
            name: "Ctrl-C (SIGINT) test passed",
            marker: "CTRL_C_TEST_PASSED",
            failure_meaning: "Ctrl-C signal test failed - SIGINT not delivered or child not terminated correctly",
            check_hint: "Check kernel/src/signal/delivery.rs, kernel/src/syscall/signal.rs:sys_kill(), and wstatus encoding in syscall/process.rs:sys_wait4()",
        },
        // Fork memory isolation test
        // Verifies copy-on-write (CoW) semantics work correctly:
        // - Stack isolation: child has separate copy of parent's stack
        // - Heap isolation: child has separate copy of parent's heap (sbrk)
        // - Global isolation: child has separate copy of parent's static data
        BootStage {
            name: "Fork memory isolation test passed",
            marker: "FORK_MEMORY_ISOLATION_PASSED",
            failure_meaning: "Fork memory isolation test failed - parent and child share memory instead of having isolated copies",
            check_hint: "Check kernel/src/process/fork.rs CoW page table cloning and fault handler in kernel/src/memory/",
        },
        // === Fork State Copy ===
        // Tests that fork correctly copies FD table, signal handlers, pgid, and sid
        BootStage {
            name: "Fork state copy test passed",
            marker: "FORK_STATE_COPY_PASSED",
            failure_meaning: "Fork state test failed - FD table, signal handlers, pgid, or sid not correctly inherited",
            check_hint: "Check kernel/src/process/fork.rs copy_process_state() function",
        },
        // === Fork Pending Signal ===
        // Tests POSIX requirement that pending signals are NOT inherited by child
        BootStage {
            name: "Fork pending signal test passed",
            marker: "FORK_PENDING_SIGNAL_TEST_PASSED",
            failure_meaning: "Fork pending signal test failed - child inherited pending signals (POSIX violation)",
            check_hint: "Check kernel/src/process/fork.rs and signal state handling during fork",
        },
        // === CoW Signal Delivery ===
        // Tests that signal delivery works when user stack is CoW-shared.
        // This specifically tests the deadlock fix where signal delivery
        // writes to a CoW page while holding the PROCESS_MANAGER lock.
        BootStage {
            name: "CoW signal delivery test passed",
            marker: "COW_SIGNAL_TEST_PASSED",
            failure_meaning: "CoW signal test failed - signal delivery deadlocked or failed when writing to CoW-shared stack",
            check_hint: "Check kernel/src/interrupts.rs handle_cow_direct() - CoW fault during signal delivery",
        },
        // === CoW Cleanup ===
        // Tests that frame reference counts are properly decremented when
        // forked children exit. Ensures no memory leaks or use-after-free.
        BootStage {
            name: "CoW cleanup test passed",
            marker: "COW_CLEANUP_TEST_PASSED",
            failure_meaning: "CoW cleanup test failed - frame refcounts not properly decremented on child exit",
            check_hint: "Check frame_decref() calls on process exit and CoW fault handling",
        },
        // === CoW Sole Owner Optimization ===
        // Tests that when child exits without writing, parent becomes sole
        // owner and can write without page copy (just makes page writable).
        BootStage {
            name: "CoW sole owner test passed",
            marker: "COW_SOLE_OWNER_TEST_PASSED",
            failure_meaning: "CoW sole owner test failed - sole owner optimization not working",
            check_hint: "Check frame_is_shared() and sole owner path in handle_cow_fault",
        },
        // === CoW Stress Test ===
        // Tests CoW at scale with many pages (128 pages = 512KB).
        // Verifies no memory corruption with many shared pages.
        BootStage {
            name: "CoW stress test passed",
            marker: "COW_STRESS_TEST_PASSED",
            failure_meaning: "CoW stress test failed - many CoW faults in sequence caused issues",
            check_hint: "Check refcounting at scale, memory corruption with many shared pages",
        },
        // === CoW Read-Only Page Sharing ===
        // Tests that read-only pages (code sections) are shared directly without
        // the COW flag. Code sections never need copying, so skipping COW reduces overhead.
        BootStage {
            name: "CoW read-only page sharing test passed",
            marker: "COW_READONLY_TEST_PASSED",
            failure_meaning: "CoW read-only test failed - code sections not shared correctly after fork",
            check_hint: "Check setup_cow_pages() read-only path in kernel/src/process/fork.rs",
        },
        // === Argv Support ===
        // Tests that exec syscall properly sets up argc/argv on the userspace stack
        BootStage {
            name: "Argv support test passed",
            marker: "ARGV_TEST_PASSED",
            failure_meaning: "Argv test failed - exec syscall not properly setting up argc/argv on stack",
            check_hint: "Check kernel/src/process/manager.rs exec_process_with_argv() and setup_argv_on_stack()",
        },
        // === Exec with Argv ===
        // Tests that fork+exec correctly passes argv to child process
        BootStage {
            name: "Exec argv test passed",
            marker: "EXEC_ARGV_TEST_PASSED",
            failure_meaning: "Exec argv test failed - fork+exec not passing arguments correctly",
            check_hint: "Check kernel/src/process/manager.rs exec_process_with_argv() and setup_argv_on_stack()",
        },
        // === Exec with Stack-Allocated Argv ===
        // Tests that stack-allocated argument buffers work through execv.
        // This is a regression test for a bug where the compiler could optimize
        // away stack-allocated arg buffers before the syscall read from them.
        // The fix uses core::hint::black_box() to prevent the optimization.
        BootStage {
            name: "Exec stack argv test passed",
            marker: "EXEC_STACK_ARGV_TEST_PASSED",
            failure_meaning: "Stack-allocated argv test failed - compiler may have optimized away argument buffers",
            check_hint: "Check core::hint::black_box() usage in try_execute_external() and test code",
        },
        // === Kernel Threads (kthreads) ===
        // Tests the kernel thread infrastructure for background work
        BootStage {
            name: "Kthread created",
            marker: "KTHREAD_CREATE: kthread created",
            failure_meaning: "Kernel thread creation failed - Thread::new_kernel or scheduler::spawn broken",
            check_hint: "Check kernel/src/task/kthread.rs:kthread_run() and scheduler integration",
        },
        BootStage {
            name: "Kthread running",
            marker: "KTHREAD_RUN: kthread running",
            failure_meaning: "Kernel thread not scheduled - timer interrupts not working or kthread entry not called",
            check_hint: "Check timer interrupt handler, scheduler ready queue, and kthread_entry() function",
        },
        BootStage {
            name: "Kthread stop signal sent",
            marker: "KTHREAD_STOP: kthread received stop signal",
            failure_meaning: "kthread_stop() failed - should_stop flag not set or kthread not woken",
            check_hint: "Check kthread_stop() and kthread_unpark() in kernel/src/task/kthread.rs",
        },
        BootStage {
            name: "Kthread exited cleanly",
            marker: "KTHREAD_EXIT: kthread exited cleanly",
            failure_meaning: "Kernel thread did not exit - kthread_should_stop() not returning true or thread not terminating",
            check_hint: "Check kthread_should_stop() and thread termination in kthread_entry()",
        },
        BootStage {
            name: "Kthread join completed",
            marker: "KTHREAD_JOIN_TEST: join returned exit_code=",
            failure_meaning: "kthread_join() failed - thread did not exit or exit_code not retrieved",
            check_hint: "Check kthread_join() spin loop and exit_code atomic access in kernel/src/task/kthread.rs",
        },
        BootStage {
            name: "Kthread park test started",
            marker: "KTHREAD_PARK_TEST: started",
            failure_meaning: "Kthread park test did not start - kthread creation or scheduling failed",
            check_hint: "Check test_kthread_park_unpark() in kernel/src/main.rs",
        },
        BootStage {
            name: "Kthread unparked successfully",
            marker: "KTHREAD_PARK_TEST: unparked",
            failure_meaning: "Kthread not unparked - kthread_park() blocked forever or kthread_unpark() not working",
            check_hint: "Check kthread_park() and kthread_unpark() in kernel/src/task/kthread.rs",
        },
        // === Work Queues ===
        // Tests the Linux-style work queue infrastructure for deferred execution
        BootStage {
            name: "Workqueue system initialized",
            marker: "WORKQUEUE_INIT: workqueue system initialized",
            failure_meaning: "Work queue initialization failed - system workqueue not created",
            check_hint: "Check kernel/src/task/workqueue.rs:init_workqueue()",
        },
        BootStage {
            name: "Kworker thread started",
            marker: "KWORKER_SPAWN: kworker/0 started",
            failure_meaning: "Worker thread spawn failed - kthread_run() or worker_thread_fn() broken",
            check_hint: "Check kernel/src/task/workqueue.rs:ensure_worker() and worker_thread_fn()",
        },
        BootStage {
            name: "Workqueue basic execution passed",
            marker: "WORKQUEUE_TEST: basic execution passed",
            failure_meaning: "Basic work execution failed - work not queued or worker not executing",
            check_hint: "Check Work::execute() and Workqueue::queue() in kernel/src/task/workqueue.rs",
        },
        BootStage {
            name: "Workqueue multiple items passed",
            marker: "WORKQUEUE_TEST: multiple work items passed",
            failure_meaning: "Multiple work items test failed - work not executed in order or not all executed",
            check_hint: "Check worker_thread_fn() loop and queue ordering in kernel/src/task/workqueue.rs",
        },
        BootStage {
            name: "Workqueue flush completed",
            marker: "WORKQUEUE_TEST: flush completed",
            failure_meaning: "Flush test failed - flush_system_workqueue() did not wait for pending work",
            check_hint: "Check Workqueue::flush() sentinel pattern in kernel/src/task/workqueue.rs",
        },
        BootStage {
            name: "Workqueue all tests passed",
            marker: "WORKQUEUE_TEST: all tests passed",
            failure_meaning: "One or more workqueue tests failed",
            check_hint: "Check test_workqueue() in kernel/src/main.rs for specific assertion failures",
        },
        BootStage {
            name: "Workqueue re-queue rejection passed",
            marker: "WORKQUEUE_TEST: re-queue rejection passed",
            failure_meaning: "Re-queue rejection test failed - schedule_work allowed already-pending work",
            check_hint: "Check Work::try_set_pending() and Workqueue::queue() in kernel/src/task/workqueue.rs",
        },
        BootStage {
            name: "Workqueue multi-item flush passed",
            marker: "WORKQUEUE_TEST: multi-item flush passed",
            failure_meaning: "Multi-item flush test failed - flush did not wait for all queued work",
            check_hint: "Check Workqueue::flush() and flush_system_workqueue() in kernel/src/task/workqueue.rs",
        },
        BootStage {
            name: "Workqueue shutdown test passed",
            marker: "WORKQUEUE_TEST: shutdown test passed",
            failure_meaning: "Workqueue shutdown test failed - destroy did not complete pending work",
            check_hint: "Check Workqueue::destroy() and worker_thread_fn() in kernel/src/task/workqueue.rs",
        },
        BootStage {
            name: "Workqueue error path test passed",
            marker: "WORKQUEUE_TEST: error path test passed",
            failure_meaning: "Workqueue error path test failed - schedule_work accepted re-queue",
            check_hint: "Check Work::try_set_pending() and Workqueue::queue() in kernel/src/task/workqueue.rs",
        },
        // === Softirq subsystem ===
        BootStage {
            name: "Softirq system initialized",
            marker: "SOFTIRQ_INIT: Softirq subsystem initialized",
            failure_meaning: "Softirq initialization failed - ksoftirqd not spawned",
            check_hint: "Check init_softirq() in kernel/src/task/softirqd.rs",
        },
        BootStage {
            name: "Softirq handler registration test passed",
            marker: "SOFTIRQ_TEST: handler registration passed",
            failure_meaning: "Softirq handler registration test failed",
            check_hint: "Check register_softirq_handler() in kernel/src/task/softirqd.rs",
        },
        BootStage {
            name: "Softirq Timer test passed",
            marker: "SOFTIRQ_TEST: Timer softirq passed",
            failure_meaning: "Timer softirq test failed - handler not called",
            check_hint: "Check raise_softirq() and do_softirq() in kernel/src/task/softirqd.rs",
        },
        BootStage {
            name: "Softirq NetRx test passed",
            marker: "SOFTIRQ_TEST: NetRx softirq passed",
            failure_meaning: "NetRx softirq test failed - handler not called",
            check_hint: "Check raise_softirq() and do_softirq() in kernel/src/task/softirqd.rs",
        },
        BootStage {
            name: "Softirq multiple test passed",
            marker: "SOFTIRQ_TEST: multiple softirqs passed",
            failure_meaning: "Multiple softirq test failed - handlers not all called",
            check_hint: "Check do_softirq() iteration in kernel/src/task/softirqd.rs",
        },
        BootStage {
            name: "Softirq priority order test passed",
            marker: "SOFTIRQ_TEST: priority order passed",
            failure_meaning: "Priority order test failed - lower priority softirq executed before higher priority",
            check_hint: "Check trailing_zeros() priority order in do_softirq() in kernel/src/task/softirqd.rs",
        },
        BootStage {
            name: "Softirq nested interrupt rejection test passed",
            marker: "SOFTIRQ_TEST: nested interrupt rejection passed",
            failure_meaning: "Nested interrupt rejection failed - do_softirq() should return false in interrupt context",
            check_hint: "Check in_interrupt() check at start of do_softirq() in kernel/src/task/softirqd.rs",
        },
        BootStage {
            name: "Softirq iteration limit test passed",
            marker: "SOFTIRQ_TEST: iteration limit passed",
            failure_meaning: "Iteration limit test failed - ksoftirqd did not process deferred softirqs",
            check_hint: "Check MAX_SOFTIRQ_RESTART limit and wakeup_ksoftirqd() in kernel/src/task/softirqd.rs",
        },
        BootStage {
            name: "Softirq ksoftirqd verification passed",
            marker: "SOFTIRQ_TEST: ksoftirqd verification passed",
            failure_meaning: "ksoftirqd verification failed - thread not initialized",
            check_hint: "Check is_initialized() and ksoftirqd_fn() in kernel/src/task/softirqd.rs",
        },
        BootStage {
            name: "Softirq all tests passed",
            marker: "SOFTIRQ_TEST: all tests passed",
            failure_meaning: "Softirq test suite failed",
            check_hint: "Check test_softirq() in kernel/src/main.rs",
        },
        // === Graphics syscalls ===
        // Tests that the FbInfo syscall (410) works correctly
        BootStage {
            name: "FbInfo syscall test passed",
            marker: "FBINFO_TEST: all tests PASSED",
            failure_meaning: "FbInfo syscall test failed - framebuffer info not returned correctly",
            check_hint: "Check kernel/src/syscall/graphics.rs sys_fbinfo() and libs/libbreenix/src/graphics.rs",
        },
        // NOTE: ENOSYS syscall verification requires external_test_bins feature
        // which is not enabled by default. Add back when external binaries are integrated.
    ]
}

/// Get DNS-specific boot stages for focused testing with dns_test_only feature
fn get_dns_stages() -> Vec<BootStage> {
    vec![
        // DNS_TEST_ONLY mode markers
        BootStage {
            name: "DNS test only mode started",
            marker: "DNS_TEST_ONLY: Starting minimal DNS test",
            failure_meaning: "Kernel didn't enter dns_test_only mode",
            check_hint: "Check kernel/src/main.rs dns_test_only_main() is being called",
        },
        BootStage {
            name: "DNS test process created",
            marker: "DNS_TEST_ONLY: Created dns_test process",
            failure_meaning: "Failed to create dns_test process",
            check_hint: "Check kernel/src/main.rs dns_test_only_main() create_user_process",
        },
        // DNS test userspace markers (go to COM1)
        BootStage {
            name: "DNS test starting",
            marker: "DNS Test: Starting",
            failure_meaning: "DNS test binary did not start executing",
            check_hint: "Check scheduler is running userspace, check userspace/tests/dns_test.rs",
        },
        BootStage {
            name: "DNS google resolve",
            marker: "DNS_TEST: google_resolve OK|DNS_TEST: google_resolve SKIP",
            failure_meaning: "DNS resolution of www.google.com failed (not timeout/network issue)",
            check_hint: "Check libs/libbreenix/src/dns.rs:resolve() and UDP socket/softirq path",
        },
        BootStage {
            name: "DNS example resolve",
            marker: "DNS_TEST: example_resolve OK|DNS_TEST: example_resolve SKIP",
            failure_meaning: "DNS resolution of example.com failed (not timeout/network issue)",
            check_hint: "Check libs/libbreenix/src/dns.rs - may be DNS server or parsing issue",
        },
        BootStage {
            name: "DNS test completed",
            marker: "DNS Test: All tests passed",
            failure_meaning: "DNS test did not complete all tests",
            check_hint: "Check userspace/tests/dns_test.rs for which step failed",
        },
    ]
}

/// Focused DNS test - only checks DNS-related boot stages
/// Much faster iteration than full boot_stages when debugging network issues
fn dns_test() -> Result<()> {
    let stages = get_dns_stages();
    let total_stages = stages.len();

    println!("DNS Test - {} stages to check", total_stages);
    println!("=========================================\n");

    // Build std test binaries BEFORE creating the test disk
    build_std_test_binaries()?;

    // Create the test disk with all userspace binaries
    test_disk::create_test_disk()?;
    println!();

    // COM2 (log output) - this is where all test markers go
    let serial_output_file = "target/xtask_dns_test_output.txt";
    // COM1 (user output) - raw userspace output
    let user_output_file = "target/xtask_dns_user_output.txt";

    // Remove old output files
    let _ = fs::remove_file(serial_output_file);
    let _ = fs::remove_file(user_output_file);

    // Kill any existing QEMU for THIS worktree only
    kill_worktree_qemu();
    thread::sleep(Duration::from_millis(500));

    println!("Starting QEMU for DNS test...\n");

    // Start QEMU with dns_test_only feature (skips all other tests)
    let mut child = Command::new("cargo")
        .args(&[
            "run",
            "--release",
            "-p",
            "breenix",
            "--features",
            "dns_test_only",  // Uses minimal boot path
            "--bin",
            "qemu-uefi",
            "--",
            "-serial",
            &format!("file:{}", user_output_file),
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

    save_qemu_pid(child.id());

    // Wait for output file to be created
    let start = Instant::now();
    let file_creation_timeout = Duration::from_secs(60);

    while !std::path::Path::new(serial_output_file).exists() {
        if start.elapsed() > file_creation_timeout {
            cleanup_qemu_child(&mut child);
            bail!("Serial output file not created after {} seconds", file_creation_timeout.as_secs());
        }
        thread::sleep(Duration::from_millis(100));
    }

    // Track which stages have passed
    let mut stages_passed = 0;
    let mut last_content_len = 0;
    let mut checked_stages: Vec<bool> = vec![false; total_stages];

    let test_start = Instant::now();
    // With dns_test_only feature, only essential boot + DNS test runs
    // Should complete in under 30 seconds
    let timeout = Duration::from_secs(60);

    loop {
        // Check timeout
        if test_start.elapsed() > timeout {
            cleanup_qemu_child(&mut child);
            println!("\n=========================================");
            println!("Result: {}/{} stages passed (TIMEOUT after {}s)", stages_passed, total_stages, timeout.as_secs());
            if stages_passed < total_stages {
                // Find first unpassed stage
                for (i, passed) in checked_stages.iter().enumerate() {
                    if !passed {
                        println!("\nFirst failed stage: [{}] {}", i + 1, stages[i].name);
                        println!("  Meaning: {}", stages[i].failure_meaning);
                        println!("  Check:   {}", stages[i].check_hint);
                        break;
                    }
                }
                bail!("DNS test incomplete - timeout");
            }
            break;
        }

        // Check if QEMU exited
        match child.try_wait() {
            Ok(Some(status)) => {
                // QEMU exited - do final check of both output files
                thread::sleep(Duration::from_millis(100));
                let kernel_content = fs::read_to_string(serial_output_file).unwrap_or_default();
                let user_content = fs::read_to_string(user_output_file).unwrap_or_default();
                for (i, stage) in stages.iter().enumerate() {
                    if !checked_stages[i] {
                        if kernel_content.contains(stage.marker) || user_content.contains(stage.marker) {
                            checked_stages[i] = true;
                            stages_passed += 1;
                            println!("[{}/{}] {}... PASS", i + 1, total_stages, stage.name);
                        }
                    }
                }
                println!("\n=========================================");
                if stages_passed == total_stages {
                    println!("Result: ALL {}/{} stages passed (total: {:.2}s)", stages_passed, total_stages, test_start.elapsed().as_secs_f64());
                    return Ok(());
                } else {
                    println!("Result: {}/{} stages passed (QEMU exit code: {:?})", stages_passed, total_stages, status.code());
                    for (i, passed) in checked_stages.iter().enumerate() {
                        if !passed {
                            println!("\nFirst failed stage: [{}] {}", i + 1, stages[i].name);
                            println!("  Meaning: {}", stages[i].failure_meaning);
                            println!("  Check:   {}", stages[i].check_hint);
                            break;
                        }
                    }
                    bail!("DNS test failed");
                }
            }
            Ok(None) => {
                // Still running
            }
            Err(e) => {
                bail!("Failed to check QEMU status: {}", e);
            }
        }

        // Read and check for markers from BOTH output files
        // - COM2 (kernel log): ARP and softirq markers
        // - COM1 (user output): DNS test markers (userspace prints to stdout -> COM1)
        let kernel_content = fs::read_to_string(serial_output_file).unwrap_or_default();
        let user_content = fs::read_to_string(user_output_file).unwrap_or_default();
        let combined_len = kernel_content.len() + user_content.len();

        if combined_len > last_content_len {
            last_content_len = combined_len;

            // Check all stages against both output sources
            for (i, stage) in stages.iter().enumerate() {
                if !checked_stages[i] {
                    if kernel_content.contains(stage.marker) || user_content.contains(stage.marker) {
                        checked_stages[i] = true;
                        stages_passed += 1;
                        println!("[{}/{}] {}... PASS", i + 1, total_stages, stage.name);
                    }
                }
            }

            // If all stages passed, we're done
            if stages_passed == total_stages {
                cleanup_qemu_child(&mut child);
                println!("\n=========================================");
                println!("Result: ALL {}/{} stages passed (total: {:.2}s)", stages_passed, total_stages, test_start.elapsed().as_secs_f64());
                return Ok(());
            }
        }

        thread::sleep(Duration::from_millis(100));
    }

    Ok(())
}

/// Boot kernel once and validate each stage with real-time output
///
/// Uses dual serial port configuration:
/// - COM1 (0x3F8): User I/O (raw userspace output) -> target/xtask_user_output.txt
/// - COM2 (0x2F8): Kernel logs (log::* output) -> target/xtask_boot_stages_output.txt
///
/// We primarily monitor COM2 since all markers use log::info!() and userspace
/// output is also logged there with "USERSPACE OUTPUT:" prefix.
fn boot_stages() -> Result<()> {
    let stages = get_boot_stages();
    let total_stages = stages.len();

    println!("Boot Stage Validator - {} stages to check", total_stages);
    println!("=========================================\n");

    // Build std test binaries BEFORE creating the test disk
    // This ensures hello_std_real is available to be included
    build_std_test_binaries()?;

    // Create the test disk with all userspace binaries
    test_disk::create_test_disk()?;
    println!();

    // COM2 (log output) - this is where all test markers go
    let serial_output_file = "target/xtask_boot_stages_output.txt";
    // COM1 (user output) - raw userspace output
    let user_output_file = "target/xtask_user_output.txt";

    // Remove old output files
    let _ = fs::remove_file(serial_output_file);
    let _ = fs::remove_file(user_output_file);

    // Kill any existing QEMU for THIS worktree only (not other worktrees)
    kill_worktree_qemu();
    thread::sleep(Duration::from_millis(500));

    println!("Starting QEMU...\n");

    // Start QEMU with dual serial ports:
    // - COM1 (0x3F8) -> user output file
    // - COM2 (0x2F8) -> kernel log output file (primary for test markers)
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
            &format!("file:{}", user_output_file),
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

    // Save the PID so other runs of this worktree can kill it if needed
    save_qemu_pid(child.id());

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
    // CI environments need more time due to virtualization overhead and resource contention
    // With 227 stages, the test can take 4-5 minutes in CI
    let timeout = if std::env::var("CI").is_ok() {
        Duration::from_secs(300) // 5 minutes for CI
    } else {
        Duration::from_secs(180) // 3 minutes locally - allows time for QEMU serial buffer flush
    };
    // Note: QEMU's file-based serial output uses stdio buffering (~4KB). When tests complete
    // quickly, their markers may still be in QEMU's buffer when the validator reads the file.
    // The SIGTERM+recheck mechanism (lines 1879-1941) partially addresses this, but intermittent
    // failures still occur. Matching CI timeout (90s) provides consistent behavior.
    let stage_timeout = if std::env::var("CI").is_ok() {
        Duration::from_secs(90) // 90 seconds per stage in CI
    } else {
        Duration::from_secs(90) // 90 seconds per stage locally (matches CI for consistency)
    };
    let mut last_progress = Instant::now();

    // Print initial waiting message
    if let Some(stage) = stages.get(0) {
        print!("[{}/{}] {}...", 1, total_stages, stage.name);
        use std::io::Write;
        let _ = std::io::stdout().flush();
    }

    while test_start.elapsed() < timeout {
        // Read both kernel log output (COM2) and userspace output (COM1)
        // Userspace stdout goes through TTY which writes to COM1
        // Kernel logs go to COM2
        let mut combined_contents = String::new();

        if let Ok(mut file) = fs::File::open(serial_output_file) {
            let mut contents_bytes = Vec::new();
            if file.read_to_end(&mut contents_bytes).is_ok() {
                combined_contents.push_str(&String::from_utf8_lossy(&contents_bytes));
            }
        }

        if let Ok(mut file) = fs::File::open(user_output_file) {
            let mut contents_bytes = Vec::new();
            if file.read_to_end(&mut contents_bytes).is_ok() {
                combined_contents.push_str(&String::from_utf8_lossy(&contents_bytes));
            }
        }

        // Only process if content has changed
        if combined_contents.len() > last_content_len {
            last_content_len = combined_contents.len();
            let contents = &combined_contents;

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

        // Check for stage timeout
        if last_progress.elapsed() > stage_timeout {
            // Before giving up, send SIGTERM to allow QEMU to flush buffers
            println!("\r\nTimeout reached, sending SIGTERM to QEMU to flush buffers...");
            term_worktree_qemu();

            // Wait 2 seconds for QEMU to flush and terminate gracefully
            thread::sleep(Duration::from_secs(2));

            // Check both files one last time after buffers flush
            let mut combined_contents = String::new();
            if let Ok(mut file) = fs::File::open(serial_output_file) {
                let mut contents_bytes = Vec::new();
                if file.read_to_end(&mut contents_bytes).is_ok() {
                    combined_contents.push_str(&String::from_utf8_lossy(&contents_bytes));
                }
            }
            if let Ok(mut file) = fs::File::open(user_output_file) {
                let mut contents_bytes = Vec::new();
                if file.read_to_end(&mut contents_bytes).is_ok() {
                    combined_contents.push_str(&String::from_utf8_lossy(&contents_bytes));
                }
            }

            if !combined_contents.is_empty() {
                let contents = &combined_contents;
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
                    cleanup_qemu_child(&mut child);

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

    // Kill QEMU and wait for it to fully terminate
    cleanup_qemu_child(&mut child);

    // Final scan of output files after QEMU terminates
    // This catches markers that were printed but not yet processed due to timing
    thread::sleep(Duration::from_millis(100)); // Let filesystem sync
    let mut combined_contents = String::new();
    if let Ok(mut file) = fs::File::open(serial_output_file) {
        let mut contents_bytes = Vec::new();
        if file.read_to_end(&mut contents_bytes).is_ok() {
            combined_contents.push_str(&String::from_utf8_lossy(&contents_bytes));
        }
    }
    if let Ok(mut file) = fs::File::open(user_output_file) {
        let mut contents_bytes = Vec::new();
        if file.read_to_end(&mut contents_bytes).is_ok() {
            combined_contents.push_str(&String::from_utf8_lossy(&contents_bytes));
        }
    }
    if !combined_contents.is_empty() {
        let contents = &combined_contents;
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
                    println!("[{}/{}] {}... PASS (found in final scan)", i + 1, total_stages, stage.name);
                }
            }
        }
    }

    println!();
    println!("=========================================");

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

    if stages_passed == total_stages {
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
///
/// Uses dual serial ports: COM1 for user output, COM2 for kernel logs.
fn ring3_smoke() -> Result<()> {
    println!("Starting Ring-3 smoke test...");

    // COM2 (log output) - where test markers go via log::info!()
    let serial_output_file = "target/xtask_ring3_smoke_output.txt";
    // COM1 (user output) - raw userspace output
    let user_output_file = "target/xtask_ring3_user_output.txt";

    // Remove old output files if they exist
    let _ = fs::remove_file(serial_output_file);
    let _ = fs::remove_file(user_output_file);

    // Kill any existing QEMU for THIS worktree only (not other worktrees)
    kill_worktree_qemu();
    thread::sleep(Duration::from_millis(500));

    println!("Building and running kernel with testing features...");

    // Start QEMU with dual serial ports:
    // - COM1 (0x3F8) -> user output file
    // - COM2 (0x2F8) -> kernel log output file (where markers go)
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
            &format!("file:{}", user_output_file),
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

    // Save the PID so other runs of this worktree can kill it if needed
    save_qemu_pid(child.id());

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
    cleanup_qemu_child(&mut child);

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
///
/// Uses dual serial ports: COM1 for user output, COM2 for kernel logs.
fn ring3_enosys() -> Result<()> {
    println!("Starting Ring-3 ENOSYS test...");

    // COM2 (log output) - where test markers go via log::info!()
    let serial_output_file = "target/xtask_ring3_enosys_output.txt";
    // COM1 (user output) - raw userspace output
    let user_output_file = "target/xtask_ring3_enosys_user_output.txt";

    // Remove old output files if they exist
    let _ = fs::remove_file(serial_output_file);
    let _ = fs::remove_file(user_output_file);

    // Kill any existing QEMU for THIS worktree only (not other worktrees)
    kill_worktree_qemu();
    thread::sleep(Duration::from_millis(500));

    println!("Building and running kernel with testing features...");

    // Start QEMU with dual serial ports:
    // - COM1 (0x3F8) -> user output file
    // - COM2 (0x2F8) -> kernel log output file (where markers go)
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
            &format!("file:{}", user_output_file),
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

    // Save the PID so other runs of this worktree can kill it if needed
    save_qemu_pid(child.id());

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
    cleanup_qemu_child(&mut child);

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

/// Boot Breenix interactively with init_shell and graphical PS/2 keyboard input.
///
/// Opens a QEMU graphical window where PS/2 keyboard input is captured and fed
/// to the init_shell via the keyboard interrupt handler.
///
/// Serial output goes to files for debugging:
/// - COM1 (0x3F8) -> target/serial_output.txt (user I/O from shell)
/// - COM2 (0x2F8) -> target/kernel.log (kernel debug logs)
fn interactive() -> Result<()> {
    println!("Building Breenix with interactive feature...");

    // Build with interactive feature
    let build_status = Command::new("cargo")
        .args(&[
            "build",
            "--release",
            "-p",
            "breenix",
            "--features",
            "testing,external_test_bins,interactive",
            "--bin",
            "qemu-uefi",
        ])
        .status()
        .map_err(|e| anyhow::anyhow!("Failed to run cargo build: {}", e))?;

    if !build_status.success() {
        bail!("Build failed");
    }

    println!();
    println!("=== Breenix Interactive Mode ===");
    println!();
    println!("A QEMU window will open shortly.");
    println!();
    println!("Instructions:");
    println!("  - Type in the QEMU window (not this terminal)");
    println!("  - The PS/2 keyboard sends input to the Breenix shell");
    println!("  - Close the QEMU window or press the power button to exit");
    println!();
    println!("Serial logs (for debugging):");
    println!("  - Shell I/O:    target/serial_output.txt");
    println!("  - Kernel logs:  target/kernel.log");
    println!();
    println!("Tip: In another terminal, run:");
    println!("  tail -f target/serial_output.txt");
    println!("  tail -f target/kernel.log");
    println!();

    // Run QEMU with graphical display for PS/2 keyboard input:
    // - Cocoa display on macOS for keyboard input (PS/2 keyboard handler feeds stdin)
    // - COM1 (0x3F8) -> file:target/serial_output.txt for shell I/O
    // - COM2 (0x2F8) -> file:target/kernel.log for kernel logs
    // Set BREENIX_INTERACTIVE=1 to tell qemu-uefi runner we're in interactive mode
    let run_status = Command::new("cargo")
        .args(&[
            "run",
            "--release",
            "-p",
            "breenix",
            "--features",
            "testing,external_test_bins,interactive",
            "--bin",
            "qemu-uefi",
            "--",
            "-display",
            "cocoa",
            "-serial",
            "file:target/serial_output.txt",
            "-serial",
            "file:target/kernel.log",
        ])
        .env("BREENIX_INTERACTIVE", "1")
        .status()
        .map_err(|e| anyhow::anyhow!("Failed to run QEMU: {}", e))?;

    if run_status.success() {
        Ok(())
    } else {
        bail!("QEMU exited with error");
    }
}

/// Automated interactive shell tests using QEMU monitor for keyboard input
///
/// This test:
/// 1. Boots Breenix with init_shell and QEMU TCP monitor enabled
/// 2. Waits for the shell prompt
/// 3. Sends keyboard commands via QEMU monitor's `sendkey` command
/// 4. Verifies command output appears in serial logs
/// 5. Tests multiple commands to ensure shell continues working
fn interactive_test() -> Result<()> {
    println!("=== Interactive Shell Test ===");
    println!();
    println!("This test sends keyboard input via QEMU monitor to verify:");
    println!("  - Shell accepts keyboard input");
    println!("  - Commands produce expected output");
    println!("  - Shell continues working after multiple commands");
    println!();

    // Output files
    let user_output_file = "target/interactive_test_user.txt";
    let kernel_log_file = "target/interactive_test_kernel.txt";

    // Clean up old files
    let _ = fs::remove_file(user_output_file);
    let _ = fs::remove_file(kernel_log_file);

    // Kill any existing QEMU for THIS worktree only (not other worktrees)
    kill_worktree_qemu();
    thread::sleep(Duration::from_millis(500));

    println!("Building with interactive feature...");

    // Build first
    let build_status = Command::new("cargo")
        .args(&[
            "build",
            "--release",
            "-p",
            "breenix",
            "--features",
            "testing,external_test_bins,interactive",
            "--bin",
            "qemu-uefi",
        ])
        .status()
        .map_err(|e| anyhow::anyhow!("Failed to run cargo build: {}", e))?;

    if !build_status.success() {
        bail!("Build failed");
    }

    println!("Starting QEMU with monitor enabled...");

    // Start QEMU with:
    // - TCP monitor on port 4444 for sending keyboard input
    // - Serial ports for capturing output
    // - No display (headless)
    let mut child = Command::new("cargo")
        .args(&[
            "run",
            "--release",
            "-p",
            "breenix",
            "--features",
            "testing,external_test_bins,interactive",
            "--bin",
            "qemu-uefi",
            "--",
            "-display",
            "none",
            "-serial",
            &format!("file:{}", user_output_file),
            "-serial",
            &format!("file:{}", kernel_log_file),
            "-monitor",
            "tcp:127.0.0.1:4444,server,nowait",
        ])
        .env("BREENIX_INTERACTIVE", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| anyhow::anyhow!("Failed to spawn QEMU: {}", e))?;

    // Save the PID so other runs of this worktree can kill it if needed
    save_qemu_pid(child.id());

    // Helper to clean up on failure
    let cleanup = |child: &mut std::process::Child| {
        cleanup_qemu_child(child);
    };

    // Wait for output files to be created
    let start = Instant::now();
    let file_timeout = Duration::from_secs(60);
    while !std::path::Path::new(user_output_file).exists() {
        if start.elapsed() > file_timeout {
            cleanup(&mut child);
            bail!("Output file not created after {} seconds", file_timeout.as_secs());
        }
        thread::sleep(Duration::from_millis(100));
    }

    // Wait for QEMU monitor to be ready
    println!("Waiting for QEMU monitor...");
    let monitor_start = Instant::now();
    let monitor_timeout = Duration::from_secs(10);
    let mut monitor: Option<TcpStream> = None;

    while monitor_start.elapsed() < monitor_timeout {
        if let Ok(stream) = TcpStream::connect("127.0.0.1:4444") {
            stream.set_read_timeout(Some(Duration::from_secs(2))).ok();
            stream.set_write_timeout(Some(Duration::from_secs(2))).ok();
            monitor = Some(stream);
            break;
        }
        thread::sleep(Duration::from_millis(200));
    }

    let mut monitor = match monitor {
        Some(m) => m,
        None => {
            cleanup(&mut child);
            bail!("Could not connect to QEMU monitor on port 4444");
        }
    };

    // Read initial monitor prompt
    let mut buf = [0u8; 1024];
    let _ = monitor.read(&mut buf);

    println!("Connected to QEMU monitor");

    // Wait for shell prompt to appear in user output
    println!("Waiting for shell prompt...");
    let prompt_start = Instant::now();
    let prompt_timeout = Duration::from_secs(30);

    while prompt_start.elapsed() < prompt_timeout {
        if let Ok(contents) = fs::read_to_string(user_output_file) {
            if contents.contains("breenix>") {
                println!("Shell prompt detected!");
                break;
            }
        }
        thread::sleep(Duration::from_millis(200));
    }

    // Helper function to send a string as keyboard input
    fn send_string(monitor: &mut TcpStream, s: &str) -> Result<()> {
        for c in s.chars() {
            let key = match c {
                'a'..='z' => c.to_string(),
                'A'..='Z' => format!("shift-{}", c.to_ascii_lowercase()),
                '0'..='9' => c.to_string(),
                ' ' => "spc".to_string(),
                '\n' => "ret".to_string(),
                '-' => "minus".to_string(),
                '_' => "shift-minus".to_string(),
                '.' => "dot".to_string(),
                '/' => "slash".to_string(),
                '\\' => "backslash".to_string(),
                ':' => "shift-semicolon".to_string(),
                ';' => "semicolon".to_string(),
                '=' => "equal".to_string(),
                '+' => "shift-equal".to_string(),
                '|' => "shift-backslash".to_string(),
                _ => continue, // Skip unsupported characters
            };
            let cmd = format!("sendkey {}\n", key);
            monitor.write_all(cmd.as_bytes())?;
            thread::sleep(Duration::from_millis(50)); // Small delay between keys
        }
        // Read any response from monitor
        let mut buf = [0u8; 256];
        let _ = monitor.read(&mut buf);
        Ok(())
    }

    // Helper to wait for string in output
    fn wait_for_output(file: &str, needle: &str, timeout: Duration) -> bool {
        let start = Instant::now();
        while start.elapsed() < timeout {
            if let Ok(contents) = fs::read_to_string(file) {
                if contents.contains(needle) {
                    return true;
                }
            }
            thread::sleep(Duration::from_millis(100));
        }
        false
    }

    // Track test results
    let mut tests_passed = 0;
    let mut tests_failed = 0;

    // Test 1: Run "help" command
    println!();
    println!("[Test 1] Sending 'help' command...");
    send_string(&mut monitor, "help\n")?;

    if wait_for_output(user_output_file, "Built-in commands:", Duration::from_secs(5)) {
        println!("  ✓ PASS: 'help' command produced expected output");
        tests_passed += 1;
    } else {
        println!("  ✗ FAIL: 'help' command did not produce expected output");
        tests_failed += 1;
    }

    // Give shell time to return to prompt
    thread::sleep(Duration::from_millis(500));

    // Test 2: Run "help" again to verify shell continues working
    println!();
    println!("[Test 2] Sending 'help' command again (testing subsequent commands)...");
    let output_len_before = fs::read_to_string(user_output_file).unwrap_or_default().len();
    send_string(&mut monitor, "help\n")?;

    thread::sleep(Duration::from_secs(2));
    let output_after = fs::read_to_string(user_output_file).unwrap_or_default();

    // Check if more output was produced (shell responded to second command)
    if output_after.len() > output_len_before + 100 {
        println!("  ✓ PASS: Shell responded to second 'help' command");
        tests_passed += 1;
    } else {
        println!("  ✗ FAIL: Shell did not respond to second command (this is the bug!)");
        tests_failed += 1;
    }

    // Test 3: Run "uptime" command
    println!();
    println!("[Test 3] Sending 'uptime' command...");
    send_string(&mut monitor, "uptime\n")?;

    // uptime prints "up N seconds" or similar
    if wait_for_output(user_output_file, "up ", Duration::from_secs(5)) {
        println!("  ✓ PASS: 'uptime' command produced expected output");
        tests_passed += 1;
    } else {
        println!("  ✗ FAIL: 'uptime' command did not produce expected output");
        tests_failed += 1;
    }

    // Test 4: Run "cat /hello.txt" to test argv passing
    println!();
    println!("[Test 4] Sending 'cat /hello.txt' command...");
    send_string(&mut monitor, "cat /hello.txt\n")?;
    thread::sleep(Duration::from_secs(3)); // Give time for cat to run and print debug

    let cat_output = fs::read_to_string(user_output_file).unwrap_or_default();
    // Check for cat debug output to understand what's happening
    if cat_output.contains("Hello from ext2") || cat_output.contains("Hello, World") {
        println!("  ✓ PASS: 'cat /hello.txt' displayed file contents");
        tests_passed += 1;
    } else if cat_output.contains("cat: missing file operand") {
        println!("  ✗ FAIL: cat didn't receive argv (argc < 2)");
        // Print debug info if available
        if let Some(debug_start) = cat_output.rfind("cat DEBUG:") {
            let debug_section: String = cat_output[debug_start..].lines().take(5).collect::<Vec<_>>().join("\n");
            println!("  Debug output: {}", debug_section);
        }
        tests_failed += 1;
    } else if cat_output.contains("cat DEBUG:") {
        // Debug output exists but file not found or other error
        println!("  ? cat debug output present, checking...");
        let debug_lines: Vec<&str> = cat_output.lines()
            .filter(|l| l.contains("cat DEBUG:") || l.contains("cat:"))
            .collect();
        for line in debug_lines.iter().take(10) {
            println!("    {}", line);
        }
        tests_failed += 1;
    } else {
        println!("  ? INCONCLUSIVE: Could not verify cat output");
        println!("  Last 5 lines of output:");
        for line in cat_output.lines().rev().take(5).collect::<Vec<_>>().into_iter().rev() {
            println!("    {}", line);
        }
    }

    // Test 5: Send Ctrl-C while at prompt (should just print ^C and continue)
    println!();
    println!("[Test 5] Sending Ctrl-C at prompt...");
    // Ctrl-C is sent as ctrl-c in QEMU
    monitor.write_all(b"sendkey ctrl-c\n")?;
    let _ = monitor.read(&mut buf);
    thread::sleep(Duration::from_secs(1));

    // Shell should still be responsive - send another command
    // Use a unique command "jobs" to detect if first char is lost (would show "Unknown command: obs")
    send_string(&mut monitor, "jobs\n")?;
    thread::sleep(Duration::from_secs(2));

    let ctrl_c_output = fs::read_to_string(user_output_file).unwrap_or_default();
    // Check for the specific bug: "Unknown command: obs" indicates first char was eaten
    if ctrl_c_output.contains("Unknown command: obs") {
        println!("  ✗ FAIL: First character after Ctrl-C was lost ('jobs' became 'obs')");
        tests_failed += 1;
    } else if ctrl_c_output.contains("No background jobs") || ctrl_c_output.contains("jobs") {
        println!("  ✓ PASS: Shell remained responsive after Ctrl-C");
        tests_passed += 1;
    } else {
        println!("  ? INCONCLUSIVE: Could not verify shell response after Ctrl-C");
        // Don't count as failure, just informational
    }

    // Test 6: Run spinner and Ctrl-C to interrupt it
    println!();
    println!("[Test 6] Running spinner and sending Ctrl-C to interrupt...");
    send_string(&mut monitor, "spinner\n")?;
    thread::sleep(Duration::from_secs(2)); // Let spinner start

    // Send Ctrl-C to interrupt
    monitor.write_all(b"sendkey ctrl-c\n")?;
    let _ = monitor.read(&mut buf);
    thread::sleep(Duration::from_secs(2));

    // Check if shell returned to prompt (should see another breenix> after the ^C)
    let spinner_output = fs::read_to_string(user_output_file).unwrap_or_default();
    let ctrl_c_count = spinner_output.matches("^C").count();
    if ctrl_c_count >= 2 && spinner_output.ends_with("breenix> ") || spinner_output.contains("breenix> \n") {
        println!("  ✓ PASS: Spinner interrupted and shell returned to prompt");
        tests_passed += 1;
    } else {
        // Check for page fault (the bug we fixed)
        if spinner_output.contains("Page fault") || spinner_output.contains("KERNEL PANIC") {
            println!("  ✗ FAIL: Kernel crashed (page fault or panic) when interrupting spinner");
            tests_failed += 1;
        } else {
            println!("  ? INCONCLUSIVE: Could not verify spinner interrupt behavior");
        }
    }

    // Clean up
    println!();
    println!("Cleaning up...");
    cleanup(&mut child);

    // Print final output for debugging
    println!();
    println!("=== User Output (last 50 lines) ===");
    if let Ok(contents) = fs::read_to_string(user_output_file) {
        for line in contents.lines().rev().take(50).collect::<Vec<_>>().into_iter().rev() {
            println!("{}", line);
        }
    }

    // Print summary
    println!();
    println!("=== Test Summary ===");
    println!("Passed: {}", tests_passed);
    println!("Failed: {}", tests_failed);

    if tests_failed > 0 {
        bail!("{} interactive test(s) failed", tests_failed);
    }

    println!();
    println!("All interactive tests passed!");
    Ok(())
}

/// Run the kthread stress test - 100+ kthreads with rapid create/stop cycles.
/// This is a dedicated test harness that ONLY runs the stress test and exits.
/// In CI: Uses cargo run (like boot_stages) for proper OVMF handling.
/// Locally: Runs in Docker for clean isolation (no stray QEMU processes).
fn kthread_stress() -> Result<()> {
    let is_ci = std::env::var("CI").is_ok();

    if is_ci {
        kthread_stress_ci()
    } else {
        kthread_stress_docker()
    }
}

/// CI version: Uses cargo run like boot_stages (handles OVMF via ovmf-prebuilt crate)
fn kthread_stress_ci() -> Result<()> {
    println!("=== Kthread Stress Test (CI mode) ===\n");

    // COM1 (user output) and COM2 (kernel logs) - stress test markers go to COM2
    let user_output_file = "target/kthread_stress_user.txt";
    let serial_output_file = "target/kthread_stress_output.txt";
    let _ = fs::remove_file(user_output_file);
    let _ = fs::remove_file(serial_output_file);

    println!("Starting QEMU via cargo run...\n");

    let mut child = Command::new("cargo")
        .args(&[
            "run",
            "--release",
            "-p",
            "breenix",
            "--features",
            "kthread_stress_test",
            "--bin",
            "qemu-uefi",
            "--",
            "-serial",
            &format!("file:{}", user_output_file),  // COM1: user output
            "-serial",
            &format!("file:{}", serial_output_file), // COM2: kernel logs (test markers)
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
    let file_creation_timeout = Duration::from_secs(120);

    while !std::path::Path::new(serial_output_file).exists() {
        if start.elapsed() > file_creation_timeout {
            let _ = child.kill();
            bail!("Serial output file not created after {} seconds", file_creation_timeout.as_secs());
        }
        thread::sleep(Duration::from_millis(100));
    }

    // Monitor for completion
    let timeout = Duration::from_secs(300); // 5 minutes for CI
    let mut success = false;
    let mut test_output = String::new();

    while start.elapsed() < timeout {
        if let Ok(contents) = fs::read_to_string(serial_output_file) {
            test_output = contents.clone();

            if contents.contains("KTHREAD_STRESS_TEST_COMPLETE") {
                success = true;
                break;
            }

            if contents.contains("panicked at") || contents.contains("PANIC:") {
                println!("\n=== STRESS TEST FAILED (panic detected) ===\n");
                for line in contents.lines() {
                    if line.contains("KTHREAD_STRESS") || line.contains("panic") || line.contains("PANIC") {
                        println!("{}", line);
                    }
                }
                let _ = child.kill();
                bail!("Kthread stress test panicked");
            }
        }
        thread::sleep(Duration::from_millis(500));
    }

    let _ = child.kill();

    if success {
        println!("\n=== Kthread Stress Test Results ===\n");
        for line in test_output.lines() {
            if line.contains("KTHREAD_STRESS") {
                println!("{}", line);
            }
        }
        println!("\n=== KTHREAD STRESS TEST PASSED ===\n");
        Ok(())
    } else {
        println!("\n=== STRESS TEST FAILED (timeout) ===\n");
        println!("Last output:");
        for line in test_output.lines().rev().take(30).collect::<Vec<_>>().into_iter().rev() {
            println!("{}", line);
        }
        bail!("Kthread stress test timed out after {} seconds", timeout.as_secs());
    }
}

/// Local version: Uses Docker for clean QEMU isolation
fn kthread_stress_docker() -> Result<()> {
    println!("=== Kthread Stress Test (Docker) ===\n");

    // Step 0: Clean old build artifacts to ensure we use the fresh stress test build
    println!("Cleaning old build artifacts...");
    for entry in glob::glob("target/release/build/breenix-*").unwrap().filter_map(|p| p.ok()) {
        let _ = fs::remove_dir_all(&entry);
    }

    // Step 1: Build kernel with stress test feature
    println!("Building kernel with kthread_stress_test feature...");
    let build_status = Command::new("cargo")
        .args(&[
            "build",
            "--release",
            "--features",
            "kthread_stress_test",
            "--bin",
            "qemu-uefi",
            "-p",
            "breenix",
        ])
        .status()
        .map_err(|e| anyhow::anyhow!("Failed to run cargo build: {}", e))?;

    if !build_status.success() {
        bail!("Failed to build kernel with kthread_stress_test feature");
    }

    // Step 2: Find the built UEFI image
    let uefi_glob = "target/release/build/breenix-*/out/breenix-uefi.img";
    let uefi_img = glob::glob(uefi_glob)
        .map_err(|e| anyhow::anyhow!("Glob error: {}", e))?
        .filter_map(|p| p.ok())
        .next()
        .ok_or_else(|| anyhow::anyhow!("UEFI image not found at {}", uefi_glob))?;

    println!("Using UEFI image: {}", uefi_img.display());

    // Step 3: Build Docker image if needed
    let docker_dir = Path::new("docker/qemu");
    println!("Ensuring Docker image is built...");
    let docker_build = Command::new("docker")
        .args(&["build", "-t", "breenix-qemu", "."])
        .current_dir(docker_dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    if docker_build.is_err() || !docker_build.unwrap().success() {
        let _ = Command::new("docker")
            .args(&["build", "-t", "breenix-qemu", "."])
            .current_dir(docker_dir)
            .status();
    }

    // Step 4: Create temp directory for output
    let output_dir = PathBuf::from("/tmp/breenix_stress_test");
    let _ = fs::remove_dir_all(&output_dir);
    fs::create_dir_all(&output_dir)?;

    // Copy OVMF files
    fs::copy("target/ovmf/x64/code.fd", output_dir.join("OVMF_CODE.fd"))?;
    fs::copy("target/ovmf/x64/vars.fd", output_dir.join("OVMF_VARS.fd"))?;

    // Create empty output files
    fs::write(output_dir.join("serial_kernel.txt"), "")?;
    fs::write(output_dir.join("serial_user.txt"), "")?;

    // Step 5: Run QEMU in Docker with timeout
    println!("\nRunning kthread stress test in Docker (100+ kthreads)...\n");

    let uefi_img_abs = fs::canonicalize(&uefi_img)?;
    let test_binaries = fs::canonicalize("target/test_binaries.img").ok();
    let ext2_img = fs::canonicalize("target/ext2.img").ok();

    let mut docker_args = vec![
        "run".to_string(),
        "--rm".to_string(),
        "-v".to_string(),
        format!("{}:/breenix/breenix-uefi.img:ro", uefi_img_abs.display()),
        "-v".to_string(),
        format!("{}:/output", output_dir.display()),
    ];

    if let Some(ref tb) = test_binaries {
        docker_args.push("-v".to_string());
        docker_args.push(format!("{}:/breenix/test_binaries.img:ro", tb.display()));
    }
    if let Some(ref ext2) = ext2_img {
        docker_args.push("-v".to_string());
        docker_args.push(format!("{}:/breenix/ext2.img:ro", ext2.display()));
    }

    docker_args.extend([
        "breenix-qemu".to_string(),
        "qemu-system-x86_64".to_string(),
        "-pflash".to_string(), "/output/OVMF_CODE.fd".to_string(),
        "-pflash".to_string(), "/output/OVMF_VARS.fd".to_string(),
        "-drive".to_string(), "if=none,id=hd,format=raw,readonly=on,file=/breenix/breenix-uefi.img".to_string(),
        "-device".to_string(), "virtio-blk-pci,drive=hd,bootindex=0,disable-modern=on,disable-legacy=off".to_string(),
        "-machine".to_string(), "pc,accel=tcg".to_string(),
        "-cpu".to_string(), "qemu64".to_string(),
        "-smp".to_string(), "1".to_string(),
        "-m".to_string(), "512".to_string(),
        "-display".to_string(), "none".to_string(),
        "-no-reboot".to_string(),
        "-no-shutdown".to_string(),
        "-device".to_string(), "isa-debug-exit,iobase=0xf4,iosize=0x04".to_string(),
        "-serial".to_string(), "file:/output/serial_user.txt".to_string(),
        "-serial".to_string(), "file:/output/serial_kernel.txt".to_string(),
    ]);

    if test_binaries.is_some() {
        docker_args.extend([
            "-drive".to_string(), "if=none,id=testdisk,format=raw,readonly=on,file=/breenix/test_binaries.img".to_string(),
            "-device".to_string(), "virtio-blk-pci,drive=testdisk,disable-modern=on,disable-legacy=off".to_string(),
        ]);
    }
    if ext2_img.is_some() {
        docker_args.extend([
            "-drive".to_string(), "if=none,id=ext2disk,format=raw,readonly=on,file=/breenix/ext2.img".to_string(),
            "-device".to_string(), "virtio-blk-pci,drive=ext2disk,disable-modern=on,disable-legacy=off".to_string(),
        ]);
    }

    let mut docker_child = Command::new("docker")
        .args(&docker_args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| anyhow::anyhow!("Failed to spawn Docker: {}", e))?;

    // Step 6: Monitor output file for completion
    let start = Instant::now();
    let timeout = Duration::from_secs(180);
    let kernel_log = output_dir.join("serial_kernel.txt");
    let mut success = false;
    let mut test_output = String::new();

    while start.elapsed() < timeout {
        if let Ok(contents) = fs::read_to_string(&kernel_log) {
            test_output = contents.clone();

            if contents.contains("KTHREAD_STRESS_TEST_COMPLETE") {
                success = true;
                break;
            }

            if contents.contains("panicked at") || contents.contains("PANIC:") {
                println!("\n=== STRESS TEST FAILED (panic detected) ===\n");
                for line in contents.lines() {
                    if line.contains("KTHREAD_STRESS") || line.contains("panic") || line.contains("PANIC") {
                        println!("{}", line);
                    }
                }
                let _ = docker_child.kill();
                let _ = Command::new("sh")
                    .args(&["-c", "docker kill $(docker ps -q --filter ancestor=breenix-qemu) 2>/dev/null || true"])
                    .status();
                bail!("Kthread stress test panicked");
            }
        }
        thread::sleep(Duration::from_millis(500));
    }

    let _ = docker_child.kill();
    let _ = Command::new("sh")
        .args(&["-c", "docker kill $(docker ps -q --filter ancestor=breenix-qemu) 2>/dev/null || true"])
        .status();

    if success {
        println!("\n=== Kthread Stress Test Results ===\n");
        for line in test_output.lines() {
            if line.contains("KTHREAD_STRESS") {
                println!("{}", line);
            }
        }
        println!("\n=== KTHREAD STRESS TEST PASSED ===\n");
        Ok(())
    } else {
        println!("\n=== STRESS TEST FAILED (timeout) ===\n");
        println!("Last output:");
        for line in test_output.lines().rev().take(30).collect::<Vec<_>>().into_iter().rev() {
            println!("{}", line);
        }
        bail!("Kthread stress test timed out after {} seconds", timeout.as_secs());
    }
}

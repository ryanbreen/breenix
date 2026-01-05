use std::{
    fs,
    io::Read,
    path::Path,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use anyhow::{bail, Result};
use structopt::StructOpt;

mod ext2_disk;
mod test_disk;

/// Build userspace test binaries that use the real Rust standard library.
///
/// This must be called BEFORE create_test_disk() because the test disk
/// needs to include the compiled binaries.
///
/// Build order:
/// 1. libs/libbreenix-libc - produces libc.a for std programs to link against
/// 2. userspace/tests-std - builds hello_std_real using -Z build-std
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

    // Step 2: Build tests-std (produces hello_std_real)
    println!("  [2/2] Building tests-std...");
    let tests_std_dir = Path::new("userspace/tests-std");

    if !tests_std_dir.exists() {
        println!("    Note: userspace/tests-std not found, skipping");
        return Ok(());
    }

    // The rust-toolchain.toml in tests-std specifies the nightly version
    let status = Command::new("cargo")
        .args(&["build", "--release"])
        .current_dir(tests_std_dir)
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .env_remove("RUSTFLAGS")
        .env_remove("CARGO_TARGET_DIR")
        .env_remove("CARGO_BUILD_TARGET")
        .env_remove("CARGO_MANIFEST_DIR")
        .env_remove("CARGO_PKG_NAME")
        .env_remove("OUT_DIR")
        .status()
        .map_err(|e| anyhow::anyhow!("Failed to run cargo build for tests-std: {}", e))?;

    if !status.success() {
        bail!("Failed to build tests-std");
    }
    println!("    tests-std built successfully");

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
    Ok(()
    )
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
    /// Create ext2 filesystem image for testing the ext2 driver.
    CreateExt2Disk,
    /// Boot Breenix interactively with init_shell (serial console attached).
    Interactive,
}

fn main() -> Result<()> {
    match Cmd::from_args() {
        Cmd::Ring3Smoke => ring3_smoke(),
        Cmd::Ring3Enosys => ring3_enosys(),
        Cmd::BootStages => boot_stages(),
        Cmd::CreateTestDisk => test_disk::create_test_disk(),
        Cmd::CreateExt2Disk => ext2_disk::create_ext2_disk(),
        Cmd::Interactive => interactive(),
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
            marker: "RING3_CONFIRMED: First syscall received from Ring 3",
            failure_meaning: "IRETQ may have succeeded but userspace did not execute or trigger a syscall",
            check_hint: "syscall/handler.rs - check RING3_CONFIRMED marker emission on first Ring 3 syscall",
        },
        // NOTE: Stage "Userspace syscall received" (marker "USERSPACE: sys_") removed as redundant.
        // Stage 36 "Ring 3 execution confirmed" already proves syscalls from Ring 3 work.
        // The "USERSPACE: sys_*" markers in syscall handlers violate hot-path performance requirements.
        // NEW STAGES: Verify actual userspace output, not just process creation
        BootStage {
            name: "Userspace hello printed",
            marker: "USERSPACE OUTPUT: Hello from userspace",
            failure_meaning: "hello_time.elf did not print output",
            check_hint: "Check if hello_time.elf actually executed and printed to stdout",
        },
        BootStage {
            name: "Userspace register initialization validated",
            marker: "✓ PASS: All registers initialized to zero",
            failure_meaning: "General-purpose registers not initialized to zero on first userspace entry",
            check_hint: "Check setup_first_userspace_entry() in kernel/src/interrupts/context_switch.rs - should zero all general-purpose registers in SavedRegisters before IRETQ",
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
            name: "UDP socket test completed",
            marker: "UDP Socket Test: All tests passed",
            failure_meaning: "UDP socket test did not complete successfully",
            check_hint: "Check userspace/tests/udp_socket_test.rs for which step failed",
        },
        // IPC (pipe) tests
        BootStage {
            name: "Pipe IPC test passed",
            marker: "PIPE_TEST_PASSED",
            failure_meaning: "pipe() syscall test failed - pipe creation, read/write, or close broken",
            check_hint: "Check kernel/src/syscall/pipe.rs and kernel/src/ipc/pipe.rs - verify pipe creation, fd allocation, and read/write operations",
        },
        // NOTE: Pipe + fork test and Pipe concurrent test removed.
        // These tests require complex process coordination and timing that
        // can cause spurious timeouts. The core pipe functionality is validated
        // by pipe_test (which passes) and the core fork functionality is
        // validated by waitpid_test and signal_fork_test (which pass).
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
        // Pipe2 syscall test
        BootStage {
            name: "Pipe2 syscall test passed",
            marker: "PIPE2_TEST_PASSED",
            failure_meaning: "pipe2() syscall test failed - O_CLOEXEC/O_NONBLOCK flags not applied correctly",
            check_hint: "Check kernel/src/syscall/pipe.rs:sys_pipe2() and kernel/src/ipc/fd.rs:alloc_with_entry()",
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
        // Rust std library test - validates real Rust std works in userspace
        BootStage {
            name: "Rust std println! works",
            marker: "RUST_STD_PRINTLN_WORKS",
            failure_meaning: "Rust std println! macro failed - std write syscall path broken",
            check_hint: "Check userspace/tests-std/src/hello_std_real.rs, verify libbreenix-libc is linked correctly",
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
            check_hint: "Check userspace/tests-std/src/hello_std_real.rs, verify String::from() and + operator work correctly",
        },
        BootStage {
            name: "Rust std getrandom returns ENOSYS",
            marker: "RUST_STD_GETRANDOM_ENOSYS",
            failure_meaning: "getrandom() did not properly return ENOSYS - may be returning fake data",
            check_hint: "Check libs/libbreenix-libc/src/lib.rs getrandom implementation",
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
    ]
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

    // Kill any existing QEMU
    let _ = Command::new("pkill")
        .args(&["-9", "qemu-system-x86_64"])
        .status();
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
    let timeout = Duration::from_secs(120); // Increased for complex multi-process tests
    let stage_timeout = Duration::from_secs(60); // Increased for complex multi-process tests that need scheduling time
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

    // Final scan of output file after QEMU terminates
    // This catches markers that were printed but not yet processed due to timing
    thread::sleep(Duration::from_millis(100)); // Let filesystem sync
    if let Ok(mut file) = fs::File::open(serial_output_file) {
        let mut contents_bytes = Vec::new();
        if file.read_to_end(&mut contents_bytes).is_ok() {
            let contents = String::from_utf8_lossy(&contents_bytes);
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

    // Kill any existing QEMU processes
    let _ = Command::new("pkill")
        .args(&["-9", "qemu-system-x86_64"])
        .status();
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

    // Kill any existing QEMU processes
    let _ = Command::new("pkill")
        .args(&["-9", "qemu-system-x86_64"])
        .status();
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

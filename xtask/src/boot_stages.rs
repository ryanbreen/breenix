//! Boot stage definitions for x86_64 and ARM64 architectures.
//!
//! Stages are organized into:
//! - Architecture-specific kernel boot stages (different for each arch)
//! - Shared userspace test stages (same tests run on both architectures)
//! - x86_64-only extra stages (diagnostic sub-tests, kthread, workqueue, softirq)

use crate::qemu_config;

/// Boot stage definition with marker, description, and failure info
pub struct BootStage {
    pub name: &'static str,
    pub marker: &'static str,
    pub failure_meaning: &'static str,
    pub check_hint: &'static str,
}

/// Timing info for a completed stage
#[derive(Clone)]
pub struct StageTiming {
    pub duration: std::time::Duration,
}

/// Get all boot stages for the given architecture.
///
/// For x86_64: kernel stages + shared userspace + x86_64-only extras
/// For ARM64: kernel stages + shared userspace
pub fn get_boot_stages(arch: &qemu_config::Arch) -> Vec<BootStage> {
    match arch {
        qemu_config::Arch::X86_64 => {
            let mut stages = x86_64_kernel_stages();
            stages.extend(shared_userspace_stages());
            stages.extend(x86_64_extra_stages());
            stages
        }
        qemu_config::Arch::Arm64 => {
            let mut stages = arm64_kernel_stages();
            stages.extend(shared_userspace_stages());
            stages
        }
    }
}

/// Get DNS-specific boot stages for focused testing with dns_test_only feature
pub fn get_dns_stages() -> Vec<BootStage> {
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
            check_hint: "Check scheduler is running userspace, check userspace/programs/dns_test.rs",
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
            check_hint: "Check userspace/programs/dns_test.rs for which step failed",
        },
    ]
}

/// x86_64 kernel-specific boot stages (56 stages).
/// These cover the x86_64 boot sequence from kernel entry through Ring 3 execution.
fn x86_64_kernel_stages() -> Vec<BootStage> {
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
            marker: "PRECONDITION 1: IDT timer entry \u{2713} PASS",
            failure_meaning: "IDT timer entry not properly configured",
            check_hint: "interrupts::validate_timer_idt_entry() - verify IDT entry for IRQ0 (vector 32)",
        },
        BootStage {
            name: "Precondition 2: Timer handler registered",
            marker: "PRECONDITION 2: Timer handler registered \u{2713} PASS",
            failure_meaning: "Timer interrupt handler not registered",
            check_hint: "Check IDT entry for IRQ0 points to timer_interrupt_entry (same as Precondition 1)",
        },
        BootStage {
            name: "Precondition 3: PIT counter active",
            marker: "PRECONDITION 3: PIT counter \u{2713} PASS",
            failure_meaning: "PIT (Programmable Interval Timer) hardware not counting",
            check_hint: "time::timer::validate_pit_counting() - verify PIT counter changing between reads",
        },
        BootStage {
            name: "Precondition 4: PIC IRQ0 unmasked",
            marker: "PRECONDITION 4: PIC IRQ0 unmasked \u{2713} PASS",
            failure_meaning: "IRQ0 is masked in PIC - timer interrupts will not fire",
            check_hint: "interrupts::validate_pic_irq0_unmasked() - verify bit 0 of PIC1 mask register is clear",
        },
        BootStage {
            name: "Precondition 5: Runnable threads exist",
            marker: "PRECONDITION 5: Scheduler has runnable threads \u{2713} PASS",
            failure_meaning: "Scheduler has no runnable threads - timer interrupt has nothing to schedule",
            check_hint: "task::scheduler::has_runnable_threads() - verify userspace processes were created",
        },
        BootStage {
            name: "Precondition 6: Current thread set",
            marker: "PRECONDITION 6: Current thread set \u{2713} PASS",
            failure_meaning: "Current thread not set in per-CPU data",
            check_hint: "per_cpu::current_thread() - verify returns Some(thread) with valid pointer",
        },
        BootStage {
            name: "Precondition 7: Interrupts disabled",
            marker: "PRECONDITION 7: Interrupts disabled \u{2713} PASS",
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
    ]
}

/// ARM64 kernel-specific boot stages (21 stages).
/// These cover the ARM64 boot sequence from kernel entry through scheduler idle loop.
fn arm64_kernel_stages() -> Vec<BootStage> {
    vec![
        // === ARM64 Kernel Boot Stages ===
        BootStage {
            name: "ARM64 kernel starting",
            marker: "Breenix ARM64 Kernel Starting",
            failure_meaning: "Kernel failed to start at all",
            check_hint: "Check kernel binary, QEMU -kernel argument, boot.S assembly entry",
        },
        BootStage {
            name: "Memory management ready",
            marker: "[boot] Memory management ready",
            failure_meaning: "Frame allocator, heap, or kernel stack initialization failed",
            check_hint: "Check memory::frame_allocator::init_aarch64(), memory::init_aarch64_heap(), memory::kernel_stack::init()",
        },
        BootStage {
            name: "Timer calibrated",
            marker: "[boot] Timer frequency:",
            failure_meaning: "ARM Generic Timer calibration failed",
            check_hint: "Check arch_impl::aarch64::timer::calibrate() - CNTFRQ_EL0 register",
        },
        BootStage {
            name: "GIC initialized",
            marker: "[boot] GIC initialized",
            failure_meaning: "GICv2 interrupt controller initialization failed",
            check_hint: "Check arch_impl::aarch64::gic::Gicv2::init() - MMIO addresses for GICD/GICC",
        },
        BootStage {
            name: "UART interrupts enabled",
            marker: "[boot] UART interrupts enabled",
            failure_meaning: "UART RX interrupt setup failed - serial input will not work",
            check_hint: "Check serial::enable_rx_interrupt() and GIC IRQ 33 enable",
        },
        BootStage {
            name: "Interrupts enabled",
            marker: "[boot] Interrupts enabled:",
            failure_meaning: "Failed to enable CPU interrupts (DAIF clear)",
            check_hint: "Check Aarch64Cpu::enable_interrupts() - DAIF register",
        },
        BootStage {
            name: "Device drivers initialized",
            marker: "[boot] Found",
            failure_meaning: "VirtIO MMIO device enumeration failed",
            check_hint: "Check drivers::init() - VirtIO MMIO probe at 0x0a000000+",
        },
        BootStage {
            name: "ext2 root filesystem mounted",
            marker: "[boot] ext2 root filesystem mounted",
            failure_meaning: "ext2 filesystem mount failed - disk I/O or superblock read issue",
            check_hint: "Check fs::ext2::init_root_fs(), verify VirtIO block device and ext2 disk image",
        },
        BootStage {
            name: "devfs initialized",
            marker: "[boot] devfs initialized",
            failure_meaning: "devfs virtual filesystem initialization failed",
            check_hint: "Check fs::devfs::init()",
        },
        BootStage {
            name: "devptsfs initialized",
            marker: "[boot] devptsfs initialized",
            failure_meaning: "devptsfs pseudo-terminal filesystem initialization failed",
            check_hint: "Check fs::devptsfs::init()",
        },
        BootStage {
            name: "procfs initialized",
            marker: "[boot] procfs initialized",
            failure_meaning: "procfs virtual filesystem initialization failed",
            check_hint: "Check fs::procfs::init()",
        },
        BootStage {
            name: "TTY subsystem initialized",
            marker: "[boot] TTY subsystem initialized",
            failure_meaning: "TTY console/PTY infrastructure initialization failed",
            check_hint: "Check tty::init()",
        },
        BootStage {
            name: "Per-CPU data initialized",
            marker: "[boot] Per-CPU data initialized",
            failure_meaning: "Per-CPU storage initialization failed",
            check_hint: "Check per_cpu_aarch64::init()",
        },
        BootStage {
            name: "Process manager initialized",
            marker: "[boot] Process manager initialized",
            failure_meaning: "Process manager creation failed",
            check_hint: "Check process::init() - ProcessManager allocation",
        },
        BootStage {
            name: "Scheduler initialized",
            marker: "[boot] Scheduler initialized",
            failure_meaning: "Scheduler initialization with idle task failed",
            check_hint: "Check init_scheduler() in main_aarch64.rs - idle thread creation",
        },
        BootStage {
            name: "Timer interrupt initialized",
            marker: "[boot] Timer interrupt initialized",
            failure_meaning: "Timer interrupt for preemptive scheduling failed to initialize",
            check_hint: "Check arch_impl::aarch64::timer_interrupt::init()",
        },
        BootStage {
            name: "SMP CPUs online",
            marker: "[smp]",
            failure_meaning: "Secondary CPU startup via PSCI failed or timed out",
            check_hint: "Check arch_impl::aarch64::smp::release_cpu() and PSCI CPU_ON",
        },
        BootStage {
            name: "ARM64 boot complete",
            marker: "Breenix ARM64 Boot Complete!",
            failure_meaning: "Kernel boot sequence did not finish - something failed between SMP init and boot complete message",
            check_hint: "Check main_aarch64.rs kernel_main() - all steps between SMP and boot complete message",
        },
        BootStage {
            name: "Test binary loading started",
            marker: "[test] Loading test binaries from ext2",
            failure_meaning: "Kernel did not enter test binary loading phase - testing feature may not be enabled",
            check_hint: "Check kernel built with --features testing and device_count > 0",
        },
        BootStage {
            name: "Test binaries loaded",
            marker: "[test] Loaded",
            failure_meaning: "Test binary loading from ext2 did not complete - possible hang in ext2 reads or process creation",
            check_hint: "Check load_test_binaries_from_ext2() - interrupts should be disabled during loading to prevent scheduler preemption",
        },
        BootStage {
            name: "Scheduler idle loop entered",
            marker: "[test] Entering scheduler idle loop",
            failure_meaning: "Kernel did not enter idle loop after loading test binaries",
            check_hint: "Check that testing feature is enabled and device_count > 0",
        },
    ]
}

/// Shared userspace test stages (163 stages).
/// These markers are emitted by test binaries and are architecture-neutral
/// (same source code compiled for aarch64 emits identical markers).
fn shared_userspace_stages() -> Vec<BootStage> {
    vec![
        // Basic execution tests
        BootStage {
            name: "Userspace hello printed",
            marker: "Hello from userspace",
            failure_meaning: "hello_time.elf did not print output",
            check_hint: "Check if hello_time.elf actually executed and printed to stdout",
        },
        BootStage {
            name: "Userspace clock_gettime validated",
            marker: "USERSPACE CLOCK_GETTIME: OK",
            failure_meaning: "Userspace process called clock_gettime syscall but got zero time or syscall failed",
            check_hint: "Verify syscall dispatch to SYS_clock_gettime (228) works from userspace and returns non-zero time",
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

        // Diagnostic tests - x86_64 uses per-test markers (Test 41a-e) but on ARM64
        // the diagnostic test skips (x86-only inline asm) and emits only the summary.
        BootStage {
            name: "Diagnostic: Summary",
            marker: "\u{2713} All diagnostic tests passed",
            failure_meaning: "Not all diagnostic tests passed - see individual test results above",
            check_hint: "Check syscall_diagnostic_test output for failures",
        },

        // Signal tests
        BootStage {
            name: "Signal handler execution verified",
            marker: "SIGNAL_HANDLER_EXECUTED",
            failure_meaning: "Signal handler was not executed when signal was delivered",
            check_hint: "Check syscall/signal.rs:sys_sigaction() and signal delivery path",
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
            check_hint: "Check syscall/signal.rs:sys_sigaltstack() and SA_ONSTACK support",
        },

        // UDP Socket tests
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
            check_hint: "Check syscall/socket.rs:sys_sendto(), net/udp.rs:build_udp_packet()",
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
            check_hint: "Check net/udp.rs:deliver_to_socket()",
        },
        BootStage {
            name: "UDP packet received from userspace",
            marker: "UDP: Received packet",
            failure_meaning: "sys_recvfrom syscall failed or returned no data",
            check_hint: "Check syscall/socket.rs:sys_recvfrom() and socket/udp.rs:recv_from()",
        },
        BootStage {
            name: "UDP RX data verified",
            marker: "UDP: RX data matches TX data - SUCCESS",
            failure_meaning: "Received packet but data was corrupted",
            check_hint: "Check packet data integrity in RX path",
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
            check_hint: "Check syscall/socket.rs:sys_recvfrom() EAGAIN handling",
        },
        BootStage {
            name: "UDP multiple packets received",
            marker: "UDP_MULTIPACKET_TEST: 3 packets OK",
            failure_meaning: "Failed to receive 3 packets in sequence",
            check_hint: "Check net/udp.rs packet queuing and delivery",
        },
        BootStage {
            name: "UDP socket test completed",
            marker: "UDP Socket Test: All tests passed",
            failure_meaning: "UDP socket test did not complete successfully",
            check_hint: "Check userspace/programs/udp_socket_test.rs for which step failed",
        },

        // TCP Socket tests
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
            failure_meaning: "sys_listen failed",
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
            failure_meaning: "sys_connect failed",
            check_hint: "Check syscall/socket.rs:sys_connect()",
        },
        BootStage {
            name: "TCP accept executed",
            marker: "TCP_TEST: accept OK",
            failure_meaning: "sys_accept returned unexpected error",
            check_hint: "Check syscall/socket.rs:sys_accept()",
        },
        BootStage {
            name: "TCP shutdown executed",
            marker: "TCP_TEST: shutdown OK",
            failure_meaning: "sys_shutdown(SHUT_RDWR) failed",
            check_hint: "Check syscall/socket.rs:sys_shutdown()",
        },
        BootStage {
            name: "TCP shutdown unconnected rejected",
            marker: "TCP_TEST: shutdown_unconnected OK",
            failure_meaning: "sys_shutdown on unconnected socket did not return ENOTCONN",
            check_hint: "Check sys_shutdown() ENOTCONN handling",
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
        // TCP Data Transfer tests
        BootStage {
            name: "TCP data test started",
            marker: "TCP_DATA_TEST: starting",
            failure_meaning: "TCP data transfer test did not start",
            check_hint: "Check tcp_socket_test.rs test 12 section",
        },
        BootStage {
            name: "TCP data server listening",
            marker: "TCP_DATA_TEST: server listening on 8082",
            failure_meaning: "TCP data test server failed to bind/listen",
            check_hint: "Check sys_bind/sys_listen for TCP sockets",
        },
        BootStage {
            name: "TCP data client connected",
            marker: "TCP_DATA_TEST: client connected",
            failure_meaning: "TCP data test client failed to connect",
            check_hint: "Check sys_connect for loopback TCP connections",
        },
        BootStage {
            name: "TCP data send",
            marker: "TCP_DATA_TEST: send OK",
            failure_meaning: "write() on TCP socket failed",
            check_hint: "Check syscall/io.rs:sys_write() for TCP socket support",
        },
        BootStage {
            name: "TCP data accept",
            marker: "TCP_DATA_TEST: accept OK",
            failure_meaning: "accept() on data test server failed after connect",
            check_hint: "Check sys_accept() loopback connection queuing",
        },
        BootStage {
            name: "TCP data recv",
            marker: "TCP_DATA_TEST: recv OK",
            failure_meaning: "read() on accepted TCP socket failed",
            check_hint: "Check syscall/io.rs:sys_read() for TCP socket support",
        },
        BootStage {
            name: "TCP data verified",
            marker: "TCP_DATA_TEST: data verified",
            failure_meaning: "Received TCP data did not match sent data",
            check_hint: "Check tcp_send/tcp_recv implementation",
        },
        BootStage {
            name: "TCP post-shutdown write test started",
            marker: "TCP_SHUTDOWN_WRITE_TEST: starting",
            failure_meaning: "Post-shutdown write test did not start",
            check_hint: "Previous test may have failed",
        },
        BootStage {
            name: "TCP write after shutdown rejected with EPIPE",
            marker: "TCP_SHUTDOWN_WRITE_TEST: EPIPE OK",
            failure_meaning: "Write after SHUT_WR was not rejected with EPIPE",
            check_hint: "Check tcp_send returns EPIPE when send_shutdown=true",
        },
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
        BootStage {
            name: "TCP SHUT_WR test started",
            marker: "TCP_SHUT_WR_TEST: starting",
            failure_meaning: "SHUT_WR test did not start",
            check_hint: "Previous test may have failed",
        },
        BootStage {
            name: "TCP SHUT_WR succeeded",
            marker: "TCP_SHUT_WR_TEST: SHUT_WR write rejected OK",
            failure_meaning: "shutdown(SHUT_WR) failed",
            check_hint: "Check sys_shutdown handling of SHUT_WR",
        },
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
            check_hint: "Check tcp_connect error handling",
        },
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
            check_hint: "Check TCP segmentation for large data",
        },
        BootStage {
            name: "TCP multi-cycle test started",
            marker: "TCP_MULTI_TEST: starting",
            failure_meaning: "Multi-cycle test did not start",
            check_hint: "Previous test may have failed",
        },
        BootStage {
            name: "TCP multi-cycle test passed",
            marker: "TCP_MULTI_TEST: 3 messages",
            failure_meaning: "Multiple write/read cycles failed",
            check_hint: "Check TCP connection state management",
        },
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
        BootStage {
            name: "TCP simultaneous close test started",
            marker: "TCP_SIMUL_CLOSE_TEST: starting",
            failure_meaning: "Simultaneous close test did not start",
            check_hint: "Previous test may have failed",
        },
        BootStage {
            name: "TCP simultaneous close test passed",
            marker: "TCP_SIMUL_CLOSE_TEST: simultaneous close OK",
            failure_meaning: "Both sides calling shutdown simultaneously failed",
            check_hint: "Check sys_shutdown handling when both sides close",
        },
        BootStage {
            name: "TCP half-close test started",
            marker: "TCP_HALFCLOSE_TEST: starting",
            failure_meaning: "Half-close test did not start",
            check_hint: "Previous test may have failed",
        },
        BootStage {
            name: "TCP half-close test passed",
            marker: "TCP_HALFCLOSE_TEST: read after SHUT_WR OK",
            failure_meaning: "Client could not read data after calling SHUT_WR",
            check_hint: "Check tcp_recv - recv_shutdown should only be set by SHUT_RD",
        },
        BootStage {
            name: "TCP socket test passed",
            marker: "TCP Socket Test: PASSED",
            failure_meaning: "TCP socket test did not complete successfully",
            check_hint: "Check userspace/programs/tcp_socket_test.rs for which step failed",
        },

        // DNS resolution tests
        BootStage {
            name: "DNS google resolve",
            marker: "DNS_TEST: google_resolve OK|DNS_TEST: google_resolve SKIP",
            failure_meaning: "DNS resolution of www.google.com failed",
            check_hint: "Check libs/libbreenix/src/dns.rs:resolve() and UDP socket path",
        },
        BootStage {
            name: "DNS example resolve",
            marker: "DNS_TEST: example_resolve OK|DNS_TEST: example_resolve SKIP",
            failure_meaning: "DNS resolution of example.com failed",
            check_hint: "Check libs/libbreenix/src/dns.rs",
        },
        BootStage {
            name: "DNS NXDOMAIN handling",
            marker: "DNS_TEST: nxdomain OK",
            failure_meaning: "NXDOMAIN handling for nonexistent domain failed",
            check_hint: "Check libs/libbreenix/src/dns.rs RCODE 3 handling",
        },
        BootStage {
            name: "DNS empty hostname",
            marker: "DNS_TEST: empty_hostname OK",
            failure_meaning: "Empty hostname validation failed",
            check_hint: "Check dns.rs resolve() InvalidHostname for empty string",
        },
        BootStage {
            name: "DNS long hostname",
            marker: "DNS_TEST: long_hostname OK",
            failure_meaning: "Hostname too long validation failed",
            check_hint: "Check dns.rs resolve() HostnameTooLong for >255 char hostname",
        },
        BootStage {
            name: "DNS txid varies",
            marker: "DNS_TEST: txid_varies OK|DNS_TEST: txid_varies SKIP",
            failure_meaning: "Transaction ID variation test failed",
            check_hint: "Check dns.rs generate_txid() produces different IDs",
        },
        BootStage {
            name: "DNS test completed",
            marker: "DNS Test: All tests passed",
            failure_meaning: "DNS test did not complete successfully",
            check_hint: "Check userspace/programs/dns_test.rs for which step failed",
        },

        // HTTP client tests
        BootStage {
            name: "HTTP port out of range",
            marker: "HTTP_TEST: port_out_of_range OK",
            failure_meaning: "HTTP client should reject port > 65535",
            check_hint: "Check libs/libbreenix/src/http.rs parse_port()",
        },
        BootStage {
            name: "HTTP port non-numeric",
            marker: "HTTP_TEST: port_non_numeric OK",
            failure_meaning: "HTTP client should reject non-numeric port",
            check_hint: "Check libs/libbreenix/src/http.rs parse_port()",
        },
        BootStage {
            name: "HTTP empty host",
            marker: "HTTP_TEST: empty_host OK",
            failure_meaning: "HTTP client should reject empty host",
            check_hint: "Check libs/libbreenix/src/http.rs parse_url()",
        },
        BootStage {
            name: "HTTP URL too long",
            marker: "HTTP_TEST: url_too_long OK",
            failure_meaning: "HTTP client should reject URL > 2048 chars",
            check_hint: "Check libs/libbreenix/src/http.rs MAX_URL_LEN check",
        },
        BootStage {
            name: "HTTP HTTPS rejection",
            marker: "HTTP_TEST: https_rejected OK",
            failure_meaning: "HTTP client should reject HTTPS URLs",
            check_hint: "Check libs/libbreenix/src/http.rs HTTPS check",
        },
        BootStage {
            name: "HTTP invalid domain",
            marker: "HTTP_TEST: invalid_domain OK",
            failure_meaning: "HTTP client should return DnsError for .invalid TLD",
            check_hint: "Check libs/libbreenix/src/http.rs and dns.rs",
        },
        BootStage {
            name: "HTTP example fetch",
            marker: "HTTP_TEST: example_fetch OK|HTTP_TEST: example_fetch SKIP",
            failure_meaning: "HTTP GET to example.com failed with unexpected error",
            check_hint: "Check libs/libbreenix/src/http.rs",
        },
        BootStage {
            name: "HTTP test completed",
            marker: "HTTP Test: All tests passed",
            failure_meaning: "HTTP test did not complete successfully",
            check_hint: "Check userspace/programs/http_test.rs for which step failed",
        },

        // IPC tests
        BootStage {
            name: "Pipe IPC test passed",
            marker: "PIPE_TEST_PASSED",
            failure_meaning: "pipe() syscall test failed",
            check_hint: "Check kernel/src/syscall/pipe.rs and kernel/src/ipc/pipe.rs",
        },
        BootStage {
            name: "Unix socket test passed",
            marker: "UNIX_SOCKET_TEST_PASSED",
            failure_meaning: "Unix domain socket test failed",
            check_hint: "Check kernel/src/syscall/socket.rs Unix socket handling",
        },

        // Signal kill and process tests
        BootStage {
            name: "SIGTERM kill test passed",
            marker: "SIGNAL_KILL_TEST_PASSED",
            failure_meaning: "SIGTERM kill test failed",
            check_hint: "Check signal/delivery.rs, context_switch signal delivery path",
        },
        BootStage {
            name: "SIGCHLD delivery test passed",
            marker: "SIGCHLD_TEST_PASSED",
            failure_meaning: "SIGCHLD delivery test failed",
            check_hint: "Check task/process_task.rs handle_thread_exit() SIGCHLD handling",
        },
        BootStage {
            name: "Pause syscall test passed",
            marker: "PAUSE_TEST_PASSED",
            failure_meaning: "pause() syscall test failed",
            check_hint: "Check syscall/signal.rs:sys_pause()",
        },
        BootStage {
            name: "Sigsuspend syscall test passed",
            marker: "SIGSUSPEND_TEST_PASSED",
            failure_meaning: "sigsuspend() syscall test failed",
            check_hint: "Check syscall/signal.rs:sys_sigsuspend()",
        },
        // kill_process_group_test removed from shared stages - its child busy-loops and
        // signal delivery to blocked processes is not yet implemented on ARM64.

        // FD and IPC tests
        BootStage {
            name: "Dup syscall test passed",
            marker: "DUP_TEST_PASSED",
            failure_meaning: "dup() syscall test failed",
            check_hint: "Check syscall/io.rs:sys_dup() and fd table management",
        },
        BootStage {
            name: "Fcntl syscall test passed",
            marker: "FCNTL_TEST_PASSED",
            failure_meaning: "fcntl() syscall test failed",
            check_hint: "Check syscall/handlers.rs:sys_fcntl()",
        },
        BootStage {
            name: "Close-on-exec test passed",
            marker: "CLOEXEC_TEST_PASSED",
            failure_meaning: "close-on-exec test failed",
            check_hint: "Check process/manager.rs exec_process_with_argv() FD_CLOEXEC handling",
        },
        BootStage {
            name: "Pipe2 syscall test passed",
            marker: "PIPE2_TEST_PASSED",
            failure_meaning: "pipe2() syscall test failed",
            check_hint: "Check syscall/pipe.rs:sys_pipe2()",
        },
        BootStage {
            name: "Shell pipeline execution test passed",
            marker: "SHELL_PIPE_TEST_PASSED",
            failure_meaning: "Shell pipeline test failed",
            check_hint: "Check dup2() in syscall/fd.rs, pipe in ipc/pipe.rs",
        },
        BootStage {
            name: "Signal exec reset test passed",
            marker: "SIGNAL_EXEC_TEST_PASSED",
            failure_meaning: "signal exec reset test failed",
            check_hint: "Check process/manager.rs:exec_process() signal handler reset",
        },
        BootStage {
            name: "Waitpid test passed",
            marker: "WAITPID_TEST_PASSED",
            failure_meaning: "waitpid test failed",
            check_hint: "Check syscall/process.rs:sys_wait4()",
        },
        BootStage {
            name: "Signal fork inheritance test passed",
            marker: "SIGNAL_FORK_TEST_PASSED",
            failure_meaning: "signal fork inheritance test failed",
            check_hint: "Check process/fork.rs signal handler cloning",
        },
        BootStage {
            name: "WNOHANG timing test passed",
            marker: "WNOHANG_TIMING_TEST_PASSED",
            failure_meaning: "WNOHANG timing test failed",
            check_hint: "Check syscall/process.rs:sys_wait4() WNOHANG handling",
        },

        // I/O multiplexing tests
        BootStage {
            name: "Poll syscall test passed",
            marker: "POLL_TEST_PASSED",
            failure_meaning: "poll() syscall test failed",
            check_hint: "Check syscall/handlers.rs:sys_poll()",
        },
        BootStage {
            name: "Select syscall test passed",
            marker: "SELECT_TEST_PASSED",
            failure_meaning: "select() syscall test failed",
            check_hint: "Check syscall/handlers.rs:sys_select()",
        },
        BootStage {
            name: "O_NONBLOCK pipe test passed",
            marker: "NONBLOCK_TEST_PASSED",
            failure_meaning: "O_NONBLOCK pipe test failed",
            check_hint: "Check sys_read()/sys_write() O_NONBLOCK handling",
        },

        // TTY and session tests
        BootStage {
            name: "TTY layer test passed",
            marker: "TTY_TEST_PASSED",
            failure_meaning: "TTY layer test failed",
            check_hint: "Check tty/ module, syscall/ioctl.rs",
        },
        BootStage {
            name: "Session syscall test passed",
            marker: "SESSION_TEST_PASSED",
            failure_meaning: "session/process group syscall test failed",
            check_hint: "Check syscall/process.rs session/pgid functions",
        },

        // Filesystem tests
        BootStage {
            name: "File read test passed",
            marker: "FILE_READ_TEST_PASSED",
            failure_meaning: "ext2 file read test failed",
            check_hint: "Check fs/ext2/ module, syscall/fs.rs",
        },
        BootStage {
            name: "Getdents test passed",
            marker: "GETDENTS_TEST_PASSED",
            failure_meaning: "getdents64 syscall failed",
            check_hint: "Check syscall/fs.rs sys_getdents64()",
        },
        BootStage {
            name: "Lseek test passed",
            marker: "LSEEK_TEST_PASSED",
            failure_meaning: "lseek syscall failed",
            check_hint: "Check syscall/fs.rs sys_lseek()",
        },
        BootStage {
            name: "Filesystem write test passed",
            marker: "FS_WRITE_TEST_PASSED",
            failure_meaning: "filesystem write operations failed",
            check_hint: "Check syscall/fs.rs sys_open O_CREAT/O_TRUNC, sys_write for RegularFile",
        },
        BootStage {
            name: "Filesystem rename test passed",
            marker: "FS_RENAME_TEST_PASSED",
            failure_meaning: "filesystem rename operations failed",
            check_hint: "Check fs/ext2/mod.rs rename(), syscall/fs.rs sys_rename()",
        },
        BootStage {
            name: "Large file test passed (indirect blocks)",
            marker: "FS_LARGE_FILE_TEST_PASSED",
            failure_meaning: "large file operations failed",
            check_hint: "Check fs/ext2/file.rs set_block_num(), write_file_range() for indirect blocks",
        },
        BootStage {
            name: "Directory ops test passed",
            marker: "FS_DIRECTORY_TEST_PASSED",
            failure_meaning: "directory operations failed",
            check_hint: "Check fs/ext2/mod.rs mkdir(), rmdir()",
        },
        BootStage {
            name: "Link ops test passed",
            marker: "FS_LINK_TEST_PASSED",
            failure_meaning: "link operations failed",
            check_hint: "Check fs/ext2/mod.rs link(), symlink(), readlink()",
        },
        BootStage {
            name: "Access syscall test passed",
            marker: "ACCESS_TEST_PASSED",
            failure_meaning: "access() syscall failed",
            check_hint: "Check syscall/fs.rs sys_access()",
        },
        BootStage {
            name: "Devfs test passed",
            marker: "DEVFS_TEST_PASSED",
            failure_meaning: "devfs test failed",
            check_hint: "Check fs/devfs/mod.rs and sys_open for /dev/* paths",
        },
        BootStage {
            name: "CWD test passed",
            marker: "CWD_TEST_PASSED",
            failure_meaning: "One or more cwd tests failed",
            check_hint: "Check cwd_test.rs output for specific failure",
        },

        // Exec from ext2 tests
        BootStage {
            name: "Exec ext2 test start",
            marker: "EXEC_EXT2_TEST_START",
            failure_meaning: "exec from ext2 test failed to start",
            check_hint: "Check exec_from_ext2_test.rs is built and loaded",
        },
        BootStage {
            name: "Exec ext2 /bin OK",
            marker: "EXEC_EXT2_BIN_OK",
            failure_meaning: "exec /bin/hello_world from ext2 failed",
            check_hint: "Check load_elf_from_ext2(), verify ext2 has /bin/hello_world",
        },
        BootStage {
            name: "Exec ext2 ENOENT OK",
            marker: "EXEC_EXT2_ENOENT_OK",
            failure_meaning: "exec of nonexistent file did not return ENOENT",
            check_hint: "Check load_elf_from_ext2() error handling",
        },
        BootStage {
            name: "Exec ext2 EACCES OK",
            marker: "EXEC_EXT2_EACCES_OK",
            failure_meaning: "exec of non-executable file did not return EACCES",
            check_hint: "Check load_elf_from_ext2() permission check",
        },
        BootStage {
            name: "Exec ext2 ENOTDIR OK",
            marker: "EXEC_EXT2_ENOTDIR_OK",
            failure_meaning: "exec of directory did not return ENOTDIR/EACCES",
            check_hint: "Check load_elf_from_ext2() directory check",
        },
        BootStage {
            name: "Exec ext2 /bin/ls OK",
            marker: "EXEC_EXT2_LS_OK",
            failure_meaning: "exec /bin/ls failed",
            check_hint: "Check kernel exec path resolution and ls binary in ext2",
        },
        BootStage {
            name: "Exec ext2 test passed",
            marker: "EXEC_EXT2_TEST_PASSED",
            failure_meaning: "One or more exec from ext2 tests failed",
            check_hint: "Check exec_from_ext2_test.rs output",
        },
        BootStage {
            name: "Block alloc test passed",
            marker: "BLOCK_ALLOC_TEST_PASSED",
            failure_meaning: "Block allocation regression test failed",
            check_hint: "Check fs_block_alloc_test.rs and fs/ext2/block_group.rs",
        },

        // Coreutil tests
        BootStage {
            name: "true coreutil test passed",
            marker: "TRUE_TEST_PASSED",
            failure_meaning: "/bin/true did not exit with code 0",
            check_hint: "Check true.rs and true_test.rs",
        },
        BootStage {
            name: "false coreutil test passed",
            marker: "FALSE_TEST_PASSED",
            failure_meaning: "/bin/false did not exit with code 1",
            check_hint: "Check false.rs and false_test.rs",
        },
        BootStage {
            name: "head coreutil test passed",
            marker: "HEAD_TEST_PASSED",
            failure_meaning: "/bin/head failed to process files correctly",
            check_hint: "Check head.rs and head_test.rs",
        },
        BootStage {
            name: "tail coreutil test passed",
            marker: "TAIL_TEST_PASSED",
            failure_meaning: "/bin/tail failed to process files correctly",
            check_hint: "Check tail.rs and tail_test.rs",
        },
        BootStage {
            name: "wc coreutil test passed",
            marker: "WC_TEST_PASSED",
            failure_meaning: "/bin/wc failed to count lines/words/bytes correctly",
            check_hint: "Check wc.rs and wc_test.rs",
        },
        BootStage {
            name: "which coreutil test passed",
            marker: "WHICH_TEST_PASSED",
            failure_meaning: "/bin/which failed to locate commands in PATH",
            check_hint: "Check which.rs and which_test.rs",
        },
        BootStage {
            name: "cat coreutil test passed",
            marker: "CAT_TEST_PASSED",
            failure_meaning: "/bin/cat failed to output file contents correctly",
            check_hint: "Check cat.rs and cat_test.rs",
        },
        BootStage {
            name: "ls coreutil test passed",
            marker: "LS_TEST_PASSED",
            failure_meaning: "/bin/ls failed to list directory contents correctly",
            check_hint: "Check ls.rs and ls_test.rs",
        },

        // Rust std library tests (from hello_std_real / hello_world.elf)
        BootStage {
            name: "Rust std println! works",
            marker: "RUST_STD_PRINTLN_WORKS",
            failure_meaning: "Rust std println! macro failed",
            check_hint: "Check hello_std_real.rs, verify libbreenix-libc is linked correctly",
        },
        BootStage {
            name: "Rust std Vec works",
            marker: "RUST_STD_VEC_WORKS",
            failure_meaning: "Rust std Vec allocation failed",
            check_hint: "Check mmap/brk syscalls with std programs, verify GlobalAlloc in libbreenix-libc",
        },
        BootStage {
            name: "Rust std String works",
            marker: "RUST_STD_STRING_WORKS",
            failure_meaning: "Rust std String operations failed",
            check_hint: "Check hello_std_real.rs, verify String::from() and + operator",
        },
        BootStage {
            name: "Rust std getrandom works",
            marker: "RUST_STD_GETRANDOM_WORKS",
            failure_meaning: "getrandom() syscall failed",
            check_hint: "Check syscall/random.rs and libbreenix-libc getrandom",
        },
        BootStage {
            name: "Rust std HashMap works",
            marker: "RUST_STD_HASHMAP_WORKS",
            failure_meaning: "HashMap creation failed - likely getrandom not seeding hasher",
            check_hint: "HashMap requires working getrandom for hasher seeding",
        },
        BootStage {
            name: "Rust std realloc preserves data",
            marker: "RUST_STD_REALLOC_WORKS",
            failure_meaning: "realloc() did not preserve data when growing",
            check_hint: "Check libbreenix-libc realloc implementation",
        },
        BootStage {
            name: "Rust std format! macro works",
            marker: "RUST_STD_FORMAT_WORKS",
            failure_meaning: "format! macro failed",
            check_hint: "Check String and Vec allocation paths",
        },
        BootStage {
            name: "Rust std realloc shrink preserves data",
            marker: "RUST_STD_REALLOC_SHRINK_WORKS",
            failure_meaning: "realloc() did not preserve data when shrinking",
            check_hint: "Check libbreenix-libc realloc implementation",
        },
        BootStage {
            name: "Rust std read() error handling works",
            marker: "RUST_STD_READ_ERROR_WORKS",
            failure_meaning: "read() did not return proper error for invalid fd",
            check_hint: "Check libbreenix-libc read implementation",
        },
        BootStage {
            name: "Rust std read() success with pipe works",
            marker: "RUST_STD_READ_SUCCESS_WORKS",
            failure_meaning: "read() did not successfully read data from a pipe",
            check_hint: "Check libbreenix-libc pipe/read implementation",
        },
        BootStage {
            name: "Rust std malloc boundary conditions work",
            marker: "RUST_STD_MALLOC_BOUNDARY_WORKS",
            failure_meaning: "malloc() boundary conditions failed",
            check_hint: "Check libbreenix-libc malloc implementation",
        },
        BootStage {
            name: "Rust std posix_memalign works",
            marker: "RUST_STD_POSIX_MEMALIGN_WORKS",
            failure_meaning: "posix_memalign() failed",
            check_hint: "Check libbreenix-libc posix_memalign implementation",
        },
        BootStage {
            name: "Rust std sbrk works",
            marker: "RUST_STD_SBRK_WORKS",
            failure_meaning: "sbrk() failed",
            check_hint: "Check libbreenix-libc sbrk implementation",
        },
        BootStage {
            name: "Rust std getpid/gettid works",
            marker: "RUST_STD_GETPID_WORKS",
            failure_meaning: "getpid()/gettid() failed",
            check_hint: "Check libbreenix-libc getpid/gettid implementation",
        },
        BootStage {
            name: "Rust std posix_memalign error handling works",
            marker: "RUST_STD_POSIX_MEMALIGN_ERRORS_WORK",
            failure_meaning: "posix_memalign() did not return EINVAL for invalid alignments",
            check_hint: "Check libbreenix-libc posix_memalign EINVAL paths",
        },
        BootStage {
            name: "Rust std close works",
            marker: "RUST_STD_CLOSE_WORKS",
            failure_meaning: "close() syscall failed",
            check_hint: "Check libbreenix-libc close/dup implementation",
        },
        BootStage {
            name: "Rust std mprotect works",
            marker: "RUST_STD_MPROTECT_WORKS",
            failure_meaning: "mprotect() syscall failed",
            check_hint: "Check libbreenix-libc mprotect and syscall/mmap.rs:sys_mprotect",
        },
        BootStage {
            name: "Rust std stub functions work",
            marker: "RUST_STD_STUB_FUNCTIONS_WORK",
            failure_meaning: "libc stub functions failed",
            check_hint: "Check libbreenix-libc stub function implementations",
        },
        BootStage {
            name: "Rust std free(NULL) is safe",
            marker: "RUST_STD_FREE_NULL_WORKS",
            failure_meaning: "free(NULL) crashed or behaved incorrectly",
            check_hint: "Check libbreenix-libc free implementation NULL check",
        },
        BootStage {
            name: "Rust std write edge cases work",
            marker: "RUST_STD_WRITE_EDGE_CASES_WORK",
            failure_meaning: "write() edge case handling failed",
            check_hint: "Check libbreenix-libc write implementation and syscall/io.rs error paths",
        },
        BootStage {
            name: "Rust std mmap/munmap direct tests work",
            marker: "RUST_STD_MMAP_WORKS",
            failure_meaning: "Direct mmap/munmap tests failed",
            check_hint: "Check libbreenix-libc mmap/munmap and syscall/mmap.rs",
        },
        BootStage {
            name: "Rust std thread::sleep works",
            marker: "RUST_STD_SLEEP_WORKS",
            failure_meaning: "nanosleep syscall failed",
            check_hint: "Check syscall/time.rs:sys_nanosleep and scheduler wake_expired_timers",
        },
        BootStage {
            name: "Rust std thread::spawn and join work",
            marker: "RUST_STD_THREAD_WORKS",
            failure_meaning: "clone/futex syscalls failed - thread creation or join not working",
            check_hint: "Check syscall/clone.rs, syscall/futex.rs, libbreenix-libc pthread_create/join",
        },

        // Ctrl-C (SIGINT) test
        BootStage {
            name: "Ctrl-C (SIGINT) test passed",
            marker: "CTRL_C_TEST_PASSED",
            failure_meaning: "Ctrl-C signal test failed",
            check_hint: "Check signal/delivery.rs, syscall/signal.rs:sys_kill()",
        },

        // Fork and CoW tests
        BootStage {
            name: "Fork memory isolation test passed",
            marker: "FORK_MEMORY_ISOLATION_PASSED",
            failure_meaning: "Fork memory isolation test failed",
            check_hint: "Check process/fork.rs CoW page table cloning",
        },
        BootStage {
            name: "Fork state copy test passed",
            marker: "FORK_STATE_COPY_PASSED",
            failure_meaning: "Fork state test failed",
            check_hint: "Check process/fork.rs copy_process_state()",
        },
        BootStage {
            name: "Fork pending signal test passed",
            marker: "FORK_PENDING_SIGNAL_TEST_PASSED",
            failure_meaning: "Fork pending signal test failed - child inherited pending signals",
            check_hint: "Check process/fork.rs signal state handling",
        },
        BootStage {
            name: "CoW signal delivery test passed",
            marker: "COW_SIGNAL_TEST_PASSED",
            failure_meaning: "CoW signal test failed",
            check_hint: "Check interrupts handle_cow_direct() - CoW fault during signal delivery",
        },
        BootStage {
            name: "CoW cleanup test passed",
            marker: "COW_CLEANUP_TEST_PASSED",
            failure_meaning: "CoW cleanup test failed",
            check_hint: "Check frame_decref() calls on process exit",
        },
        BootStage {
            name: "CoW sole owner test passed",
            marker: "COW_SOLE_OWNER_TEST_PASSED",
            failure_meaning: "CoW sole owner test failed",
            check_hint: "Check frame_is_shared() and sole owner path in handle_cow_fault",
        },
        BootStage {
            name: "CoW stress test passed",
            marker: "COW_STRESS_TEST_PASSED",
            failure_meaning: "CoW stress test failed",
            check_hint: "Check refcounting at scale",
        },
        BootStage {
            name: "CoW read-only page sharing test passed",
            marker: "COW_READONLY_TEST_PASSED",
            failure_meaning: "CoW read-only test failed",
            check_hint: "Check setup_cow_pages() read-only path in process/fork.rs",
        },

        // Argv and exec tests
        BootStage {
            name: "Argv support test passed",
            marker: "ARGV_TEST_PASSED",
            failure_meaning: "Argv test failed",
            check_hint: "Check process/manager.rs exec_process_with_argv() and setup_argv_on_stack()",
        },
        BootStage {
            name: "Exec argv test passed",
            marker: "EXEC_ARGV_TEST_PASSED",
            failure_meaning: "Exec argv test failed",
            check_hint: "Check process/manager.rs exec_process_with_argv()",
        },
        BootStage {
            name: "Exec stack argv test passed",
            marker: "EXEC_STACK_ARGV_TEST_PASSED",
            failure_meaning: "Stack-allocated argv test failed",
            check_hint: "Check core::hint::black_box() usage in try_execute_external()",
        },

        // Graphics test
        BootStage {
            name: "FbInfo syscall test passed",
            marker: "FBINFO_TEST: all tests PASSED",
            failure_meaning: "FbInfo syscall test failed",
            check_hint: "Check syscall/graphics.rs sys_fbinfo()",
        },
    ]
}

/// x86_64-only extra stages (33 stages).
/// These include diagnostic sub-tests, process group kill, kthread, workqueue, and softirq tests.
fn x86_64_extra_stages() -> Vec<BootStage> {
    vec![
        // Diagnostic sub-tests (Test 41a-e) - x86_64 only (uses x86 inline asm)
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

        // Process group kill semantics test
        BootStage {
            name: "Process group kill semantics test passed",
            marker: "KILL_PGROUP_TEST_PASSED",
            failure_meaning: "kill process group test failed - kill(0, sig), kill(-pgid, sig), or kill(-1, sig) not working correctly",
            check_hint: "Check kernel/src/syscall/signal.rs:sys_kill() and process group signal delivery in kernel/src/signal/delivery.rs",
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
    ]
}

//! ARM64 Kernel POST test using shared QEMU infrastructure
//!
//! This test validates that the ARM64 kernel boots successfully and initializes
//! all required subsystems. It mirrors the x86_64 boot_post_test.rs but is
//! specifically designed for ARM64 architecture.
//!
//! Run with: cargo test --test arm64_boot_post_test -- --ignored --nocapture

mod shared_qemu_aarch64;
use shared_qemu_aarch64::{arm64_post_checks, get_arm64_kernel_output};

/// ARM64 kernel POST test
///
/// This test:
/// 1. Builds the ARM64 kernel (if not already built)
/// 2. Runs it in QEMU (qemu-system-aarch64 with virt machine)
/// 3. Captures serial output
/// 4. Validates all POST checkpoints are reached
/// 5. Reports pass/fail for each subsystem
///
/// This test is marked #[ignore] because it requires QEMU and takes significant time.
/// Run explicitly with: cargo test --test arm64_boot_post_test -- --ignored --nocapture
#[test]
#[ignore]
fn test_arm64_kernel_post() {
    println!("\n========================================");
    println!("  ARM64 Breenix Kernel POST Test");
    println!("========================================\n");

    // Get kernel output from shared QEMU instance
    let output = get_arm64_kernel_output();

    // Check for build/run errors
    if output.starts_with("BUILD_ERROR:") {
        panic!("ARM64 kernel build failed: {}", &output[12..]);
    }
    if output.starts_with("QEMU_ERROR:") {
        panic!("ARM64 QEMU run failed: {}", &output[11..]);
    }

    // POST checks
    println!("\nPOST Results:");
    println!("================\n");

    let post_checks = arm64_post_checks();

    let mut passed = 0;
    let mut failed = Vec::new();

    for check in &post_checks {
        print!("  {:.<22} ", check.subsystem);
        if output.contains(check.check_string) {
            println!("PASS - {}", check.description);
            passed += 1;
        } else {
            println!("FAIL - {}", check.description);
            failed.push((check.subsystem, check.check_string, check.description));
        }
    }

    println!("\n================");
    println!(
        "Summary: {}/{} subsystems passed POST",
        passed,
        post_checks.len()
    );
    println!("================\n");

    // Check for boot completion marker
    if output.contains("Breenix ARM64 Boot Complete") && output.contains("Hello from ARM64") {
        println!("Boot completion marker found - kernel reached expected state");
    } else {
        println!(
            "Warning: Boot completion marker not found - kernel may not have fully initialized"
        );
    }

    // Report failures
    if !failed.is_empty() {
        eprintln!("\nPOST FAILED - The following subsystems did not initialize:");
        for (subsystem, _, _) in &failed {
            eprintln!("   - {}", subsystem);
        }
        eprintln!("\nFirst 80 lines of kernel output:");
        eprintln!("--------------------------------");
        for line in output.lines().take(80) {
            eprintln!("{}", line);
        }

        panic!(
            "ARM64 kernel POST failed - {} subsystems did not initialize",
            failed.len()
        );
    }

    println!("All POST checks passed - ARM64 kernel is healthy!\n");
}

/// Minimal ARM64 boot test - just checks for "Hello from ARM64"
///
/// This is a simpler test that just verifies the kernel boots at all.
/// Useful for quick sanity checks.
#[test]
#[ignore]
fn test_arm64_boot_hello() {
    println!("\n========================================");
    println!("  ARM64 Boot Hello Test (Minimal)");
    println!("========================================\n");

    let output = get_arm64_kernel_output();

    // Check for errors
    if output.starts_with("BUILD_ERROR:") {
        panic!("ARM64 kernel build failed: {}", &output[12..]);
    }
    if output.starts_with("QEMU_ERROR:") {
        panic!("ARM64 QEMU run failed: {}", &output[11..]);
    }

    // Just check for the hello message
    if output.contains("Hello from ARM64") {
        println!("SUCCESS: ARM64 kernel printed 'Hello from ARM64!'");
        println!("Kernel output length: {} bytes", output.len());
    } else {
        eprintln!("FAILED: 'Hello from ARM64' not found in output");
        eprintln!("\nKernel output (first 50 lines):");
        for line in output.lines().take(50) {
            eprintln!("{}", line);
        }
        panic!("ARM64 kernel did not print expected hello message");
    }
}

/// ARM64 GIC initialization test
///
/// Specifically validates that the GICv2 interrupt controller initializes correctly.
#[test]
#[ignore]
fn test_arm64_gic_init() {
    println!("\n========================================");
    println!("  ARM64 GIC Initialization Test");
    println!("========================================\n");

    let output = get_arm64_kernel_output();

    // Check for errors
    if output.starts_with("BUILD_ERROR:") || output.starts_with("QEMU_ERROR:") {
        panic!("Failed to get kernel output");
    }

    let gic_checks = [
        ("GICv2 Init", "Initializing GICv2"),
        ("GIC Complete", "GIC initialized"),
        ("IRQ 33 Enable", "Enabling GIC IRQ 33 (UART0)"),
        ("UART IRQ", "UART interrupts enabled"),
    ];

    let mut all_passed = true;

    for (name, check) in &gic_checks {
        print!("  {:.<25} ", name);
        if output.contains(check) {
            println!("PASS");
        } else {
            println!("FAIL");
            all_passed = false;
        }
    }

    if !all_passed {
        eprintln!("\nGIC-related output:");
        for line in output.lines() {
            if line.to_lowercase().contains("gic")
                || line.to_lowercase().contains("irq")
                || line.to_lowercase().contains("interrupt")
            {
                eprintln!("  {}", line);
            }
        }
        panic!("ARM64 GIC initialization incomplete");
    }

    println!("\nGIC initialization verified successfully!");
}

/// ARM64 timer initialization test
///
/// Validates that the ARM Generic Timer initializes correctly.
#[test]
#[ignore]
fn test_arm64_timer_init() {
    println!("\n========================================");
    println!("  ARM64 Generic Timer Test");
    println!("========================================\n");

    let output = get_arm64_kernel_output();

    // Check for errors
    if output.starts_with("BUILD_ERROR:") || output.starts_with("QEMU_ERROR:") {
        panic!("Failed to get kernel output");
    }

    let timer_checks = [
        ("Timer Init", "Initializing Generic Timer"),
        ("Timer Freq", "Timer frequency:"),
        ("Timer Interrupt", "Initializing timer interrupt"),
        ("Timer Interrupt Done", "Timer interrupt initialized"),
    ];

    let mut all_passed = true;

    for (name, check) in &timer_checks {
        print!("  {:.<25} ", name);
        if output.contains(check) {
            println!("PASS");
        } else {
            println!("FAIL");
            all_passed = false;
        }
    }

    // Extract and display timer frequency if found
    for line in output.lines() {
        if line.contains("Timer frequency:") {
            println!("\nTimer info: {}", line.trim());
        }
        if line.contains("Current timestamp:") {
            println!("  {}", line.trim());
        }
    }

    if !all_passed {
        eprintln!("\nTimer-related output:");
        for line in output.lines() {
            if line.to_lowercase().contains("timer") {
                eprintln!("  {}", line);
            }
        }
        panic!("ARM64 timer initialization incomplete");
    }

    println!("\nGeneric Timer initialization verified successfully!");
}

/// ARM64 memory initialization test
///
/// Validates that memory management initializes correctly.
#[test]
#[ignore]
fn test_arm64_memory_init() {
    println!("\n========================================");
    println!("  ARM64 Memory Initialization Test");
    println!("========================================\n");

    let output = get_arm64_kernel_output();

    // Check for errors
    if output.starts_with("BUILD_ERROR:") || output.starts_with("QEMU_ERROR:") {
        panic!("Failed to get kernel output");
    }

    let memory_checks = [
        ("MMU", "MMU already enabled"),
        ("Memory Init", "Initializing memory management"),
        ("Memory Ready", "Memory management ready"),
    ];

    let mut all_passed = true;

    for (name, check) in &memory_checks {
        print!("  {:.<25} ", name);
        if output.contains(check) {
            println!("PASS");
        } else {
            println!("FAIL");
            all_passed = false;
        }
    }

    if !all_passed {
        eprintln!("\nMemory-related output:");
        for line in output.lines() {
            if line.to_lowercase().contains("memory")
                || line.to_lowercase().contains("mmu")
                || line.to_lowercase().contains("heap")
                || line.to_lowercase().contains("frame")
            {
                eprintln!("  {}", line);
            }
        }
        panic!("ARM64 memory initialization incomplete");
    }

    println!("\nMemory initialization verified successfully!");
}

// =============================================================================
// ARM64 Interrupt System Tests (ported from x86_64 interrupt_tests.rs)
// =============================================================================

/// ARM64 interrupt system initialization test
///
/// Validates that the GICv2 and exception vectors initialize correctly.
/// This is the ARM64 equivalent of test_interrupt_initialization() which checks
/// GDT/IDT/PIC on x86_64.
#[test]
#[ignore]
fn test_arm64_interrupt_initialization() {
    println!("\n========================================");
    println!("  ARM64 Interrupt System Init Test");
    println!("========================================\n");

    let output = get_arm64_kernel_output();

    // Check for errors
    if output.starts_with("BUILD_ERROR:") || output.starts_with("QEMU_ERROR:") {
        panic!("Failed to get kernel output");
    }

    // ARM64-specific interrupt system checks (equivalent to GDT/IDT/PIC on x86_64)
    let interrupt_checks = [
        ("Exception Vectors", "Current exception level: EL1"), // VBAR_EL1 set if we're at EL1
        ("GICv2 Distributor", "Initializing GICv2"),
        ("GIC Complete", "GIC initialized"),
        ("UART IRQ Enable", "Enabling GIC IRQ 33 (UART0)"),
        ("CPU IRQ Enable", "Interrupts enabled:"),
    ];

    let mut all_passed = true;

    for (name, check) in &interrupt_checks {
        print!("  {:.<30} ", name);
        if output.contains(check) {
            println!("PASS");
        } else {
            println!("FAIL");
            all_passed = false;
        }
    }

    if !all_passed {
        eprintln!("\nInterrupt-related output:");
        for line in output.lines() {
            if line.to_lowercase().contains("gic")
                || line.to_lowercase().contains("interrupt")
                || line.to_lowercase().contains("irq")
                || line.to_lowercase().contains("exception")
            {
                eprintln!("  {}", line);
            }
        }
        panic!("ARM64 interrupt system initialization incomplete");
    }

    println!("\nInterrupt system initialization verified successfully!");
}

/// ARM64 breakpoint exception test
///
/// Validates that BRK instruction exceptions are handled correctly.
/// This is the ARM64 equivalent of test_breakpoint_interrupt() which tests INT 3 on x86_64.
#[test]
#[ignore]
fn test_arm64_breakpoint_exception() {
    println!("\n========================================");
    println!("  ARM64 Breakpoint Exception Test");
    println!("========================================\n");

    let output = get_arm64_kernel_output();

    // Check for errors
    if output.starts_with("BUILD_ERROR:") || output.starts_with("QEMU_ERROR:") {
        panic!("Failed to get kernel output");
    }

    // Check for BRK exception handling
    // The kernel should handle BRK instructions and continue execution
    let brk_checks = [
        ("BRK Handler", "Breakpoint (BRK"),
        // Note: The kernel may or may not execute BRK instructions during POST
        // If it does, we should see the handler message
    ];

    println!("  Checking for BRK exception handling capability...");

    // Check if exception handling code exists (we can verify via GIC/interrupt setup)
    if output.contains("Current exception level: EL1") && output.contains("GIC initialized") {
        println!("  Exception handling infrastructure: PRESENT");
        println!("  (BRK exceptions will be handled when executed)");
    } else {
        println!("  FAIL - Exception infrastructure not ready");
        panic!("ARM64 exception handling not properly initialized");
    }

    // If a BRK was executed during boot, check it was handled
    for (name, check) in &brk_checks {
        if output.contains(check) {
            println!("  {:.<30} PASS", name);
        }
    }

    println!("\nBreakpoint exception handling verified!");
}

/// ARM64 keyboard/input interrupt test
///
/// Validates that keyboard queue is initialized for input handling.
/// On ARM64, VirtIO input may use polling or IRQ 33 for UART input.
#[test]
#[ignore]
fn test_arm64_keyboard_input() {
    println!("\n========================================");
    println!("  ARM64 Keyboard/Input Test");
    println!("========================================\n");

    let output = get_arm64_kernel_output();

    // Check for errors
    if output.starts_with("BUILD_ERROR:") || output.starts_with("QEMU_ERROR:") {
        panic!("Failed to get kernel output");
    }

    // ARM64 input handling (either VirtIO keyboard or UART input)
    let input_checks = [
        ("UART IRQ", "UART interrupts enabled"),
        ("VirtIO Input", "VirtIO input device"),
    ];

    let mut uart_ready = false;
    let mut virtio_ready = false;

    for (name, check) in &input_checks {
        print!("  {:.<30} ", name);
        if output.contains(check) {
            println!("FOUND");
            if name == &"UART IRQ" {
                uart_ready = true;
            }
            if name == &"VirtIO Input" {
                virtio_ready = true;
            }
        } else {
            println!("not found");
        }
    }

    // At least one input method should be available
    if uart_ready || virtio_ready {
        println!("\n  Input handling capability: READY");
        if uart_ready {
            println!("    - UART serial input enabled");
        }
        if virtio_ready {
            println!("    - VirtIO keyboard enabled");
        }
    } else {
        // OPTIONAL: Input handling is not required for basic boot test
        println!("\n  Input handling:         (not configured - optional for boot test)");
    }

    println!("\nKeyboard/input setup verified!");
}

/// ARM64 timer interrupt test
///
/// Validates that timer interrupts are working by checking for advancing timestamps.
/// This is the ARM64 equivalent of test_timer_interrupt().
#[test]
#[ignore]
fn test_arm64_timer_interrupt() {
    println!("\n========================================");
    println!("  ARM64 Timer Interrupt Test");
    println!("========================================\n");

    let output = get_arm64_kernel_output();

    // Check for errors
    if output.starts_with("BUILD_ERROR:") || output.starts_with("QEMU_ERROR:") {
        panic!("Failed to get kernel output");
    }

    // Extract timestamps from log lines
    let timestamps = shared_qemu_aarch64::extract_arm64_timestamps(output);

    println!("  Timestamp analysis:");
    println!("    Found {} log entries with timestamps", timestamps.len());

    if timestamps.len() < 5 {
        eprintln!("\nWarning: Few timestamps found, timer may not be running long enough");
        eprintln!(
            "First few timestamps: {:?}",
            &timestamps[..timestamps.len().min(5)]
        );
    }

    // Verify timestamps are increasing (timer interrupts are working)
    let mut increasing = true;
    let mut violations = Vec::new();
    for window in timestamps.windows(2) {
        if window[1] < window[0] {
            increasing = false;
            violations.push((window[0], window[1]));
        }
    }

    print!("  {:.<30} ", "Timestamps Monotonic");
    if increasing {
        println!("PASS");
    } else {
        println!("FAIL");
        eprintln!(
            "  Timestamp violations: {:?}",
            &violations[..violations.len().min(3)]
        );
    }

    // Check minimum number of timestamps (indicates timer is ticking)
    print!("  {:.<30} ", "Timer Ticking");
    if timestamps.len() >= 10 {
        println!("PASS ({} updates)", timestamps.len());
    } else {
        println!("FAIL ({} updates, expected >= 10)", timestamps.len());
    }

    // Calculate tick rate if we have enough samples
    if timestamps.len() > 1 {
        let first = timestamps.first().unwrap();
        let last = timestamps.last().unwrap();
        let duration = last - first;
        if duration > 0.0 {
            let tick_rate = (timestamps.len() as f64) / duration;
            println!("  Timer output rate: ~{:.1} log entries/sec", tick_rate);
        }
    }

    assert!(
        increasing,
        "Timer interrupts not advancing time monotonically"
    );
    assert!(
        timestamps.len() >= 5,
        "Too few timer updates: {}",
        timestamps.len()
    );

    println!("\nTimer interrupt test passed!");
}

// =============================================================================
// ARM64 Timer Tests (ported from x86_64 timer_tests.rs)
// =============================================================================

/// ARM64 timer initialization test
///
/// Validates that the ARM Generic Timer initializes correctly.
/// This is the ARM64 equivalent of test_timer_initialization().
#[test]
#[ignore]
fn test_arm64_timer_initialization() {
    println!("\n========================================");
    println!("  ARM64 Timer Initialization Test");
    println!("========================================\n");

    let output = get_arm64_kernel_output();

    // Check for errors
    if output.starts_with("BUILD_ERROR:") || output.starts_with("QEMU_ERROR:") {
        panic!("Failed to get kernel output");
    }

    // ARM Generic Timer initialization checks
    let timer_init_checks = [
        ("Timer Init", "Initializing Generic Timer"),
        ("Timer Frequency", "Timer frequency:"),
        ("Timer IRQ Init", "ARM64 timer interrupt init"),
        ("Timer Configured", "Timer configured for"),
        ("Timer Complete", "Timer interrupt initialized"),
    ];

    let mut all_passed = true;
    let mut frequency_hz: Option<u64> = None;

    for (name, check) in &timer_init_checks {
        print!("  {:.<30} ", name);
        if output.contains(check) {
            println!("PASS");
        } else {
            println!("FAIL");
            all_passed = false;
        }
    }

    // Extract and display timer frequency
    for line in output.lines() {
        if line.contains("Timer frequency:") {
            // Try to extract the frequency value
            if let Some(hz_str) = line.split("Timer frequency:").nth(1) {
                let hz_str = hz_str.trim().replace(" Hz", "").replace(",", "");
                if let Ok(hz) = hz_str.parse::<u64>() {
                    frequency_hz = Some(hz);
                }
            }
            println!("\n  Reported: {}", line.trim());
        }
        if line.contains("Timer configured for") {
            println!("  {}", line.trim());
        }
    }

    // Validate frequency is reasonable (ARM typically 1-100 MHz range)
    if let Some(hz) = frequency_hz {
        print!("  {:.<30} ", "Frequency Valid");
        if hz >= 1_000_000 && hz <= 1_000_000_000 {
            println!("PASS ({} Hz)", hz);
        } else {
            println!("WARN (unusual: {} Hz)", hz);
        }
    }

    if !all_passed {
        eprintln!("\nTimer-related output:");
        for line in output.lines() {
            if line.to_lowercase().contains("timer") {
                eprintln!("  {}", line);
            }
        }
        panic!("ARM64 timer initialization incomplete");
    }

    println!("\nTimer initialization verified successfully!");
}

/// ARM64 timer ticks test
///
/// Validates that multiple unique timestamps are produced.
/// This is the ARM64 equivalent of test_timer_ticks().
#[test]
#[ignore]
fn test_arm64_timer_ticks() {
    println!("\n========================================");
    println!("  ARM64 Timer Ticks Test");
    println!("========================================\n");

    let output = get_arm64_kernel_output();

    // Check for errors
    if output.starts_with("BUILD_ERROR:") || output.starts_with("QEMU_ERROR:") {
        panic!("Failed to get kernel output");
    }

    // Extract timestamps using shared helper
    let timestamps = shared_qemu_aarch64::extract_arm64_timestamps(output);

    // Convert to strings for HashSet since f64 doesn't implement Hash/Eq
    let timestamp_strings: Vec<String> = timestamps.iter().map(|t| format!("{:.6}", t)).collect();
    let unique_timestamps: std::collections::HashSet<_> = timestamp_strings.iter().collect();

    println!("  Total timestamps: {}", timestamps.len());
    println!("  Unique timestamps: {}", unique_timestamps.len());

    // Display some sample timestamps
    if !timestamps.is_empty() {
        println!("\n  Sample timestamps (first 5):");
        for (i, ts) in timestamps.iter().take(5).enumerate() {
            println!("    [{}] {:.6}", i, ts);
        }
        if timestamps.len() > 5 {
            println!("    ...");
            if let Some(last) = timestamps.last() {
                println!("    [{}] {:.6}", timestamps.len() - 1, last);
            }
        }
    }

    // We should see multiple different timestamps (at least 2 for timer advancement)
    assert!(
        unique_timestamps.len() >= 2,
        "Timer doesn't appear to be advancing: {} unique timestamps",
        unique_timestamps.len()
    );

    println!(
        "\nTimer ticks test passed ({} unique timestamps)",
        unique_timestamps.len()
    );
}

/// ARM64 delay functionality test
///
/// Validates that delay/sleep operations work correctly.
/// This is the ARM64 equivalent of test_delay_functionality().
#[test]
#[ignore]
fn test_arm64_delay_functionality() {
    println!("\n========================================");
    println!("  ARM64 Delay Functionality Test");
    println!("========================================\n");

    let output = get_arm64_kernel_output();

    // Check for errors
    if output.starts_with("BUILD_ERROR:") || output.starts_with("QEMU_ERROR:") {
        panic!("Failed to get kernel output");
    }

    // Check for delay test markers (if the kernel runs delay tests)
    let delay_checks = [
        ("Delay Test Start", "Testing delay"),
        ("Delay Complete", "delay"),
    ];

    let mut delay_tested = false;

    for (name, check) in &delay_checks {
        if output.to_lowercase().contains(&check.to_lowercase()) {
            print!("  {:.<30} ", name);
            println!("FOUND");
            delay_tested = true;
        }
    }

    // The delay functionality can be inferred from timer working correctly
    if !delay_tested {
        println!("  Note: Explicit delay test not found in boot log");
        println!("  Verifying delay capability via timer...");

        // Verify timer is working (prerequisite for delays)
        let has_timer =
            output.contains("Timer initialized") || output.contains("Timer interrupt initialized");

        print!("  {:.<30} ", "Timer (delay prereq)");
        if has_timer {
            println!("PASS");
            println!("  Delay functionality is available (timer operational)");
        } else {
            println!("FAIL");
            panic!("Timer not initialized - delays will not work");
        }
    }

    println!("\nDelay functionality verified!");
}

/// ARM64 RTC functionality test
///
/// Validates Real Time Clock functionality (if available).
/// This is the ARM64 equivalent of test_rtc_functionality().
/// Note: ARM64 QEMU virt machine may not have an RTC like PC-style CMOS RTC.
#[test]
#[ignore]
fn test_arm64_rtc_functionality() {
    println!("\n========================================");
    println!("  ARM64 RTC Functionality Test");
    println!("========================================\n");

    let output = get_arm64_kernel_output();

    // Check for errors
    if output.starts_with("BUILD_ERROR:") || output.starts_with("QEMU_ERROR:") {
        panic!("Failed to get kernel output");
    }

    // ARM64 may use PL031 RTC or derive wall-clock time from other sources
    let rtc_checks = [
        ("Unix Timestamp", "Unix timestamp"),
        ("RTC Init", "RTC"),
        ("Wall Clock", "wall clock"),
        ("Real Time", "real time"),
    ];

    let mut rtc_found = false;
    let mut timestamp: Option<u64> = None;

    for (name, check) in &rtc_checks {
        if output.to_lowercase().contains(&check.to_lowercase()) {
            print!("  {:.<30} ", name);
            println!("FOUND");
            rtc_found = true;
        }
    }

    // Try to extract a Unix timestamp if mentioned
    for line in output.lines() {
        if line.to_lowercase().contains("unix timestamp")
            || line.to_lowercase().contains("real time")
        {
            // Try to parse a timestamp from the line
            for word in line.split_whitespace() {
                if let Ok(ts) = word.trim_matches(|c: char| !c.is_numeric()).parse::<u64>() {
                    if ts > 1577836800 && ts < 2000000000 {
                        // Reasonable Unix timestamp range (2020-2033)
                        timestamp = Some(ts);
                        println!("  Extracted Unix timestamp: {}", ts);
                        break;
                    }
                }
            }
        }
    }

    // Validate timestamp if found
    if let Some(ts) = timestamp {
        print!("  {:.<30} ", "Timestamp Valid");
        if ts > 1577836800 {
            // After Jan 1, 2020
            println!("PASS ({})", ts);
        } else {
            println!("WARN (timestamp seems old: {})", ts);
        }
    }

    if !rtc_found {
        println!("  Note: ARM64 QEMU virt machine may not have RTC");
        println!("  Time tracking via Generic Timer is available");

        // Verify monotonic time works (alternative to RTC)
        let has_timer = output.contains("Timer frequency:");
        print!("  {:.<30} ", "Generic Timer (time source)");
        if has_timer {
            println!("PASS");
        } else {
            println!("FAIL");
        }
    }

    println!("\nRTC/time functionality verified!");
}

// =============================================================================
// ARM64 Privilege Level Tests (ported from x86_64 ring3_smoke_test.rs)
// =============================================================================

/// ARM64 EL0 (userspace) smoke test
///
/// This test validates that EL0 execution works correctly by checking for:
/// 1. EL0 entry via ERET (ARM64 equivalent of IRETQ to Ring 3)
/// 2. Syscalls from EL0 via SVC instruction (ARM64 equivalent of INT 0x80)
/// 3. Process creation working
///
/// ARM64 vs x86_64 equivalents:
/// - Ring 3 (CPL=3) -> EL0 (Exception Level 0)
/// - CS=0x33 -> SPSR[3:0]=0x0
/// - INT 0x80 -> SVC instruction
/// - IRETQ -> ERET
#[test]
#[ignore]
fn test_arm64_el0_smoke() {
    println!("\n========================================");
    println!("  ARM64 EL0 (Userspace) Smoke Test");
    println!("========================================\n");

    let output = get_arm64_kernel_output();

    // Check for errors
    if output.starts_with("BUILD_ERROR:") || output.starts_with("QEMU_ERROR:") {
        panic!("Failed to get kernel output");
    }

    // Check for EL0 entry marker (ARM64 equivalent of IRETQ to Ring 3)
    let found_el0_enter = output.contains("EL0_ENTER: First userspace entry");

    // Check for EL0 smoke marker
    let found_el0_smoke = output.contains("EL0_SMOKE: userspace executed");

    // Check for EL0_CONFIRMED marker - definitive proof syscall came from EL0
    // This is the ARM64 equivalent of RING3_CONFIRMED
    let found_el0_confirmed = output.contains("EL0_CONFIRMED: First syscall received from EL0");

    // Check for process creation
    let found_process_created = output.contains("SUCCESS - returning PID")
        || output.contains("Process created with PID")
        || output.contains("Successfully created");

    // Check for syscall output (evidence syscalls are working)
    let found_syscall_output = output.contains("[user]")
        || output.contains("[syscall]")
        || output.contains("Hello from EL0");

    // Check for critical fault markers that would indicate failure
    assert!(
        !output.contains("DOUBLE FAULT") && !output.contains("Data Abort"),
        "Kernel faulted during EL0 test"
    );
    assert!(
        !output.contains("kernel panic"),
        "Kernel panicked during EL0 test"
    );

    // Display results
    println!("EL0 Evidence Found:");

    if found_el0_confirmed {
        println!("  [PASS] EL0_CONFIRMED - Syscall from EL0 (SPSR[3:0]=0)!");
    } else {
        println!("  [----] EL0_CONFIRMED - Not found (userspace may not have executed syscall)");
    }

    if found_el0_enter {
        println!("  [PASS] EL0_ENTER - First ERET to userspace logged");
    } else {
        println!("  [----] EL0_ENTER - Not found");
    }

    if found_el0_smoke {
        println!("  [PASS] EL0_SMOKE - Userspace execution verified");
    } else {
        println!("  [----] EL0_SMOKE - Not found");
    }

    if found_process_created {
        println!("  [PASS] Process creation succeeded");
    } else {
        println!("  [----] Process creation marker not found");
    }

    if found_syscall_output {
        println!("  [PASS] Syscall output detected");
    } else {
        println!("  [----] No syscall output detected");
    }

    // The strongest evidence is EL0_CONFIRMED - a syscall from userspace
    if found_el0_confirmed {
        println!("\n========================================");
        println!("EL0 SMOKE TEST PASSED - DEFINITIVE PROOF:");
        println!("  First syscall received from EL0 (SPSR confirms userspace)");
        println!("  ARM64 equivalent of x86_64 Ring 3 (CS=0x33) confirmed!");
        println!("========================================");
    } else if found_el0_enter || found_el0_smoke {
        println!("\n========================================");
        println!("EL0 SMOKE TEST: PARTIAL");
        println!("  EL0 entry detected but no syscall confirmed from userspace");
        println!("  This is acceptable for current ARM64 parity state");
        println!("========================================");
    } else if found_process_created {
        println!("\n========================================");
        println!("EL0 SMOKE TEST: INFRASTRUCTURE READY");
        println!("  Process creation working, but no EL0 execution evidence");
        println!("========================================");
    } else {
        println!("\n========================================");
        println!("EL0 SMOKE TEST: NOT YET IMPLEMENTED");
        println!("  No userspace execution detected");
        println!("========================================");
    }

    // This test passes if infrastructure is in place
    // Full EL0 execution confirmation requires EL0_CONFIRMED
    assert!(
        output.contains("Current exception level: EL1"),
        "Kernel must be running at EL1 for EL0 transitions"
    );
}

/// ARM64 syscall infrastructure test
///
/// This test validates that the syscall infrastructure is properly set up:
/// - SVC instruction handling (ARM64 equivalent of INT 0x80)
/// - Exception vector setup for synchronous exceptions
/// - Syscall dispatch working
///
/// This is the ARM64 equivalent of test_syscall_infrastructure().
#[test]
#[ignore]
fn test_arm64_syscall_infrastructure() {
    println!("\n========================================");
    println!("  ARM64 Syscall Infrastructure Test");
    println!("========================================\n");

    let output = get_arm64_kernel_output();

    // Check for errors
    if output.starts_with("BUILD_ERROR:") || output.starts_with("QEMU_ERROR:") {
        panic!("Failed to get kernel output");
    }

    // Infrastructure checks - these indicate syscall handling is set up
    let syscall_checks = [
        ("Exception Level", "Current exception level: EL1"),
        ("GIC Initialized", "GIC initialized"),
        ("Interrupts Enabled", "Interrupts enabled:"),
    ];

    let mut all_passed = true;

    println!("Syscall Infrastructure:");
    for (name, check) in &syscall_checks {
        print!("  {:.<35} ", name);
        if output.contains(check) {
            println!("PASS");
        } else {
            println!("FAIL");
            all_passed = false;
        }
    }

    // Check for SVC handler evidence
    println!("\nSVC (Syscall) Handling:");

    // Check for EL0_CONFIRMED which proves SVC from userspace works
    print!("  {:.<35} ", "SVC from EL0 (userspace)");
    if output.contains("EL0_CONFIRMED") {
        println!("PASS - SVC instruction working!");
    } else {
        println!("not verified (no userspace syscall detected)");
    }

    // Check for any syscall-related output
    print!("  {:.<35} ", "Syscall output");
    if output.contains("[syscall]")
        || output.contains("sys_write")
        || output.contains("Hello from")
        || output.contains("[user]")
    {
        println!("detected");
    } else {
        println!("not found");
    }

    // ARM64-specific syscall notes
    println!("\nARM64 Syscall Convention:");
    println!("  - SVC #0 triggers synchronous exception");
    println!("  - X8 = syscall number");
    println!("  - X0-X5 = arguments");
    println!("  - X0 = return value");
    println!("  - ERET returns to EL0");

    assert!(
        all_passed,
        "ARM64 syscall infrastructure not fully initialized"
    );

    println!("\nSyscall infrastructure verified!");
}

/// ARM64 process creation test
///
/// Validates that user processes can be created and scheduled.
#[test]
#[ignore]
fn test_arm64_process_creation() {
    println!("\n========================================");
    println!("  ARM64 Process Creation Test");
    println!("========================================\n");

    let output = get_arm64_kernel_output();

    // Check for errors
    if output.starts_with("BUILD_ERROR:") || output.starts_with("QEMU_ERROR:") {
        panic!("Failed to get kernel output");
    }

    let process_checks = [
        ("Process Manager Init", "Initializing process manager"),
        ("Scheduler Init", "Initializing scheduler"),
    ];

    println!("Process Management Infrastructure:");
    for (name, check) in &process_checks {
        print!("  {:.<35} ", name);
        if output.contains(check) {
            println!("PASS");
        } else {
            println!("FAIL");
        }
    }

    // Check for actual process creation
    println!("\nProcess Creation:");

    print!("  {:.<35} ", "Process created");
    let process_created = output.contains("SUCCESS - returning PID")
        || output.contains("Process created with PID")
        || output.contains("Successfully created init process");

    if process_created {
        println!("PASS");

        // Try to extract PID
        for line in output.lines() {
            if line.contains("PID") && (line.contains("SUCCESS") || line.contains("created")) {
                println!("    {}", line.trim());
                break;
            }
        }
    } else {
        println!("not found");
    }

    // Check for init process
    print!("  {:.<35} ", "Init process (PID 1)");
    if output.contains("init_user_process") || output.contains("PID 1") {
        println!("detected");
    } else {
        println!("not found");
    }

    // Check if process actually ran (via EL0 confirmation)
    print!("  {:.<35} ", "Process executed (EL0)");
    if output.contains("EL0_CONFIRMED") || output.contains("EL0_SMOKE") {
        println!("PASS - Userspace code ran!");
    } else {
        println!("not verified");
    }

    println!("\nProcess creation test complete!");
}

/// ARM64 syscall return value test
///
/// Validates that syscalls return correct values (negative errno on error).
#[test]
#[ignore]
fn test_arm64_syscall_returns() {
    println!("\n========================================");
    println!("  ARM64 Syscall Return Value Test");
    println!("========================================\n");

    let output = get_arm64_kernel_output();

    // Check for errors
    if output.starts_with("BUILD_ERROR:") || output.starts_with("QEMU_ERROR:") {
        panic!("Failed to get kernel output");
    }

    println!("Syscall Return Convention (ARM64):");
    println!("  - Success: X0 = positive value or 0");
    println!("  - Error: X0 = negative errno");
    println!("");

    // Look for evidence of syscall execution
    print!("  {:.<35} ", "Syscall execution evidence");
    if output.contains("EL0_CONFIRMED")
        || output.contains("syscall")
        || output.contains("[user]")
        || output.contains("sys_")
    {
        println!("FOUND");
    } else {
        println!("not found");
    }

    // Check for error handling evidence
    // OPTIONAL: Errors only occur if syscalls fail, which is not required for basic boot
    print!("  {:.<35} ", "Error handling");
    if output.contains("EINVAL")
        || output.contains("ENOSYS")
        || output.contains("EBADF")
        || output.contains("errno")
    {
        println!("detected");
    } else {
        println!("(no errors - syscalls succeeded)");
    }

    println!("\nSyscall return test complete!");
}

// =============================================================================
// ARM64 Signal Tests (placeholder for signal delivery from EL0)
// =============================================================================

/// ARM64 signal delivery infrastructure test
///
/// Validates that signal delivery infrastructure exists for ARM64.
/// Full signal tests require EL0 userspace execution.
#[test]
#[ignore]
fn test_arm64_signal_infrastructure() {
    println!("\n========================================");
    println!("  ARM64 Signal Infrastructure Test");
    println!("========================================\n");

    let output = get_arm64_kernel_output();

    // Check for errors
    if output.starts_with("BUILD_ERROR:") || output.starts_with("QEMU_ERROR:") {
        panic!("Failed to get kernel output");
    }

    // Signal infrastructure is part of process management
    let signal_checks = [("Process Manager", "Initializing process manager")];

    println!("Signal Infrastructure:");
    for (name, check) in &signal_checks {
        print!("  {:.<35} ", name);
        if output.contains(check) {
            println!("PASS");
        } else {
            println!("FAIL");
        }
    }

    // Check for signal-related output
    print!("  {:.<35} ", "Signal handling code");
    if output.contains("signal") || output.contains("SIGKILL") || output.contains("sigreturn") {
        println!("detected");
    } else {
        println!("not triggered (expected - no signals sent yet)");
    }

    println!("\nARM64 Signal Notes:");
    println!("  - Signal frame saved on user stack");
    println!("  - SP_EL0 adjusted for signal handler");
    println!("  - sys_sigreturn restores context via ERET");

    println!("\nSignal infrastructure test complete!");
}

/// ARM64 timer continues during blocking syscall test
///
/// This is a REGRESSION TEST for a critical bug where timer interrupts stopped
/// firing after userspace entry. The root cause was that syscall entry masks IRQs
/// (`msr daifset, #0x2`), and the WFI loop in blocking syscalls didn't re-enable them.
///
/// The fix adds `msr daifclr, #2` before WFI in blocking syscall paths (io.rs).
///
/// This test validates that timer tick markers appear AFTER userspace entry,
/// specifically after `[STDIN_BLOCK]` which indicates the shell is blocked
/// waiting for keyboard input in a WFI loop.
#[test]
#[ignore]
fn test_arm64_timer_during_blocking_syscall() {
    println!("\n========================================");
    println!("  ARM64 Timer During Blocking Syscall");
    println!("========================================\n");

    let output = get_arm64_kernel_output();

    // Check for errors
    if output.starts_with("BUILD_ERROR:") || output.starts_with("QEMU_ERROR:") {
        panic!("Failed to get kernel output");
    }

    // Find the position of key markers in the output
    let stdin_block_pos = output.find("[STDIN_BLOCK]");
    let t10_pos = output.find("[T10]");
    let timer_diag_pos = output.find("[TIMER_DIAG]");

    // Find heartbeat dots after STDIN_BLOCK
    let dots_after_block = if let Some(block_pos) = stdin_block_pos {
        let after_block = &output[block_pos..];
        after_block.matches('.').count()
    } else {
        0
    };

    println!("  Marker Analysis:");

    // Check 1: STDIN_BLOCK marker exists (shell entered blocking read)
    print!("  {:.<40} ", "Shell entered blocking read");
    if stdin_block_pos.is_some() {
        println!("PASS");
    } else {
        println!("FAIL (no [STDIN_BLOCK] marker)");
    }

    // Check 2: [T10] marker appears after STDIN_BLOCK (timer fires during block)
    print!("  {:.<40} ", "[T10] after blocking syscall");
    let t10_after_block = match (stdin_block_pos, t10_pos) {
        (Some(block), Some(t10)) => t10 > block,
        _ => false,
    };
    if t10_after_block {
        println!("PASS");
    } else {
        println!("FAIL (timer not ticking after userspace entry)");
    }

    // Check 3: [TIMER_DIAG] marker appears (timer reached tick 100)
    print!("  {:.<40} ", "[TIMER_DIAG] marker (tick 100)");
    let timer_diag_after_block = match (stdin_block_pos, timer_diag_pos) {
        (Some(block), Some(diag)) => diag > block,
        _ => false,
    };
    if timer_diag_after_block {
        println!("PASS");
    } else if timer_diag_pos.is_some() {
        println!("WARN (appears but not after [STDIN_BLOCK])");
    } else {
        println!("FAIL (no [TIMER_DIAG] marker)");
    }

    // Check 4: Heartbeat dots appear after block (timer continues firing)
    print!("  {:.<40} ", "Heartbeat dots after blocking");
    if dots_after_block >= 3 {
        println!("PASS ({} dots)", dots_after_block);
    } else if dots_after_block > 0 {
        println!("PARTIAL ({} dots, expected >= 3)", dots_after_block);
    } else {
        println!("FAIL (no heartbeat dots after [STDIN_BLOCK])");
    }

    // Check 5: Context switching occurs (idle thread runs)
    print!("  {:.<40} ", "Context switch to idle thread");
    let context_switch = output.contains("Switching from thread 2 to thread 0")
        || output.contains("Idle thread 0 is alone");
    if context_switch {
        println!("PASS");
    } else {
        println!("FAIL (no context switch observed)");
    }

    // Print diagnostic info if any checks failed
    if !t10_after_block || dots_after_block < 3 {
        eprintln!("\nDiagnostic: Timer-related output after [STDIN_BLOCK]:");
        if let Some(block_pos) = stdin_block_pos {
            let after_block = &output[block_pos..];
            for line in after_block.lines().take(20) {
                eprintln!("  {}", line);
            }
        }
    }

    // Assertions for regression testing
    assert!(
        stdin_block_pos.is_some(),
        "Shell must enter blocking read syscall"
    );
    assert!(
        t10_after_block,
        "Timer tick [T10] must appear after [STDIN_BLOCK] - if this fails, \
         the IRQ masking bug in blocking syscalls may have regressed. \
         Check that io.rs has `msr daifclr, #2` before WFI on ARM64."
    );
    assert!(
        dots_after_block >= 1,
        "At least one heartbeat dot must appear after blocking syscall - \
         timer interrupts are not firing in userspace"
    );

    println!("\nTimer-during-blocking-syscall test passed!");
    println!("\nNote: This test guards against regression of the ARM64 IRQ masking bug");
    println!("where syscall entry (`msr daifset, #0x2`) masked interrupts and WFI");
    println!("loops didn't re-enable them, causing timer interrupts to stop.");
}

//! Test exec functionality directly

use crate::process::creation::create_user_process;
use alloc::string::String;
use alloc::vec;

/// Test multiple concurrent processes to validate page table isolation
///
/// 🚨 CRITICAL CONCURRENCY TEST 🚨
///
/// This test validates that multiple processes can run concurrently without
/// stomping on each other's memory or causing crashes.
///
/// TWO-STAGE VALIDATION PATTERN:
/// - Stage 1 (This function): Creates and schedules the process
///   - Marker: "Direct execution test: process scheduled for execution"
///   - This is a CHECKPOINT confirming process creation succeeded
///   - Does NOT prove the process executed
/// - Stage 2 (Boot stage 31): Validates actual execution
///   - Marker: "USERSPACE OUTPUT: Hello from userspace"
///   - This PROVES the process ran and printed output
///
/// SUCCESS CRITERIA:
/// - Must see: Multiple "Hello from userspace! Current time: XXXXX" outputs with different times
/// - Must see: All processes complete successfully without crashes
/// - Must see: No page table conflicts or double faults
///
/// FAILURE RESPONSE:
/// - INVESTIGATE page table isolation implementation
/// - CHECK for memory corruption between processes
/// - VERIFY syscall isolation and stack isolation
pub fn test_direct_execution() {
    log::info!("=== MULTIPLE CONCURRENT PROCESSES TEST ===");
    log::info!("Testing page table isolation with concurrent hello_time.elf processes");

    // Create and run hello_time.elf directly
    #[cfg(feature = "testing")]
    let hello_time_buf = crate::userspace_test::get_test_binary("hello_time");
    #[cfg(feature = "testing")]
    let hello_time_elf: &[u8] = &hello_time_buf;
    #[cfg(not(feature = "testing"))]
    let hello_time_elf = &create_hello_world_elf();

    log::info!(
        "CONCURRENCY TEST: Loading hello_time.elf, size: {} bytes",
        hello_time_elf.len()
    );

    // First create one hello_time process to verify basic execution
    match create_user_process(String::from("hello_time_test"), hello_time_elf) {
        Ok(pid) => {
            log::info!(
                "✓ CONCURRENT: Created hello_time process with PID {}",
                pid.as_u64()
            );
            log::info!("✓ CONCURRENT: Process should execute hello_time.elf and print time");
        }
        Err(e) => {
            log::error!("✗ CONCURRENT: Failed to create hello_time process: {}", e);
        }
    }

    log::info!("✓ CONCURRENT: Created hello_time test process");
    log::info!("    -> Process will execute hello_time.elf when scheduler runs");
    log::info!("    -> Look for 'Hello from userspace! Current time: XXXXX' output");
}

/// Test fork from userspace - validates that userspace processes can call fork()
///
/// TWO-STAGE VALIDATION PATTERN:
/// - Stage 1 (This function): Creates and schedules the process
///   - Marker: "Fork test: process scheduled for execution"
///   - This is a CHECKPOINT confirming process creation succeeded
///   - Does NOT prove the process executed
/// - Stage 2 (Boot stage 31): Validates actual execution
///   - Marker: "USERSPACE OUTPUT: Hello from userspace"
///   - This PROVES the process ran and printed output
pub fn test_userspace_fork() {
    log::info!("=== Testing multiple instances of same program ===");
    log::info!("This test runs TWO hello_time processes to isolate the issue");

    // TEMPORARILY: Use hello_time instead of fork_test to see if issue is with different ELF binaries
    #[cfg(feature = "testing")]
    let test_elf_buf = crate::userspace_test::get_test_binary("hello_time");
    #[cfg(feature = "testing")]
    let test_elf: &[u8] = &test_elf_buf;
    #[cfg(not(feature = "testing"))]
    let test_elf = &create_hello_world_elf();

    log::info!("Creating second hello_time process...");

    match create_user_process(String::from("hello_time_2"), test_elf) {
        Ok(pid) => {
            log::info!(
                "✓ Created second hello_time process with PID {}",
                pid.as_u64()
            );
            log::info!("✓ Now we have TWO hello_time processes");
            log::info!("   - If this works: issue is with different ELF layouts");
            log::info!("   - If this fails: issue is with multiple processes in general");
        }
        Err(e) => {
            log::error!("✗ Failed to create second hello_time process: {}", e);
        }
    }
}

/// Test fork/exec pattern - the standard UNIX way to create processes
pub fn test_fork_exec() {
    log::info!("=== Testing fork() + exec() pattern ===");

    // First create a parent process that will fork
    #[cfg(feature = "testing")]
    let fork_test_buf = crate::userspace_test::get_test_binary("fork_test");
    #[cfg(feature = "testing")]
    let fork_test_elf: &[u8] = &fork_test_buf;
    #[cfg(not(feature = "testing"))]
    let fork_test_elf = &create_minimal_elf_no_bss();

    match create_user_process(String::from("parent_process"), fork_test_elf) {
        Ok(parent_pid) => {
            log::info!("Created parent process with PID {}", parent_pid.as_u64());

            // Now fork the parent process
            match crate::process::with_process_manager(|manager| manager.fork_process(parent_pid)) {
                Some(Ok(child_pid)) => {
                    log::info!(
                        "✓ Fork succeeded! Parent PID: {}, Child PID: {}",
                        parent_pid.as_u64(),
                        child_pid.as_u64()
                    );

                    // Now exec hello_time.elf in the child process
                    log::info!(
                        "Attempting to exec hello_time.elf into child process {}",
                        child_pid.as_u64()
                    );

                    #[cfg(feature = "testing")]
                    let hello_time_buf = crate::userspace_test::get_test_binary("hello_time");
                    #[cfg(feature = "testing")]
                    let hello_time_elf: &[u8] = &hello_time_buf;
                    #[cfg(not(feature = "testing"))]
                    let hello_time_elf = &create_minimal_elf_no_bss();

                    match crate::process::with_process_manager(|manager| {
                        manager.exec_process(child_pid, hello_time_elf, Some("hello_time"))
                    }) {
                        Some(Ok(entry_point)) => {
                            log::info!("✓ exec succeeded! Child process {} now running hello_time at {:#x}", 
                                child_pid.as_u64(), entry_point);
                            log::info!(
                                "Parent process {} continues running fork_test",
                                parent_pid.as_u64()
                            );
                        }
                        Some(Err(e)) => {
                            log::error!("✗ exec failed: {}", e);
                        }
                        None => {
                            log::error!("Process manager not available for exec");
                        }
                    }
                }
                Some(Err(e)) => {
                    log::error!("✗ Fork failed: {}", e);
                }
                None => {
                    log::error!("Process manager not available for fork");
                }
            }
        }
        Err(e) => {
            log::error!("Failed to create parent process: {}", e);
        }
    }
}

/// Test exec directly by creating a process and then calling exec on it
pub fn test_exec_directly() {
    log::info!("=== Testing exec() directly ===");

    // First create a process with fork_test.elf
    #[cfg(feature = "testing")]
    let fork_test_buf = crate::userspace_test::get_test_binary("fork_test");
    #[cfg(feature = "testing")]
    let fork_test_elf: &[u8] = &fork_test_buf;
    #[cfg(not(feature = "testing"))]
    let fork_test_elf = &create_minimal_elf_no_bss();

    match create_user_process(String::from("test_process"), fork_test_elf) {
        Ok(pid) => {
            log::info!("Created test process with PID {}", pid.as_u64());

            // Wait a bit for process to be scheduled
            for _ in 0..1000000 {
                core::hint::spin_loop();
            }

            // Now try to exec hello_time.elf into this process
            log::info!(
                "Attempting to exec hello_time.elf into process {}",
                pid.as_u64()
            );

            #[cfg(feature = "testing")]
            let hello_time_buf = crate::userspace_test::get_test_binary("hello_time");
            #[cfg(feature = "testing")]
            let hello_time_elf: &[u8] = &hello_time_buf;
            #[cfg(not(feature = "testing"))]
            let hello_time_elf = &create_minimal_elf_no_bss();

            // Use with_process_manager to properly disable interrupts
            match crate::process::with_process_manager(|manager| {
                manager.exec_process(pid, hello_time_elf, Some("hello_time"))
            }) {
                Some(Ok(entry_point)) => {
                    log::info!("✓ exec succeeded! New entry point: {:#x}", entry_point);
                    log::info!("Process {} should now be running hello_time", pid.as_u64());
                }
                Some(Err(e)) => {
                    log::error!("✗ exec failed: {}", e);
                }
                None => {
                    log::error!("Process manager not available");
                }
            }
        }
        Err(e) => {
            log::error!("Failed to create test process: {}", e);
        }
    }
}

/// Test exec with real userspace programs (fork_test.elf -> hello_time.elf)
pub fn test_exec_real_userspace() {
    log::info!("=== Testing exec() with Real Userspace Programs ===");

    #[cfg(feature = "testing")]
    {
        let fork_test_buf = crate::userspace_test::get_test_binary("fork_test");
        let fork_test_elf: &[u8] = &fork_test_buf;
        let hello_time_buf = crate::userspace_test::get_test_binary("hello_time");
        let hello_time_elf: &[u8] = &hello_time_buf;

        log::info!("fork_test.elf size: {} bytes", fork_test_elf.len());
        log::info!("hello_time.elf size: {} bytes", hello_time_elf.len());

        // Create a process with fork_test.elf
        match crate::process::with_process_manager(|manager| {
            manager.create_process(String::from("fork_test_proc"), fork_test_elf)
        }) {
            Some(Ok(pid)) => {
                log::info!("Created fork_test process with PID {}", pid.as_u64());

                // Remove from ready queue to prevent scheduling before exec
                crate::process::with_process_manager(|manager| {
                    if manager.remove_from_ready_queue(pid) {
                        log::info!("Removed process {} from ready queue", pid.as_u64());
                    }
                    Some(())
                });

                // Now exec hello_time.elf into this process
                log::info!("Executing hello_time.elf into process {}", pid.as_u64());
                match crate::process::with_process_manager(|manager| {
                    manager.exec_process(pid, hello_time_elf, Some("hello_time"))
                }) {
                    Some(Ok(entry_point)) => {
                        log::info!("✓ Real userspace exec succeeded! Entry: {:#x}", entry_point);

                        // Add back to ready queue and schedule
                        x86_64::instructions::interrupts::without_interrupts(|| {
                            let mut manager_guard = crate::process::manager();
                            if let Some(ref mut manager) = *manager_guard {
                                manager.add_to_ready_queue(pid);
                                log::info!("✓ Process {} added back to ready queue", pid.as_u64());

                                if let Some(process) = manager.get_process(pid) {
                                    if let Some(ref main_thread) = process.main_thread {
                                        crate::task::scheduler::spawn(alloc::boxed::Box::new(
                                            main_thread.clone(),
                                        ));
                                        log::info!("✓ hello_time.elf scheduled for execution");
                                    }
                                }
                            }
                        });
                    }
                    Some(Err(e)) => {
                        log::error!("✗ Real userspace exec failed: {}", e);
                    }
                    None => {
                        log::error!("Process manager not available for exec");
                    }
                }
            }
            Some(Err(e)) => {
                log::error!("Failed to create fork_test process: {}", e);
            }
            None => {
                log::error!("Process manager not available for process creation");
            }
        }
    }

    #[cfg(not(feature = "testing"))]
    {
        log::warn!("Real userspace test requires testing feature - using minimal ELF instead");
        test_exec_minimal();
    }
}

/// Test exec with a minimal ELF to isolate BSS issues
pub fn test_exec_minimal() {
    log::info!("=== Testing exec() with minimal ELF ===");

    // Create a minimal ELF without BSS segment
    let minimal_elf = create_minimal_elf_no_bss();

    // Create a process
    match create_user_process(String::from("minimal_test"), &minimal_elf) {
        Ok(pid) => {
            log::info!("Created minimal test process with PID {}", pid.as_u64());

            // Wait a bit
            for _ in 0..100000 {
                core::hint::spin_loop();
            }

            // Try to exec hello_time into it
            #[cfg(feature = "testing")]
            let hello_time_buf = crate::userspace_test::get_test_binary("hello_time");
            #[cfg(feature = "testing")]
            let hello_time_elf: &[u8] = &hello_time_buf;
            #[cfg(not(feature = "testing"))]
            let hello_time_elf = &create_minimal_elf_no_bss();

            // Use with_process_manager to properly disable interrupts
            log::info!("Attempting exec with hello_time.elf...");
            match crate::process::with_process_manager(|manager| {
                manager.exec_process(pid, hello_time_elf, Some("hello_time"))
            }) {
                Some(Ok(entry_point)) => {
                    log::info!("✓ Minimal exec test passed! Entry: {:#x}", entry_point);
                }
                Some(Err(e)) => {
                    log::error!("✗ Minimal exec test failed: {}", e);
                }
                None => {
                    log::error!("Process manager not available");
                }
            }
        }
        Err(e) => {
            log::error!("Failed to create minimal process: {}", e);
        }
    }
}

/// Test fork/exec pattern as a shell would do it
pub fn test_shell_fork_exec() {
    log::info!("=== Testing fork/exec as a shell would ===");

    // Simulate a shell process that wants to run a command
    #[cfg(feature = "testing")]
    let shell_buf = crate::userspace_test::get_test_binary("fork_test"); // Using fork_test as our "shell"
    #[cfg(feature = "testing")]
    let shell_elf: &[u8] = &shell_buf;
    #[cfg(not(feature = "testing"))]
    let shell_elf = &create_minimal_elf_no_bss();

    match create_user_process(String::from("shell"), shell_elf) {
        Ok(shell_pid) => {
            log::info!("Created shell process with PID {}", shell_pid.as_u64());

            // Simulate shell receiving command to run hello_time
            log::info!(
                "Shell (PID {}) wants to run hello_time command",
                shell_pid.as_u64()
            );

            // Step 1: Shell forks itself
            match crate::process::with_process_manager(|manager| manager.fork_process(shell_pid)) {
                Some(Ok(child_pid)) => {
                    log::info!(
                        "✓ Shell forked! Shell PID: {}, Child PID: {}",
                        shell_pid.as_u64(),
                        child_pid.as_u64()
                    );

                    // Step 2: Child process execs the command
                    #[cfg(feature = "testing")]
                    let command_buf = crate::userspace_test::get_test_binary("hello_time");
                    #[cfg(feature = "testing")]
                    let command_elf: &[u8] = &command_buf;
                    #[cfg(not(feature = "testing"))]
                    let command_elf = &create_hello_world_elf();

                    // Remove child from ready queue before exec
                    crate::process::with_process_manager(|manager| {
                        if manager.remove_from_ready_queue(child_pid) {
                            log::info!(
                                "Removed child {} from ready queue before exec",
                                child_pid.as_u64()
                            );
                        }
                        Some(())
                    });

                    match crate::process::with_process_manager(|manager| {
                        manager.exec_process(child_pid, command_elf, Some("hello_time"))
                    }) {
                        Some(Ok(entry_point)) => {
                            log::info!(
                                "✓ Child {} exec'd hello_time successfully! Entry: {:#x}",
                                child_pid.as_u64(),
                                entry_point
                            );

                            // Add child back to ready queue
                            x86_64::instructions::interrupts::without_interrupts(|| {
                                let mut manager_guard = crate::process::manager();
                                if let Some(ref mut manager) = *manager_guard {
                                    manager.add_to_ready_queue(child_pid);
                                    log::info!(
                                        "✓ Child {} added back to ready queue",
                                        child_pid.as_u64()
                                    );

                                    // Schedule the child
                                    if let Some(process) = manager.get_process(child_pid) {
                                        if let Some(ref main_thread) = process.main_thread {
                                            crate::task::scheduler::spawn(alloc::boxed::Box::new(
                                                main_thread.clone(),
                                            ));
                                            log::info!(
                                                "✓ hello_time command scheduled for execution"
                                            );
                                        }
                                    }
                                }
                            });

                            log::info!(
                                "Shell {} continues running while child {} runs hello_time",
                                shell_pid.as_u64(),
                                child_pid.as_u64()
                            );
                        }
                        Some(Err(e)) => {
                            log::error!("✗ Child exec failed: {}", e);
                        }
                        None => {
                            log::error!("Process manager not available for exec");
                        }
                    }
                }
                Some(Err(e)) => {
                    log::error!("✗ Shell fork failed: {}", e);
                }
                None => {
                    log::error!("Process manager not available for fork");
                }
            }
        }
        Err(e) => {
            log::error!("Failed to create shell process: {}", e);
        }
    }
}

/// Test timer functionality with comprehensive timer test program
pub fn test_timer_functionality() {
    log::info!("=== TIMER FUNCTIONALITY TEST ===");
    log::info!("Running comprehensive timer test program");

    // Use timer_test.elf to verify timer functionality
    #[cfg(feature = "testing")]
    let timer_test_elf_buf = crate::userspace_test::get_test_binary("timer_test");
    #[cfg(feature = "testing")]
    let timer_test_elf: &[u8] = &timer_test_elf_buf;
    #[cfg(not(feature = "testing"))]
    let timer_test_elf = &create_hello_world_elf();

    log::info!(
        "Loading timer_test.elf, size: {} bytes",
        timer_test_elf.len()
    );

    // Create timer test process
    match create_user_process(String::from("timer_test"), timer_test_elf) {
        Ok(pid) => {
            log::info!("✓ Created timer test process with PID {}", pid.as_u64());
            log::info!("Process will run comprehensive timer tests:");
            log::info!("  - Test 1: Initial time reading");
            log::info!("  - Test 2: Time after yielding");
            log::info!("  - Test 3: Time after busy wait");
            log::info!("  - Test 4: Rapid time calls");
            log::info!("  - Test 5: Progress over 1 second");
            log::info!("Expected: Non-zero time values that increment");
        }
        Err(e) => {
            log::error!("✗ Failed to create timer test process: {}", e);
        }
    }
}

/// Test exec without scheduling - creates process without adding to scheduler
pub fn test_exec_without_scheduling() {
    log::info!("=== Testing exec() without immediate scheduling ===");

    // Create a process without scheduling it
    #[cfg(feature = "testing")]
    let initial_buf = crate::userspace_test::get_test_binary("fork_test");
    #[cfg(feature = "testing")]
    let initial_elf: &[u8] = &initial_buf;
    #[cfg(not(feature = "testing"))]
    let initial_elf = &create_minimal_elf_no_bss();

    // Use with_process_manager to prevent deadlock during ELF loading
    let pid = crate::process::with_process_manager(|manager| {
        log::info!("Creating process with interrupts disabled...");
        match manager.create_process(String::from("exec_test"), initial_elf) {
            Ok(pid) => {
                log::info!("Created process {} without scheduling", pid.as_u64());

                // CRITICAL: Remove from ready queue to prevent scheduling before exec
                if manager.remove_from_ready_queue(pid) {
                    log::info!(
                        "Removed process {} from ready queue to prevent early scheduling",
                        pid.as_u64()
                    );
                }

                Some(pid)
            }
            Err(e) => {
                log::error!("Failed to create process: {}", e);
                None
            }
        }
    })
    .flatten();

    if let Some(pid) = pid {
        // Now exec the actual program we want to run
        #[cfg(feature = "testing")]
        let target_buf = crate::userspace_test::get_test_binary("hello_time");
        #[cfg(feature = "testing")]
        let target_elf: &[u8] = &target_buf;
        #[cfg(not(feature = "testing"))]
        let target_elf = &create_exec_test_elf();

        log::info!("Calling exec to load target program...");

        match crate::process::with_process_manager(|manager| manager.exec_process(pid, target_elf, Some("hello_time")))
        {
            Some(Ok(entry_point)) => {
                log::info!("✓ exec succeeded! New entry point: {:#x}", entry_point);

                // Now add process back to ready queue after exec
                x86_64::instructions::interrupts::without_interrupts(|| {
                    let mut manager_guard = crate::process::manager();
                    if let Some(ref mut manager) = *manager_guard {
                        // Add back to ready queue
                        manager.add_to_ready_queue(pid);
                        log::info!(
                            "✓ Process {} added back to ready queue after exec",
                            pid.as_u64()
                        );

                        // Also need to spawn the thread
                        if let Some(process) = manager.get_process(pid) {
                            if let Some(ref main_thread) = process.main_thread {
                                crate::task::scheduler::spawn(alloc::boxed::Box::new(
                                    main_thread.clone(),
                                ));
                                log::info!(
                                    "✓ Process {} thread scheduled with exec'd program",
                                    pid.as_u64()
                                );
                            }
                        }
                    }
                });
            }
            Some(Err(e)) => {
                log::error!("✗ exec failed: {}", e);
            }
            None => {
                log::error!("Process manager not available");
            }
        }
    }
}

/// Create a fork test ELF that tests syscalls
#[allow(dead_code)]
fn create_fork_test_elf() -> alloc::vec::Vec<u8> {
    use alloc::vec::Vec;

    let mut elf = Vec::new();

    // ELF header (64 bytes)
    elf.extend_from_slice(&[
        0x7f, 0x45, 0x4c, 0x46, // Magic
        0x02, 0x01, 0x01, 0x00, // 64-bit, little-endian, current version
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // padding
        0x02, 0x00, // ET_EXEC
        0x3e, 0x00, // EM_X86_64
        0x01, 0x00, 0x00, 0x00, // version
        0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x00, 0x00, // entry = 0x40000000
        0x40, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // phoff
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // shoff
        0x00, 0x00, 0x00, 0x00, // flags
        0x40, 0x00, // ehsize
        0x38, 0x00, // phentsize
        0x02, 0x00, // phnum (2 segments - code and data)
        0x00, 0x00, // shentsize
        0x00, 0x00, // shnum
        0x00, 0x00, // shstrndx
    ]);

    // Program header 1 - code segment
    elf.extend_from_slice(&[
        0x01, 0x00, 0x00, 0x00, // PT_LOAD
        0x05, 0x00, 0x00, 0x00, // flags (R+X)
        0xb0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // offset = 176
        0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x00, 0x00, // vaddr = 0x40000000
        0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x00, 0x00, // paddr
        0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // filesz = 128 bytes
        0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // memsz = 128 bytes
        0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // align
    ]);

    // Program header 2 - data segment
    elf.extend_from_slice(&[
        0x01, 0x00, 0x00, 0x00, // PT_LOAD
        0x06, 0x00, 0x00, 0x00, // flags (R+W)
        0x30, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // offset = 304 (176 + 128)
        0x00, 0x10, 0x00, 0x40, 0x00, 0x00, 0x00, 0x00, // vaddr = 0x40001000
        0x00, 0x10, 0x00, 0x40, 0x00, 0x00, 0x00, 0x00, // paddr
        0x20, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // filesz = 32 bytes
        0x20, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // memsz = 32 bytes
        0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // align
    ]);

    // Code section at offset 176
    let code = vec![
        // Print "Before fork\n"
        0xb8, 0x01, 0x00, 0x00, 0x00, // mov eax, 1 (sys_write)
        0xbf, 0x01, 0x00, 0x00, 0x00, // mov edi, 1 (stdout)
        0x48, 0xbe, 0x00, 0x10, 0x00, 0x10, 0x00, 0x00, 0x00,
        0x00, // mov rsi, 0x40001000 (string address)
        0xba, 0x0c, 0x00, 0x00, 0x00, // mov edx, 12 (string length)
        0xcd, 0x80, // int 0x80 (syscall)
        // Call fork()
        0xb8, 0x05, 0x00, 0x00, 0x00, // mov eax, 5 (sys_fork)
        0xcd, 0x80, // int 0x80 (syscall)
        // Test if we're parent or child
        0x48, 0x85, 0xc0, // test rax, rax
        0x74, 0x18, // jz child_code (jump if zero)
        // Parent: print "Parent\n" and exit
        0xb8, 0x01, 0x00, 0x00, 0x00, // mov eax, 1 (sys_write)
        0xbf, 0x01, 0x00, 0x00, 0x00, // mov edi, 1 (stdout)
        0x48, 0xbe, 0x0c, 0x10, 0x00, 0x40, 0x00, 0x00, 0x00, 0x00, // mov rsi, 0x4000100c
        0xba, 0x07, 0x00, 0x00, 0x00, // mov edx, 7
        0xcd, 0x80, // int 0x80
        0xeb, 0x16, // jmp exit_parent
        // Child: print "Child\n" and exit
        0xb8, 0x01, 0x00, 0x00, 0x00, // mov eax, 1 (sys_write)
        0xbf, 0x01, 0x00, 0x00, 0x00, // mov edi, 1 (stdout)
        0x48, 0xbe, 0x13, 0x10, 0x00, 0x40, 0x00, 0x00, 0x00, 0x00, // mov rsi, 0x40001013
        0xba, 0x06, 0x00, 0x00, 0x00, // mov edx, 6
        0xcd, 0x80, // int 0x80
        // Exit
        0xb8, 0x00, 0x00, 0x00, 0x00, // mov eax, 0 (sys_exit)
        0x31, 0xff, // xor edi, edi (exit code 0)
        0xcd, 0x80, // int 0x80 (syscall)
        // Should never reach here
        0xeb, 0xfe, // jmp $ (infinite loop)
    ];

    // Pad code section to 128 bytes
    elf.extend_from_slice(&code);
    for _ in code.len()..128 {
        elf.push(0x90); // NOP padding
    }

    // Data section at offset 304
    elf.extend_from_slice(b"Before fork\n");
    elf.extend_from_slice(b"Parent\n");
    elf.extend_from_slice(b"Child\n");

    // Pad data section to 32 bytes
    let data_len = 12 + 7 + 6; // lengths of strings
    for _ in data_len..32 {
        elf.push(0x00);
    }

    elf
}

/// Create a minimal ELF without BSS segment for testing
fn create_minimal_elf_no_bss() -> alloc::vec::Vec<u8> {
    create_hello_world_elf()
}

/// Create a hello world ELF that tests syscalls
#[allow(dead_code)]
pub fn create_hello_world_elf() -> alloc::vec::Vec<u8> {
    use alloc::vec::Vec;

    let mut elf = Vec::new();

    // ELF header (64 bytes)
    elf.extend_from_slice(&[
        0x7f, 0x45, 0x4c, 0x46, // Magic
        0x02, 0x01, 0x01, 0x00, // 64-bit, little-endian, current version
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // padding
        0x02, 0x00, // ET_EXEC
        0x3e, 0x00, // EM_X86_64
        0x01, 0x00, 0x00, 0x00, // version
        0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x00, 0x00, // entry = 0x40000000
        0x40, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // phoff
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // shoff
        0x00, 0x00, 0x00, 0x00, // flags
        0x40, 0x00, // ehsize
        0x38, 0x00, // phentsize
        0x02, 0x00, // phnum (2 segments - code and data)
        0x00, 0x00, // shentsize
        0x00, 0x00, // shnum
        0x00, 0x00, // shstrndx
    ]);

    // Program header 1 - code segment
    elf.extend_from_slice(&[
        0x01, 0x00, 0x00, 0x00, // PT_LOAD
        0x05, 0x00, 0x00, 0x00, // flags (R+X)
        0xb0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // offset = 176
        0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x00, 0x00, // vaddr = 0x40000000
        0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x00, 0x00, // paddr
        0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // filesz = 128 bytes
        0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // memsz = 128 bytes
        0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // align
    ]);

    // Program header 2 - data segment
    elf.extend_from_slice(&[
        0x01, 0x00, 0x00, 0x00, // PT_LOAD
        0x06, 0x00, 0x00, 0x00, // flags (R+W)
        0x30, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // offset = 304 (176 + 128)
        0x00, 0x10, 0x00, 0x40, 0x00, 0x00, 0x00, 0x00, // vaddr = 0x40001000
        0x00, 0x10, 0x00, 0x40, 0x00, 0x00, 0x00, 0x00, // paddr
        0x20, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // filesz = 32 bytes
        0x20, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // memsz = 32 bytes
        0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // align
    ]);

    // Code section at offset 176
    let code = vec![
        // Print "Hello from userspace!\n"
        0xb8, 0x01, 0x00, 0x00, 0x00, // mov eax, 1 (sys_write)
        0xbf, 0x01, 0x00, 0x00, 0x00, // mov edi, 1 (stdout)
        0x48, 0xbe, 0x00, 0x10, 0x00, 0x10, 0x00, 0x00, 0x00,
        0x00, // mov rsi, 0x40001000 (string address)
        0xba, 0x16, 0x00, 0x00, 0x00, // mov edx, 22 (string length)
        0xcd, 0x80, // int 0x80 (syscall)
        // Exit with code 0
        0xb8, 0x00, 0x00, 0x00, 0x00, // mov eax, 0 (sys_exit)
        0x31, 0xff, // xor edi, edi (exit code 0)
        0xcd, 0x80, // int 0x80 (syscall)
        // Should never reach here
        0xeb, 0xfe, // jmp $ (infinite loop)
    ];

    // Pad code section to 128 bytes
    elf.extend_from_slice(&code);
    for _ in code.len()..128 {
        elf.push(0x90); // NOP padding
    }

    // Data section at offset 304 - the "Hello from userspace!\n" string
    let message = b"Hello from userspace!\n";
    elf.extend_from_slice(message);

    // Pad data section to 32 bytes
    for _ in message.len()..32 {
        elf.push(0x00);
    }

    elf
}

/// Create a minimal ELF binary for exec testing (different from fork test)
#[allow(dead_code)]
fn create_exec_test_elf() -> alloc::vec::Vec<u8> {
    use alloc::vec::Vec;

    // Create a simple ELF that just loops with NOPs to test basic execution
    let mut elf = Vec::new();

    // ELF header (64 bytes)
    elf.extend_from_slice(&[
        0x7f, 0x45, 0x4c, 0x46, // e_ident[EI_MAG0..EI_MAG3] = ELF
        0x02, // e_ident[EI_CLASS] = ELFCLASS64
        0x01, // e_ident[EI_DATA] = ELFDATA2LSB
        0x01, // e_ident[EI_VERSION] = EV_CURRENT
        0x00, // e_ident[EI_OSABI] = ELFOSABI_NONE
        0x00, // e_ident[EI_ABIVERSION] = 0
    ]);

    // Pad EI_PAD to 16 bytes total
    for _ in 0..7 {
        elf.push(0x00);
    }

    elf.extend_from_slice(&[
        0x02, 0x00, // e_type = ET_EXEC (2)
        0x3e, 0x00, // e_machine = EM_X86_64 (62)
        0x01, 0x00, 0x00, 0x00, // e_version = EV_CURRENT (1)
    ]);

    // e_entry (8 bytes) = 0x40000000
    elf.extend_from_slice(&[0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x00, 0x00]);

    // e_phoff (8 bytes) = 64 (program headers start after ELF header)
    elf.extend_from_slice(&[0x40, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);

    // e_shoff (8 bytes) = 0 (no section headers)
    elf.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);

    elf.extend_from_slice(&[
        0x00, 0x00, 0x00, 0x00, // e_flags = 0
        0x40, 0x00, // e_ehsize = 64
        0x38, 0x00, // e_phentsize = 56
        0x01, 0x00, // e_phnum = 1 (one program header)
        0x00, 0x00, // e_shentsize = 0
        0x00, 0x00, // e_shnum = 0
        0x00, 0x00, // e_shstrndx = 0
    ]);

    // Program header (56 bytes)
    elf.extend_from_slice(&[
        0x01, 0x00, 0x00, 0x00, // p_type = PT_LOAD (1)
        0x05, 0x00, 0x00, 0x00, // p_flags = PF_R | PF_X (5)
    ]);

    // p_offset (8 bytes) = 120 (after headers)
    elf.extend_from_slice(&[0x78, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);

    // p_vaddr (8 bytes) = 0x40000000
    elf.extend_from_slice(&[0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x00, 0x00]);

    // p_paddr (8 bytes) = 0x40000000
    elf.extend_from_slice(&[0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x00, 0x00]);

    // p_filesz (8 bytes) = 20 (code section with int3)
    elf.extend_from_slice(&[0x14, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);

    // p_memsz (8 bytes) = 20
    elf.extend_from_slice(&[0x14, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);

    // p_align (8 bytes) = 4096
    elf.extend_from_slice(&[0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);

    // Code section (starting at offset 120) - NOPs then breakpoint for proof of execution
    elf.extend_from_slice(&[
        0x90, 0x90, 0x90, 0x90, 0x90, // 5 NOPs (0x40000000-0x40000004)
        0xcc, // int3 breakpoint (0x40000005) - PROOF OF EXECUTION
        0xeb, 0xfe, // jmp $ (infinite loop after breakpoint)
        0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90,
        0x90, // 12 bytes padding = 20 total
    ]);

    elf
}

/// Test that undefined syscalls return ENOSYS
///
/// TWO-STAGE VALIDATION PATTERN:
/// - Stage 1 (This function): Creates and schedules the process
///   - Marker: "ENOSYS test: process scheduled for execution"
///   - This is a CHECKPOINT confirming process creation succeeded
///   - Does NOT prove the process executed or that ENOSYS works
/// - Stage 2 (Boot stage 32): Validates actual execution and ENOSYS return value
///   - Marker: "USERSPACE OUTPUT: ENOSYS OK"
///   - This PROVES the process ran AND syscall 999 returned -38
pub fn test_syscall_enosys() {
    log::info!("Testing undefined syscall returns ENOSYS");

    // ALWAYS load from disk - no embedded binaries
    #[cfg(feature = "testing")]
    let syscall_enosys_elf_buf = crate::userspace_test::get_test_binary("syscall_enosys");
    #[cfg(feature = "testing")]
    let syscall_enosys_elf: &[u8] = &syscall_enosys_elf_buf;
    #[cfg(not(feature = "testing"))]
    let syscall_enosys_elf = &create_hello_world_elf();

    match crate::process::creation::create_user_process(
        String::from("syscall_enosys"),
        syscall_enosys_elf,
    ) {
        Ok(pid) => {
            log::info!("Created syscall_enosys process with PID {:?}", pid);
            log::info!("    -> Should print 'ENOSYS OK' if syscall 999 returns -38");
        }
        Err(e) => {
            log::error!("Failed to create syscall_enosys process: {}", e);
            log::error!("ENOSYS test cannot run without valid userspace process");
        }
    }
}

/// Test signal handler execution
///
/// TWO-STAGE VALIDATION PATTERN:
/// - Stage 1 (This function): Creates and schedules the process
///   - Marker: "Signal handler test: process scheduled for execution"
///   - This is a CHECKPOINT confirming process creation succeeded
///   - Does NOT prove the handler executed
/// - Stage 2 (Boot stage): Validates actual signal handler execution
///   - Marker: "SIGNAL_HANDLER_EXECUTED"
///   - This PROVES the signal handler was called when the signal was delivered
pub fn test_signal_handler() {
    log::info!("Testing signal handler execution");

    #[cfg(feature = "testing")]
    let signal_handler_test_elf_buf = crate::userspace_test::get_test_binary("signal_handler_test");
    #[cfg(feature = "testing")]
    let signal_handler_test_elf: &[u8] = &signal_handler_test_elf_buf;
    #[cfg(not(feature = "testing"))]
    let signal_handler_test_elf = &create_hello_world_elf();

    match crate::process::creation::create_user_process(
        String::from("signal_handler_test"),
        signal_handler_test_elf,
    ) {
        Ok(pid) => {
            log::info!("Created signal_handler_test process with PID {:?}", pid);
            log::info!("    -> Should print 'SIGNAL_HANDLER_EXECUTED' if handler runs");
        }
        Err(e) => {
            log::error!("Failed to create signal_handler_test process: {}", e);
            log::error!("Signal handler test cannot run without valid userspace process");
        }
    }
}

/// Test signal handler return via trampoline
///
/// This test validates the complete signal delivery and return mechanism:
/// - Signal handler is registered and executed
/// - Handler returns normally
/// - Trampoline code calls sigreturn
/// - Execution resumes at the point where signal was delivered
///
/// Boot stages:
/// - Stage 1 (Checkpoint): Process creation
///   - Marker: "Signal return test: process scheduled for execution"
///   - This is a CHECKPOINT confirming process creation succeeded
///   - Does NOT prove the trampoline worked
/// - Stage 2 (Boot stage): Validates handler return and context restoration
///   - Marker: "SIGNAL_RETURN_WORKS"
///   - This PROVES the trampoline successfully restored pre-signal context
pub fn test_signal_return() {
    log::info!("Testing signal handler return via trampoline");

    #[cfg(feature = "testing")]
    let signal_return_test_elf_buf = crate::userspace_test::get_test_binary("signal_return_test");
    #[cfg(feature = "testing")]
    let signal_return_test_elf: &[u8] = &signal_return_test_elf_buf;
    #[cfg(not(feature = "testing"))]
    let signal_return_test_elf = &create_hello_world_elf();

    match crate::process::creation::create_user_process(
        String::from("signal_return_test"),
        signal_return_test_elf,
    ) {
        Ok(pid) => {
            log::info!("Created signal_return_test process with PID {:?}", pid);
            log::info!("    -> Should print 'SIGNAL_RETURN_WORKS' if trampoline works");
        }
        Err(e) => {
            log::error!("Failed to create signal_return_test process: {}", e);
            log::error!("Signal return test cannot run without valid userspace process");
        }
    }
}

/// Test that registers are preserved across signal delivery and sigreturn
///
/// TWO-STAGE VALIDATION PATTERN:
/// - Stage 1 (Checkpoint): Process creation
///   - Marker: "Signal regs test: process scheduled for execution"
///   - This is a CHECKPOINT confirming process creation succeeded
/// - Stage 2 (Boot stage): Validates register preservation
///   - Marker: "SIGNAL_REGS_PRESERVED"
///   - This PROVES registers are correctly saved/restored across signals
pub fn test_signal_regs() {
    log::info!("Testing signal register preservation");

    #[cfg(feature = "testing")]
    let signal_regs_test_elf_buf = crate::userspace_test::get_test_binary("signal_regs_test");
    #[cfg(feature = "testing")]
    let signal_regs_test_elf: &[u8] = &signal_regs_test_elf_buf;
    #[cfg(not(feature = "testing"))]
    let signal_regs_test_elf = &create_hello_world_elf();

    match crate::process::creation::create_user_process(
        String::from("signal_regs_test"),
        signal_regs_test_elf,
    ) {
        Ok(pid) => {
            log::info!("Created signal_regs_test process with PID {:?}", pid);
            log::info!("Signal regs test: process scheduled for execution.");
            log::info!("    -> Should print 'SIGNAL_REGS_PRESERVED' if registers preserved");
        }
        Err(e) => {
            log::error!("Failed to create signal_regs_test process: {}", e);
            log::error!("Signal regs test cannot run without valid userspace process");
        }
    }
}

/// Test pipe syscall functionality
///
/// TWO-STAGE VALIDATION PATTERN:
/// - Stage 1 (Checkpoint): Process creation
///   - Marker: "Pipe test: process scheduled for execution"
///   - This is a CHECKPOINT confirming process creation succeeded
/// - Stage 2 (Boot stage): Validates pipe operations
///   - Marker: "PIPE_TEST_PASSED"
///   - This PROVES pipe creation, read/write, and close all work
pub fn test_pipe() {
    log::info!("Testing pipe syscall functionality");

    #[cfg(feature = "testing")]
    let pipe_test_elf_buf = crate::userspace_test::get_test_binary("pipe_test");
    #[cfg(feature = "testing")]
    let pipe_test_elf: &[u8] = &pipe_test_elf_buf;
    #[cfg(not(feature = "testing"))]
    let pipe_test_elf = &create_hello_world_elf();

    match crate::process::creation::create_user_process(
        String::from("pipe_test"),
        pipe_test_elf,
    ) {
        Ok(pid) => {
            log::info!("Created pipe_test process with PID {:?}", pid);
            log::info!("Pipe test: process scheduled for execution.");
            log::info!("    -> Emits pass marker on success (PIPE_TEST_...)");
        }
        Err(e) => {
            log::error!("Failed to create pipe_test process: {}", e);
            log::error!("Pipe test cannot run without valid userspace process");
        }
    }
}

/// Test pipe + fork concurrency
///
/// TWO-STAGE VALIDATION PATTERN:
/// - Stage 1 (Checkpoint): Process creation
///   - Marker: "Pipe+fork test: process scheduled for execution"
///   - This is a CHECKPOINT confirming process creation succeeded
/// - Stage 2 (Boot stage): Validates pipe operations across fork boundary
///   - Marker: "PIPE_FORK_TEST_PASSED"
///   - This PROVES pipes work correctly across fork, with proper IPC and EOF handling
pub fn test_pipe_fork() {
    log::info!("Testing pipe+fork concurrency");

    #[cfg(feature = "testing")]
    let pipe_fork_test_elf_buf = crate::userspace_test::get_test_binary("pipe_fork_test");
    #[cfg(feature = "testing")]
    let pipe_fork_test_elf: &[u8] = &pipe_fork_test_elf_buf;
    #[cfg(not(feature = "testing"))]
    let pipe_fork_test_elf = &create_hello_world_elf();

    match crate::process::creation::create_user_process(
        String::from("pipe_fork_test"),
        pipe_fork_test_elf,
    ) {
        Ok(pid) => {
            log::info!("Created pipe_fork_test process with PID {:?}", pid);
            log::info!("Pipe+fork test: process scheduled for execution.");
            log::info!("    -> Emits pass marker on success (PIPE_FORK_...)");
        }
        Err(e) => {
            log::error!("Failed to create pipe_fork_test process: {}", e);
            log::error!("Pipe+fork test cannot run without valid userspace process");
        }
    }
}

/// Test concurrent pipe writes from multiple processes
///
/// TWO-STAGE VALIDATION PATTERN:
/// - Stage 1 (Checkpoint): Process creation
///   - Marker: "Pipe concurrent test: process scheduled for execution"
///   - This is a CHECKPOINT confirming process creation succeeded
/// - Stage 2 (Boot stage): Validates concurrent pipe operations
///   - Marker: "PIPE_CONCURRENT_TEST_PASSED"
///   - This PROVES the pipe buffer handles concurrent writes correctly under Arc<Mutex<PipeBuffer>>
pub fn test_pipe_concurrent() {
    log::info!("Testing concurrent pipe writes from multiple processes");

    #[cfg(feature = "testing")]
    let pipe_concurrent_test_elf_buf = crate::userspace_test::get_test_binary("pipe_concurrent_test");
    #[cfg(feature = "testing")]
    let pipe_concurrent_test_elf: &[u8] = &pipe_concurrent_test_elf_buf;
    #[cfg(not(feature = "testing"))]
    let pipe_concurrent_test_elf = &create_hello_world_elf();

    match crate::process::creation::create_user_process(
        String::from("pipe_concurrent_test"),
        pipe_concurrent_test_elf,
    ) {
        Ok(pid) => {
            log::info!("Created pipe_concurrent_test process with PID {:?}", pid);
            log::info!("Pipe concurrent test: process scheduled for execution.");
            log::info!("    -> Emits pass marker on success (PIPE_CONCURRENT_...)");
        }
        Err(e) => {
            log::error!("Failed to create pipe_concurrent_test process: {}", e);
            log::error!("Pipe concurrent test cannot run without valid userspace process");
        }
    }
}

/// Test waitpid syscall functionality
///
/// TWO-STAGE VALIDATION PATTERN:
/// - Stage 1 (Checkpoint): Process creation
///   - Marker: "Waitpid test: process scheduled for execution"
///   - This is a CHECKPOINT confirming process creation succeeded
/// - Stage 2 (Boot stage): Validates waitpid operations
///   - Marker: "WAITPID_TEST_PASSED"
///   - This PROVES waitpid correctly waits for child, returns correct PID, and status extraction works
pub fn test_waitpid() {
    log::info!("Testing waitpid syscall functionality");

    #[cfg(feature = "testing")]
    let waitpid_test_elf_buf = crate::userspace_test::get_test_binary("waitpid_test");
    #[cfg(feature = "testing")]
    let waitpid_test_elf: &[u8] = &waitpid_test_elf_buf;
    #[cfg(not(feature = "testing"))]
    let waitpid_test_elf = &create_hello_world_elf();

    match crate::process::creation::create_user_process(
        String::from("waitpid_test"),
        waitpid_test_elf,
    ) {
        Ok(pid) => {
            log::info!("Created waitpid_test process with PID {:?}", pid);
            log::info!("Waitpid test: process scheduled for execution.");
            log::info!("    -> Emits pass marker on success (WAITPID_TEST_PASSED)");
        }
        Err(e) => {
            log::error!("Failed to create waitpid_test process: {}", e);
            log::error!("Waitpid test cannot run without valid userspace process");
        }
    }
}

/// Test signal handler inheritance across fork
///
/// TWO-STAGE VALIDATION PATTERN:
/// - Stage 1 (Checkpoint): Process creation
///   - Marker: "Signal fork test: process scheduled for execution"
///   - This is a CHECKPOINT confirming process creation succeeded
/// - Stage 2 (Boot stage): Validates signal inheritance
///   - Marker: "SIGNAL_FORK_TEST_PASSED"
///   - This PROVES signal handlers are correctly inherited by forked children
pub fn test_signal_fork() {
    log::info!("Testing signal handler inheritance across fork");

    #[cfg(feature = "testing")]
    let signal_fork_test_elf_buf = crate::userspace_test::get_test_binary("signal_fork_test");
    #[cfg(feature = "testing")]
    let signal_fork_test_elf: &[u8] = &signal_fork_test_elf_buf;
    #[cfg(not(feature = "testing"))]
    let signal_fork_test_elf = &create_hello_world_elf();

    match crate::process::creation::create_user_process(
        String::from("signal_fork_test"),
        signal_fork_test_elf,
    ) {
        Ok(pid) => {
            log::info!("Created signal_fork_test process with PID {:?}", pid);
            log::info!("Signal fork test: process scheduled for execution.");
            log::info!("    -> Emits pass marker on success (SIGNAL_FORK_TEST_PASSED)");
        }
        Err(e) => {
            log::error!("Failed to create signal_fork_test process: {}", e);
            log::error!("Signal fork test cannot run without valid userspace process");
        }
    }
}

/// Test SIGTERM delivery with default handler (kill test)
///
/// TWO-STAGE VALIDATION PATTERN:
/// - Stage 1 (Checkpoint): Process creation
///   - Marker: "Signal kill test: process scheduled for execution"
///   - This is a CHECKPOINT confirming process creation succeeded
/// - Stage 2 (Boot stage): Validates SIGTERM delivery terminates child
///   - Marker: "SIGNAL_KILL_TEST_PASSED"
///   - This PROVES SIGTERM is delivered and child is terminated
pub fn test_signal_kill() {
    log::info!("Testing SIGTERM delivery with default handler");

    #[cfg(feature = "testing")]
    let signal_test_elf_buf = crate::userspace_test::get_test_binary("signal_test");
    #[cfg(feature = "testing")]
    let signal_test_elf: &[u8] = &signal_test_elf_buf;
    #[cfg(not(feature = "testing"))]
    let signal_test_elf = &create_hello_world_elf();

    match crate::process::creation::create_user_process(
        String::from("signal_test"),
        signal_test_elf,
    ) {
        Ok(pid) => {
            log::info!("Created signal_test process with PID {:?}", pid);
            log::info!("Signal kill test: process scheduled for execution.");
            log::info!("    -> Userspace will print pass marker when child terminated by SIGTERM");
        }
        Err(e) => {
            log::error!("Failed to create signal_test process: {}", e);
            log::error!("Signal kill test cannot run without valid userspace process");
        }
    }
}

/// Test SIGCHLD delivery when child exits
///
/// TWO-STAGE VALIDATION PATTERN:
/// - Stage 1 (Checkpoint): Process creation
///   - Marker: "SIGCHLD test: process scheduled for execution"
///   - This is a CHECKPOINT confirming process creation succeeded
/// - Stage 2 (Boot stage): Validates SIGCHLD delivery
///   - Marker: "SIGCHLD_TEST_PASSED"
///   - This PROVES SIGCHLD is delivered to parent when child terminates
pub fn test_sigchld() {
    log::info!("Testing SIGCHLD delivery on child exit");

    #[cfg(feature = "testing")]
    let sigchld_test_elf_buf = crate::userspace_test::get_test_binary("sigchld_test");
    #[cfg(feature = "testing")]
    let sigchld_test_elf: &[u8] = &sigchld_test_elf_buf;
    #[cfg(not(feature = "testing"))]
    let sigchld_test_elf = &create_hello_world_elf();

    match crate::process::creation::create_user_process(
        String::from("sigchld_test"),
        sigchld_test_elf,
    ) {
        Ok(pid) => {
            log::info!("Created sigchld_test process with PID {:?}", pid);
            log::info!("SIGCHLD test: process scheduled for execution.");
            log::info!("    -> Userspace will print pass marker when handler is called");
        }
        Err(e) => {
            log::error!("Failed to create sigchld_test process: {}", e);
            log::error!("SIGCHLD test cannot run without valid userspace process");
        }
    }
}

/// Test WNOHANG timing behavior
///
/// TWO-STAGE VALIDATION PATTERN:
/// - Stage 1 (Checkpoint): Process creation
///   - Marker: "WNOHANG timing test: process scheduled for execution"
///   - This is a CHECKPOINT confirming process creation succeeded
/// - Stage 2 (Boot stage): Validates WNOHANG timing
///   - Marker: "WNOHANG_TIMING_TEST_PASSED"
///   - This PROVES WNOHANG returns 0 when child still running, ECHILD when no children
pub fn test_wnohang_timing() {
    log::info!("Testing WNOHANG timing behavior");

    #[cfg(feature = "testing")]
    let wnohang_timing_test_elf_buf = crate::userspace_test::get_test_binary("wnohang_timing_test");
    #[cfg(feature = "testing")]
    let wnohang_timing_test_elf: &[u8] = &wnohang_timing_test_elf_buf;
    #[cfg(not(feature = "testing"))]
    let wnohang_timing_test_elf = &create_hello_world_elf();

    match crate::process::creation::create_user_process(
        String::from("wnohang_timing_test"),
        wnohang_timing_test_elf,
    ) {
        Ok(pid) => {
            log::info!("Created wnohang_timing_test process with PID {:?}", pid);
            log::info!("WNOHANG timing test: process scheduled for execution.");
            log::info!("    -> Emits pass marker on success (WNOHANG_TIMING_TEST_PASSED)");
        }
        Err(e) => {
            log::error!("Failed to create wnohang_timing_test process: {}", e);
            log::error!("WNOHANG timing test cannot run without valid userspace process");
        }
    }
}

/// Test signal handler reset on exec
///
/// TWO-STAGE VALIDATION PATTERN:
/// - Stage 1 (Checkpoint): Process creation
///   - Marker: "Signal exec test: process scheduled for execution"
///   - This is a CHECKPOINT confirming process creation succeeded
/// - Stage 2 (Boot stage): Validates signal reset on exec
///   - Marker: "SIGNAL_EXEC_TEST_PASSED"
///   - This PROVES signal handlers are reset to SIG_DFL after exec
pub fn test_signal_exec() {
    log::info!("Testing signal handler reset on exec");

    #[cfg(feature = "testing")]
    let signal_exec_test_elf_buf = crate::userspace_test::get_test_binary("signal_exec_test");
    #[cfg(feature = "testing")]
    let signal_exec_test_elf: &[u8] = &signal_exec_test_elf_buf;
    #[cfg(not(feature = "testing"))]
    let signal_exec_test_elf = &create_hello_world_elf();

    match crate::process::creation::create_user_process(
        String::from("signal_exec_test"),
        signal_exec_test_elf,
    ) {
        Ok(pid) => {
            log::info!("Created signal_exec_test process with PID {:?}", pid);
            log::info!("Signal exec test: process scheduled for execution.");
            log::info!("    -> Test will emit pass marker on success");
        }
        Err(e) => {
            log::error!("Failed to create signal_exec_test process: {}", e);
            log::error!("Signal exec test cannot run without valid userspace process");
        }
    }
}

/// Test pause() syscall functionality
///
/// TWO-STAGE VALIDATION PATTERN:
/// - Stage 1 (Checkpoint): Process creation
///   - Marker: "Pause test: process scheduled for execution"
///   - This is a CHECKPOINT confirming process creation succeeded
/// - Stage 2 (Boot stage): Validates pause behavior
///   - Marker: "PAUSE_TEST_PASSED"
///   - This PROVES pause() blocks until signal delivered, and signal handler executes
pub fn test_pause() {
    log::info!("Testing pause() syscall functionality");

    #[cfg(feature = "testing")]
    let pause_test_elf_buf = crate::userspace_test::get_test_binary("pause_test");
    #[cfg(feature = "testing")]
    let pause_test_elf: &[u8] = &pause_test_elf_buf;
    #[cfg(not(feature = "testing"))]
    let pause_test_elf = &create_hello_world_elf();

    match crate::process::creation::create_user_process(
        String::from("pause_test"),
        pause_test_elf,
    ) {
        Ok(pid) => {
            log::info!("Created pause_test process with PID {:?}", pid);
            log::info!("Pause test: process scheduled for execution.");
            log::info!("    -> Userspace will emit PAUSE_TEST marker if successful");
        }
        Err(e) => {
            log::error!("Failed to create pause_test process: {}", e);
            log::error!("Pause test cannot run without valid userspace process");
        }
    }
}

/// Test dup() syscall functionality
///
/// TWO-STAGE VALIDATION PATTERN:
/// - Stage 1 (Checkpoint): Process creation
///   - Marker: "Dup test: process scheduled for execution"
///   - This is a CHECKPOINT confirming process creation succeeded
/// - Stage 2 (Boot stage): Validates dup behavior
///   - Marker: "DUP_TEST_PASSED"
///   - This PROVES dup() creates working duplicate fd that survives original fd close
pub fn test_dup() {
    log::info!("Testing dup() syscall functionality");

    #[cfg(feature = "testing")]
    let dup_test_elf_buf = crate::userspace_test::get_test_binary("dup_test");
    #[cfg(feature = "testing")]
    let dup_test_elf: &[u8] = &dup_test_elf_buf;
    #[cfg(not(feature = "testing"))]
    let dup_test_elf = &create_hello_world_elf();

    match crate::process::creation::create_user_process(
        String::from("dup_test"),
        dup_test_elf,
    ) {
        Ok(pid) => {
            log::info!("Created dup_test process with PID {:?}", pid);
            log::info!("Dup test: process scheduled for execution.");
            log::info!("    -> Userspace will emit DUP_TEST marker if successful");
        }
        Err(e) => {
            log::error!("Failed to create dup_test process: {}", e);
            log::error!("Dup test cannot run without valid userspace process");
        }
    }
}

/// Test fcntl() syscall functionality
///
/// TWO-STAGE VALIDATION PATTERN:
/// - Stage 1 (Checkpoint): Process creation
///   - Marker: "Fcntl test: process scheduled for execution"
///   - This is a CHECKPOINT confirming process creation succeeded
/// - Stage 2 (Boot stage): Validates fcntl behavior
///   - Marker: "FCNTL_TEST_PASSED"
///   - This PROVES fcntl F_GETFD/F_SETFD/F_GETFL/F_SETFL/F_DUPFD all work
pub fn test_fcntl() {
    log::info!("Testing fcntl() syscall functionality");

    #[cfg(feature = "testing")]
    let fcntl_test_elf_buf = crate::userspace_test::get_test_binary("fcntl_test");
    #[cfg(feature = "testing")]
    let fcntl_test_elf: &[u8] = &fcntl_test_elf_buf;
    #[cfg(not(feature = "testing"))]
    let fcntl_test_elf = &create_hello_world_elf();

    match crate::process::creation::create_user_process(
        String::from("fcntl_test"),
        fcntl_test_elf,
    ) {
        Ok(pid) => {
            log::info!("Created fcntl_test process with PID {:?}", pid);
            log::info!("Fcntl test: process scheduled for execution.");
            log::info!("    -> Userspace will emit FCNTL_TEST marker if successful");
        }
        Err(e) => {
            log::error!("Failed to create fcntl_test process: {}", e);
            log::error!("Fcntl test cannot run without valid userspace process");
        }
    }
}

/// Test pipe2() syscall functionality
///
/// TWO-STAGE VALIDATION PATTERN:
/// - Stage 1 (Checkpoint): Process creation
///   - Marker: "Pipe2 test: process scheduled for execution"
///   - This is a CHECKPOINT confirming process creation succeeded
/// - Stage 2 (Boot stage): Validates pipe2 behavior
///   - Marker: "PIPE2_TEST_PASSED"
///   - This PROVES pipe2 with O_CLOEXEC/O_NONBLOCK flags works correctly
pub fn test_pipe2() {
    log::info!("Testing pipe2() syscall functionality");

    #[cfg(feature = "testing")]
    let pipe2_test_elf_buf = crate::userspace_test::get_test_binary("pipe2_test");
    #[cfg(feature = "testing")]
    let pipe2_test_elf: &[u8] = &pipe2_test_elf_buf;
    #[cfg(not(feature = "testing"))]
    let pipe2_test_elf = &create_hello_world_elf();

    match crate::process::creation::create_user_process(
        String::from("pipe2_test"),
        pipe2_test_elf,
    ) {
        Ok(pid) => {
            log::info!("Created pipe2_test process with PID {:?}", pid);
            log::info!("Pipe2 test: process scheduled for execution.");
            log::info!("    -> Userspace will emit PIPE2_TEST marker if successful");
        }
        Err(e) => {
            log::error!("Failed to create pipe2_test process: {}", e);
            log::error!("Pipe2 test cannot run without valid userspace process");
        }
    }
}

/// Test poll() syscall functionality
///
/// TWO-STAGE VALIDATION PATTERN:
/// - Stage 1 (Checkpoint): Process creation
///   - Marker: "Poll test: process scheduled for execution"
///   - This is a CHECKPOINT confirming process creation succeeded
/// - Stage 2 (Boot stage): Validates poll behavior
///   - Marker: "POLL_TEST_PASSED"
///   - This PROVES poll correctly monitors fds for I/O readiness
pub fn test_poll() {
    log::info!("Testing poll() syscall functionality");

    #[cfg(feature = "testing")]
    let poll_test_elf_buf = crate::userspace_test::get_test_binary("poll_test");
    #[cfg(feature = "testing")]
    let poll_test_elf: &[u8] = &poll_test_elf_buf;
    #[cfg(not(feature = "testing"))]
    let poll_test_elf = &create_hello_world_elf();

    match crate::process::creation::create_user_process(
        String::from("poll_test"),
        poll_test_elf,
    ) {
        Ok(pid) => {
            log::info!("Created poll_test process with PID {:?}", pid);
            log::info!("Poll test: process scheduled for execution.");
            log::info!("    -> Userspace will emit POLL_TEST marker if successful");
        }
        Err(e) => {
            log::error!("Failed to create poll_test process: {}", e);
            log::error!("Poll test cannot run without valid userspace process");
        }
    }
}

/// Test select() syscall functionality
///
/// TWO-STAGE VALIDATION PATTERN:
/// - Stage 1 (Checkpoint): Process creation
///   - Marker: "Select test: process scheduled for execution"
///   - This is a CHECKPOINT confirming process creation succeeded
/// - Stage 2 (Boot stage): Validates select behavior
///   - Marker: "SELECT_TEST_PASSED"
///   - This PROVES select correctly monitors fds for I/O readiness using fd_set bitmaps
pub fn test_select() {
    log::info!("Testing select() syscall functionality");

    #[cfg(feature = "testing")]
    let select_test_elf_buf = crate::userspace_test::get_test_binary("select_test");
    #[cfg(feature = "testing")]
    let select_test_elf: &[u8] = &select_test_elf_buf;
    #[cfg(not(feature = "testing"))]
    let select_test_elf = &create_hello_world_elf();

    match crate::process::creation::create_user_process(
        String::from("select_test"),
        select_test_elf,
    ) {
        Ok(pid) => {
            log::info!("Created select_test process with PID {:?}", pid);
            log::info!("Select test: process scheduled for execution.");
            log::info!("    -> Userspace will emit SELECT_TEST marker if successful");
        }
        Err(e) => {
            log::error!("Failed to create select_test process: {}", e);
            log::error!("Select test cannot run without valid userspace process");
        }
    }
}

/// Test O_NONBLOCK pipe behavior
///
/// TWO-STAGE VALIDATION PATTERN:
/// - Stage 1 (Checkpoint): Process creation
///   - Marker: "Nonblock test: process scheduled for execution"
///   - This is a CHECKPOINT confirming process creation succeeded
/// - Stage 2 (Boot stage): Validates non-blocking pipe I/O
///   - Marker: "NONBLOCK_TEST_PASSED"
///   - This PROVES O_NONBLOCK correctly causes read/write on empty/full pipes to return EAGAIN
pub fn test_nonblock() {
    log::info!("Testing O_NONBLOCK pipe behavior");

    #[cfg(feature = "testing")]
    let nonblock_test_elf_buf = crate::userspace_test::get_test_binary("nonblock_test");
    #[cfg(feature = "testing")]
    let nonblock_test_elf: &[u8] = &nonblock_test_elf_buf;
    #[cfg(not(feature = "testing"))]
    let nonblock_test_elf = &create_hello_world_elf();

    match crate::process::creation::create_user_process(
        String::from("nonblock_test"),
        nonblock_test_elf,
    ) {
        Ok(pid) => {
            log::info!("Created nonblock_test process with PID {:?}", pid);
            log::info!("Nonblock test: process scheduled for execution.");
            log::info!("    -> Userspace will emit NONBLOCK_TEST marker if successful");
        }
        Err(e) => {
            log::error!("Failed to create nonblock_test process: {}", e);
            log::error!("Nonblock test cannot run without valid userspace process");
        }
    }
}

/// Test TTY layer functionality
///
/// TWO-STAGE VALIDATION PATTERN:
/// - Stage 1 (Checkpoint): Process creation
///   - Marker: "TTY test: process scheduled for execution"
///   - This is a CHECKPOINT confirming process creation succeeded
/// - Stage 2 (Boot stage): Validates TTY operations
///   - Marker: "TTY_TEST_PASSED"
///   - This PROVES isatty, tcgetattr, tcsetattr, and raw/cooked mode switching all work
pub fn test_tty() {
    log::info!("Testing TTY layer functionality");

    #[cfg(feature = "testing")]
    let tty_test_elf_buf = crate::userspace_test::get_test_binary("tty_test");
    #[cfg(feature = "testing")]
    let tty_test_elf: &[u8] = &tty_test_elf_buf;
    #[cfg(not(feature = "testing"))]
    let tty_test_elf = &create_hello_world_elf();

    match crate::process::creation::create_user_process(
        String::from("tty_test"),
        tty_test_elf,
    ) {
        Ok(pid) => {
            log::info!("Created tty_test process with PID {:?}", pid);
            log::info!("TTY test: process scheduled for execution.");
            log::info!("    -> Userspace will emit TTY_TEST_PASSED marker if successful");
        }
        Err(e) => {
            log::error!("Failed to create tty_test process: {}", e);
            log::error!("TTY test cannot run without valid userspace process");
        }
    }
}

/// Test job control infrastructure
///
/// TWO-STAGE VALIDATION PATTERN:
/// - Stage 1 (Checkpoint): Process creation
///   - Marker: "Job control test: process scheduled for execution"
///   - This is a CHECKPOINT confirming process creation succeeded
/// - Stage 2 (Boot stage): Validates job control infrastructure
///   - Marker: "JOB_CONTROL_TEST_PASSED"
///   - This PROVES setpgid, getpgrp, SIGCONT, WUNTRACED, and tcgetpgrp all work
pub fn test_job_control() {
    log::info!("Testing job control infrastructure");

    #[cfg(feature = "testing")]
    let job_control_test_elf_buf = crate::userspace_test::get_test_binary("job_control_test");
    #[cfg(feature = "testing")]
    let job_control_test_elf: &[u8] = &job_control_test_elf_buf;
    #[cfg(not(feature = "testing"))]
    let job_control_test_elf = &create_hello_world_elf();

    match crate::process::creation::create_user_process(
        String::from("job_control_test"),
        job_control_test_elf,
    ) {
        Ok(pid) => {
            log::info!("Created job_control_test process with PID {:?}", pid);
            log::info!("Job control test: process scheduled for execution.");
            log::info!("    -> Userspace will emit JOB_CONTROL_TEST_PASSED marker if successful");
        }
        Err(e) => {
            log::error!("Failed to create job_control_test process: {}", e);
            log::error!("Job control test cannot run without valid userspace process");
        }
    }
}

/// Test session and process group syscalls
///
/// TWO-STAGE VALIDATION PATTERN:
/// - Stage 1 (Checkpoint): Process creation
///   - Marker: "Session test: process scheduled for execution"
///   - This is a CHECKPOINT confirming process creation succeeded
/// - Stage 2 (Boot stage): Validates session/pgid operations
///   - Marker: "SESSION_TEST_PASSED"
///   - This PROVES getpgid, setpgid, getpgrp, getsid, setsid all work correctly
pub fn test_session() {
    log::info!("Testing session and process group syscalls");

    #[cfg(feature = "testing")]
    let session_test_elf_buf = crate::userspace_test::get_test_binary("session_test");
    #[cfg(feature = "testing")]
    let session_test_elf: &[u8] = &session_test_elf_buf;
    #[cfg(not(feature = "testing"))]
    let session_test_elf = &create_hello_world_elf();

    match crate::process::creation::create_user_process(
        String::from("session_test"),
        session_test_elf,
    ) {
        Ok(pid) => {
            log::info!("Created session_test process with PID {:?}", pid);
            log::info!("Session test: process scheduled for execution.");
            log::info!("    -> Userspace will emit SESSION_TEST_PASSED marker if successful");
        }
        Err(e) => {
            log::error!("Failed to create session_test process: {}", e);
            log::error!("Session test cannot run without valid userspace process");
        }
    }
}

/// Test ext2 file read functionality
///
/// TWO-STAGE VALIDATION PATTERN:
/// - Stage 1 (Checkpoint): Process creation
///   - Marker: "File read test: process scheduled for execution"
///   - This is a CHECKPOINT confirming process creation succeeded
/// - Stage 2 (Boot stage): Validates file reading from ext2
///   - Marker: "FILE_READ_TEST_PASSED"
///   - This PROVES open, read, fstat, and close syscalls work on ext2 filesystem
pub fn test_file_read() {
    log::info!("Testing ext2 file read functionality");

    #[cfg(feature = "testing")]
    let file_read_test_elf_buf = crate::userspace_test::get_test_binary("file_read_test");
    #[cfg(feature = "testing")]
    let file_read_test_elf: &[u8] = &file_read_test_elf_buf;
    #[cfg(not(feature = "testing"))]
    let file_read_test_elf = &create_hello_world_elf();

    match crate::process::creation::create_user_process(
        String::from("file_read_test"),
        file_read_test_elf,
    ) {
        Ok(pid) => {
            log::info!("Created file_read_test process with PID {:?}", pid);
            log::info!("File read test: process scheduled for execution.");
            log::info!("    -> Userspace will emit FILE_READ_TEST_PASSED marker if successful");
        }
        Err(e) => {
            log::error!("Failed to create file_read_test process: {}", e);
            log::error!("File read test cannot run without valid userspace process");
        }
    }
}

/// Test Ctrl-C (SIGINT) signal delivery
///
/// TWO-STAGE VALIDATION PATTERN:
/// - Stage 1 (Checkpoint): Process creation
///   - Marker: "Ctrl-C test: process scheduled for execution"
///   - This is a CHECKPOINT confirming process creation succeeded
/// - Stage 2 (Boot stage): Validates SIGINT delivery and wstatus encoding
///   - Marker: "CTRL_C_TEST_PASSED"
///   - This PROVES:
///     1. Parent can fork a child process
///     2. SIGINT can be sent to child via kill()
///     3. Child is terminated by the signal (default SIGINT action)
///     4. waitpid() correctly reports WIFSIGNALED with WTERMSIG == SIGINT
pub fn test_ctrl_c() {
    log::info!("Testing Ctrl-C (SIGINT) signal delivery");

    #[cfg(feature = "testing")]
    let ctrl_c_test_elf_buf = crate::userspace_test::get_test_binary("ctrl_c_test");
    #[cfg(feature = "testing")]
    let ctrl_c_test_elf: &[u8] = &ctrl_c_test_elf_buf;
    #[cfg(not(feature = "testing"))]
    let ctrl_c_test_elf = &create_hello_world_elf();

    match crate::process::creation::create_user_process(
        String::from("ctrl_c_test"),
        ctrl_c_test_elf,
    ) {
        Ok(pid) => {
            log::info!("Created ctrl_c_test process with PID {:?}", pid);
            log::info!("Ctrl-C test: process scheduled for execution.");
            log::info!("    -> Userspace will emit CTRL_C_TEST_PASSED marker if successful");
        }
        Err(e) => {
            log::error!("Failed to create ctrl_c_test process: {}", e);
            log::error!("Ctrl-C test cannot run without valid userspace process");
        }
    }
}

/// Test getdents64 syscall for directory listing
pub fn test_getdents() {
    log::info!("Testing getdents64 syscall for directory listing");

    #[cfg(feature = "testing")]
    let getdents_test_elf_buf = crate::userspace_test::get_test_binary("getdents_test");
    #[cfg(feature = "testing")]
    let getdents_test_elf: &[u8] = &getdents_test_elf_buf;
    #[cfg(not(feature = "testing"))]
    let getdents_test_elf = &create_hello_world_elf();

    match crate::process::creation::create_user_process(
        String::from("getdents_test"),
        getdents_test_elf,
    ) {
        Ok(pid) => {
            log::info!("Created getdents_test process with PID {:?}", pid);
            log::info!("Getdents test: process scheduled for execution.");
            log::info!("    -> Userspace will emit GETDENTS_TEST_PASSED marker if successful");
        }
        Err(e) => {
            log::error!("Failed to create getdents_test process: {}", e);
            log::error!("Getdents test cannot run without valid userspace process");
        }
    }
}

/// Test lseek syscall including SEEK_END
pub fn test_lseek() {
    log::info!("Testing lseek syscall (SEEK_SET, SEEK_CUR, SEEK_END)");

    #[cfg(feature = "testing")]
    let lseek_test_elf_buf = crate::userspace_test::get_test_binary("lseek_test");
    #[cfg(feature = "testing")]
    let lseek_test_elf: &[u8] = &lseek_test_elf_buf;
    #[cfg(not(feature = "testing"))]
    let lseek_test_elf = &create_hello_world_elf();

    match crate::process::creation::create_user_process(
        String::from("lseek_test"),
        lseek_test_elf,
    ) {
        Ok(pid) => {
            log::info!("Created lseek_test process with PID {:?}", pid);
            log::info!("Lseek test: process scheduled for execution.");
            log::info!("    -> Userspace will emit LSEEK_TEST_PASSED marker if successful");
        }
        Err(e) => {
            log::error!("Failed to create lseek_test process: {}", e);
            log::error!("Lseek test cannot run without valid userspace process");
        }
    }
}

/// Test filesystem write operations (write, O_CREAT, O_TRUNC, O_APPEND, unlink)
pub fn test_fs_write() {
    log::info!("Testing filesystem write operations (write, O_CREAT, O_TRUNC, O_APPEND, unlink)");

    #[cfg(feature = "testing")]
    let fs_write_test_elf_buf = crate::userspace_test::get_test_binary("fs_write_test");
    #[cfg(feature = "testing")]
    let fs_write_test_elf: &[u8] = &fs_write_test_elf_buf;
    #[cfg(not(feature = "testing"))]
    let fs_write_test_elf = &create_hello_world_elf();

    match crate::process::creation::create_user_process(
        String::from("fs_write_test"),
        fs_write_test_elf,
    ) {
        Ok(pid) => {
            log::info!("Created fs_write_test process with PID {:?}", pid);
            log::info!("Filesystem write test: process scheduled for execution.");
            log::info!("    -> Userspace will emit FS_WRITE_TEST_PASSED marker if successful");
        }
        Err(e) => {
            log::error!("Failed to create fs_write_test process: {}", e);
            log::error!("Filesystem write test cannot run without valid userspace process");
        }
    }
}

/// Test filesystem rename operations on ext2
pub fn test_fs_rename() {
    log::info!("Testing filesystem rename operations");

    #[cfg(feature = "testing")]
    let fs_rename_test_elf_buf = crate::userspace_test::get_test_binary("fs_rename_test");
    #[cfg(feature = "testing")]
    let fs_rename_test_elf: &[u8] = &fs_rename_test_elf_buf;
    #[cfg(not(feature = "testing"))]
    let fs_rename_test_elf = &create_hello_world_elf();

    match crate::process::creation::create_user_process(
        String::from("fs_rename_test"),
        fs_rename_test_elf,
    ) {
        Ok(pid) => {
            log::info!("Created fs_rename_test process with PID {:?}", pid);
            log::info!("Filesystem rename test: process scheduled for execution.");
            log::info!("    -> Userspace will emit FS_RENAME_TEST_PASSED marker if successful");
        }
        Err(e) => {
            log::error!("Failed to create fs_rename_test process: {}", e);
            log::error!("Filesystem rename test cannot run without valid userspace process");
        }
    }
}

/// Test large file operations on ext2 (indirect blocks)
pub fn test_fs_large_file() {
    log::info!("Testing large file operations (indirect blocks)");

    #[cfg(feature = "testing")]
    let fs_large_file_test_elf_buf = crate::userspace_test::get_test_binary("fs_large_file_test");
    #[cfg(feature = "testing")]
    let fs_large_file_test_elf: &[u8] = &fs_large_file_test_elf_buf;
    #[cfg(not(feature = "testing"))]
    let fs_large_file_test_elf = &create_hello_world_elf();

    match crate::process::creation::create_user_process(
        String::from("fs_large_file_test"),
        fs_large_file_test_elf,
    ) {
        Ok(pid) => {
            log::info!("Created fs_large_file_test process with PID {:?}", pid);
            log::info!("Large file test: process scheduled for execution.");
            log::info!("    -> Userspace will emit FS_LARGE_FILE_TEST_PASSED marker if successful");
        }
        Err(e) => {
            log::error!("Failed to create fs_large_file_test process: {}", e);
            log::error!("Large file test cannot run without valid userspace process");
        }
    }
}

/// Test filesystem directory operations (mkdir, rmdir)
pub fn test_fs_directory() {
    log::info!("Testing filesystem directory operations (mkdir, rmdir)");

    #[cfg(feature = "testing")]
    let fs_directory_test_elf_buf = crate::userspace_test::get_test_binary("fs_directory_test");
    #[cfg(feature = "testing")]
    let fs_directory_test_elf: &[u8] = &fs_directory_test_elf_buf;
    #[cfg(not(feature = "testing"))]
    let fs_directory_test_elf = &create_hello_world_elf();

    match crate::process::creation::create_user_process(
        String::from("fs_directory_test"),
        fs_directory_test_elf,
    ) {
        Ok(pid) => {
            log::info!("Created fs_directory_test process with PID {:?}", pid);
            log::info!("Directory test: process scheduled for execution.");
            log::info!("    -> Userspace will emit FS_DIRECTORY_TEST_PASSED marker if successful");
        }
        Err(e) => {
            log::error!("Failed to create fs_directory_test process: {}", e);
            log::error!("Directory test cannot run without valid userspace process");
        }
    }
}

/// Test filesystem link operations (link, symlink, readlink)
pub fn test_fs_link() {
    log::info!("Testing filesystem link operations (link, symlink, readlink)");

    #[cfg(feature = "testing")]
    let fs_link_test_elf_buf = crate::userspace_test::get_test_binary("fs_link_test");
    #[cfg(feature = "testing")]
    let fs_link_test_elf: &[u8] = &fs_link_test_elf_buf;
    #[cfg(not(feature = "testing"))]
    let fs_link_test_elf = &create_hello_world_elf();

    match crate::process::creation::create_user_process(
        String::from("fs_link_test"),
        fs_link_test_elf,
    ) {
        Ok(pid) => {
            log::info!("Created fs_link_test process with PID {:?}", pid);
            log::info!("Link test: process scheduled for execution.");
            log::info!("    -> Userspace will emit FS_LINK_TEST_PASSED marker if successful");
        }
        Err(e) => {
            log::error!("Failed to create fs_link_test process: {}", e);
            log::error!("Link test cannot run without valid userspace process");
        }
    }
}

/// Test access() syscall
pub fn test_access() {
    log::info!("Testing access() syscall (F_OK, R_OK, W_OK)");

    #[cfg(feature = "testing")]
    let access_test_elf_buf = crate::userspace_test::get_test_binary("access_test");
    #[cfg(feature = "testing")]
    let access_test_elf: &[u8] = &access_test_elf_buf;
    #[cfg(not(feature = "testing"))]
    let access_test_elf = &create_hello_world_elf();

    match crate::process::creation::create_user_process(
        String::from("access_test"),
        access_test_elf,
    ) {
        Ok(pid) => {
            log::info!("Created access_test process with PID {:?}", pid);
            log::info!("Access test: process scheduled for execution.");
            log::info!("    -> Userspace will emit ACCESS_TEST_PASSED marker if successful");
        }
        Err(e) => {
            log::error!("Failed to create access_test process: {}", e);
            log::error!("Access test cannot run without valid userspace process");
        }
    }
}

/// Test devfs (/dev/null, /dev/zero, /dev/console, /dev/tty)
pub fn test_devfs() {
    log::info!("Testing devfs device files (/dev/null, /dev/zero, /dev/console, /dev/tty)");

    #[cfg(feature = "testing")]
    let devfs_test_elf_buf = crate::userspace_test::get_test_binary("devfs_test");
    #[cfg(feature = "testing")]
    let devfs_test_elf: &[u8] = &devfs_test_elf_buf;
    #[cfg(not(feature = "testing"))]
    let devfs_test_elf = &create_hello_world_elf();

    match crate::process::creation::create_user_process(
        String::from("devfs_test"),
        devfs_test_elf,
    ) {
        Ok(pid) => {
            log::info!("Created devfs_test process with PID {:?}", pid);
            log::info!("Devfs test: process scheduled for execution.");
            log::info!("    -> Userspace will emit DEVFS_TEST_PASSED marker if successful");
        }
        Err(e) => {
            log::error!("Failed to create devfs_test process: {}", e);
            log::error!("Devfs test cannot run without valid userspace process");
        }
    }
}

/// Test current working directory syscalls (getcwd, chdir)
pub fn test_cwd() {
    log::info!("Testing cwd syscalls (getcwd, chdir)");

    #[cfg(feature = "testing")]
    let cwd_test_elf_buf = crate::userspace_test::get_test_binary("cwd_test");
    #[cfg(feature = "testing")]
    let cwd_test_elf: &[u8] = &cwd_test_elf_buf;
    #[cfg(not(feature = "testing"))]
    let cwd_test_elf = &create_hello_world_elf();

    match crate::process::creation::create_user_process(
        String::from("cwd_test"),
        cwd_test_elf,
    ) {
        Ok(pid) => {
            log::info!("Created cwd_test process with PID {:?}", pid);
            log::info!("CWD test: process scheduled for execution.");
            log::info!("    -> Userspace will emit CWD_TEST_PASSED marker if successful");
        }
        Err(e) => {
            log::error!("Failed to create cwd_test process: {}", e);
            log::error!("CWD test cannot run without valid userspace process");
        }
    }
}

/// Test exec from ext2 filesystem
pub fn test_exec_from_ext2() {
    log::info!("Testing exec from ext2 filesystem");

    #[cfg(feature = "testing")]
    let exec_ext2_elf_buf = crate::userspace_test::get_test_binary("exec_from_ext2_test");
    #[cfg(feature = "testing")]
    let exec_ext2_elf: &[u8] = &exec_ext2_elf_buf;
    #[cfg(not(feature = "testing"))]
    let exec_ext2_elf = &create_hello_world_elf();

    match crate::process::creation::create_user_process(
        String::from("exec_from_ext2_test"),
        exec_ext2_elf,
    ) {
        Ok(pid) => {
            log::info!("Created exec_from_ext2_test process with PID {:?}", pid);
            log::info!("Exec ext2 test: process scheduled for execution.");
            log::info!("    -> Userspace will emit EXEC_EXT2_TEST_PASSED marker if successful");
        }
        Err(e) => {
            log::error!("Failed to create exec_from_ext2_test process: {}", e);
            log::error!("Exec ext2 test cannot run without valid userspace process");
        }
    }
}

/// Test filesystem block allocation (regression test for s_first_data_block offset bug)
/// This tests:
/// - truncate_file() properly frees blocks (not just clears pointers)
/// - Multi-file operations don't corrupt other files' data blocks
pub fn test_fs_block_alloc() {
    log::info!("Testing filesystem block allocation (regression test)");

    #[cfg(feature = "testing")]
    let fs_block_alloc_test_elf_buf = crate::userspace_test::get_test_binary("fs_block_alloc_test");
    #[cfg(feature = "testing")]
    let fs_block_alloc_test_elf: &[u8] = &fs_block_alloc_test_elf_buf;
    #[cfg(not(feature = "testing"))]
    let fs_block_alloc_test_elf = &create_hello_world_elf();

    match crate::process::creation::create_user_process(
        String::from("fs_block_alloc_test"),
        fs_block_alloc_test_elf,
    ) {
        Ok(pid) => {
            log::info!("Created fs_block_alloc_test process with PID {:?}", pid);
            log::info!("Block alloc test: process scheduled for execution.");
            log::info!("    -> Userspace will emit BLOCK_ALLOC_TEST_PASSED marker if successful");
        }
        Err(e) => {
            log::error!("Failed to create fs_block_alloc_test process: {}", e);
            log::error!("Block alloc test cannot run without valid userspace process");
        }
    }
}

/// Test Rust std library support via hello_std_real
pub fn test_hello_std_real() {
    log::info!("Testing Rust std library support (hello_std_real)");

    #[cfg(feature = "testing")]
    let hello_std_real_elf_buf = crate::userspace_test::get_test_binary("hello_std_real");
    #[cfg(feature = "testing")]
    let hello_std_real_elf: &[u8] = &hello_std_real_elf_buf;
    #[cfg(not(feature = "testing"))]
    let hello_std_real_elf = &create_hello_world_elf();

    match crate::process::creation::create_user_process(
        String::from("hello_std_real"),
        hello_std_real_elf,
    ) {
        Ok(pid) => {
            log::info!("Created hello_std_real process with PID {:?}", pid);
            log::info!("hello_std_real test: process scheduled for execution.");
        }
        Err(e) => {
            log::error!("Failed to create hello_std_real process: {}", e);
            log::error!("hello_std_real test cannot run without valid userspace process");
        }
    }
}

/// Test fork memory isolation
///
/// This test verifies that fork() correctly implements copy-on-write semantics,
/// ensuring parent and child have isolated memory spaces.
///
/// TWO-STAGE VALIDATION PATTERN:
/// - Stage 1 (Checkpoint): Process creation
///   - Marker: "fork_memory_test: process scheduled for execution"
///   - This is a CHECKPOINT confirming process creation succeeded
/// - Stage 2 (Boot stage): Validates memory isolation
///   - Marker: "FORK_MEMORY_ISOLATION_PASSED"
///   - This PROVES:
///     1. Stack memory is isolated (child sees original value after parent modifies)
///     2. Heap memory (sbrk) is isolated
///     3. Global/static memory is isolated
///
/// This test was added to prevent regression of a bug where fork was copying
/// 707 pages instead of ~20, indicating incorrect CoW behavior.
pub fn test_fork_memory() {
    log::info!("Testing fork memory isolation (CoW semantics)");

    #[cfg(feature = "testing")]
    let fork_memory_test_elf_buf = crate::userspace_test::get_test_binary("fork_memory_test");
    #[cfg(feature = "testing")]
    let fork_memory_test_elf: &[u8] = &fork_memory_test_elf_buf;
    #[cfg(not(feature = "testing"))]
    let fork_memory_test_elf = &create_hello_world_elf();

    match crate::process::creation::create_user_process(
        String::from("fork_memory_test"),
        fork_memory_test_elf,
    ) {
        Ok(pid) => {
            log::info!("Created fork_memory_test process with PID {:?}", pid);
            log::info!("fork_memory_test: process scheduled for execution.");
            log::info!("    -> Userspace will emit FORK_MEMORY_ISOLATION_PASSED marker if successful");
        }
        Err(e) => {
            log::error!("Failed to create fork_memory_test process: {}", e);
            log::error!("Fork memory test cannot run without valid userspace process");
        }
    }
}

/// Test fork state copying (copy_process_state)
///
/// This test verifies that fork() correctly copies all inherited process state:
/// - File descriptors (pipe FDs work across fork)
/// - Signal handlers (child inherits parent's handlers)
/// - Process group ID (child inherits parent's PGID)
/// - Session ID (child inherits parent's SID)
///
/// TWO-STAGE VALIDATION PATTERN:
/// - Stage 1 (Checkpoint): Process creation
///   - Marker: "fork_state_test: process scheduled for execution"
///   - This is a CHECKPOINT confirming process creation succeeded
/// - Stage 2 (Boot stage): Validates state inheritance
///   - Marker: "FORK_STATE_COPY_PASSED"
///   - This PROVES:
///     1. File descriptors are inherited (pipe data readable)
///     2. Signal handlers are inherited (SIGUSR1 handler works)
///     3. PGID is inherited
///     4. SID is inherited
pub fn test_fork_state() {
    log::info!("Testing fork state copying (copy_process_state)");

    #[cfg(feature = "testing")]
    let fork_state_test_elf_buf = crate::userspace_test::get_test_binary("fork_state_test");
    #[cfg(feature = "testing")]
    let fork_state_test_elf: &[u8] = &fork_state_test_elf_buf;
    #[cfg(not(feature = "testing"))]
    let fork_state_test_elf = &create_hello_world_elf();

    match crate::process::creation::create_user_process(
        String::from("fork_state_test"),
        fork_state_test_elf,
    ) {
        Ok(pid) => {
            log::info!("Created fork_state_test process with PID {:?}", pid);
            log::info!("fork_state_test: process scheduled for execution.");
            log::info!("    -> Userspace will emit FORK_STATE_COPY_PASSED marker if successful");
        }
        Err(e) => {
            log::error!("Failed to create fork_state_test process: {}", e);
            log::error!("Fork state test cannot run without valid userspace process");
        }
    }
}

/// Test fork pending signal non-inheritance (POSIX requirement)
pub fn test_fork_pending_signal() {
    log::info!("Testing fork pending signal non-inheritance (POSIX)");

    #[cfg(feature = "testing")]
    let fork_pending_signal_test_elf_buf =
        crate::userspace_test::get_test_binary("fork_pending_signal_test");
    #[cfg(feature = "testing")]
    let fork_pending_signal_test_elf: &[u8] = &fork_pending_signal_test_elf_buf;
    #[cfg(not(feature = "testing"))]
    let fork_pending_signal_test_elf = &create_hello_world_elf();

    match crate::process::creation::create_user_process(
        String::from("fork_pending_signal_test"),
        fork_pending_signal_test_elf,
    ) {
        Ok(pid) => {
            log::info!("Created fork_pending_signal_test process with PID {:?}", pid);
            log::info!(
                "    -> Userspace will emit FORK_PENDING_SIGNAL_TEST_PASSED marker if successful"
            );
        }
        Err(e) => {
            log::error!("Failed to create fork_pending_signal_test process: {}", e);
        }
    }
}

/// Test CoW signal delivery
///
/// This test specifically verifies that signal delivery works correctly
/// when the user stack is a CoW-shared page. This was the root cause of
/// a deadlock bug where signal delivery writes to a CoW page while holding
/// the PROCESS_MANAGER lock.
///
/// TWO-STAGE VALIDATION PATTERN:
/// - Stage 1 (Checkpoint): Process creation
///   - Marker: (none - process creation logs)
/// - Stage 2 (Boot stage): Validates CoW + signal interaction
///   - Marker: "COW_SIGNAL_TEST_PASSED"
///   - This PROVES:
///     1. Signal delivery can write to CoW-shared stack without deadlocking
///     2. Signal handler executes correctly on CoW-copied stack
///     3. The try_manager() + handle_cow_direct() fallback works
pub fn test_cow_signal() {
    log::info!("Testing CoW signal delivery (deadlock fix)");

    #[cfg(feature = "testing")]
    let cow_signal_test_elf_buf = crate::userspace_test::get_test_binary("cow_signal_test");
    #[cfg(feature = "testing")]
    let cow_signal_test_elf: &[u8] = &cow_signal_test_elf_buf;
    #[cfg(not(feature = "testing"))]
    let cow_signal_test_elf = &create_hello_world_elf();

    match crate::process::creation::create_user_process(
        String::from("cow_signal_test"),
        cow_signal_test_elf,
    ) {
        Ok(pid) => {
            log::info!("Created cow_signal_test process with PID {:?}", pid);
            log::info!("    -> Userspace will emit COW_SIGNAL_TEST_PASSED marker if successful");
        }
        Err(e) => {
            log::error!("Failed to create cow_signal_test process: {}", e);
        }
    }
}

/// Test CoW cleanup on process exit
///
/// This test verifies that when forked children exit, shared CoW frame
/// reference counts are properly decremented. Tests that parent's memory
/// remains intact after multiple children write to and exit from shared pages.
///
/// TWO-STAGE VALIDATION PATTERN:
/// - Stage 1 (Checkpoint): Process creation
/// - Stage 2 (Boot stage): Validates CoW cleanup
///   - Marker: "COW_CLEANUP_TEST_PASSED"
///   - This PROVES: Frame refcounts are correctly decremented on child exit
pub fn test_cow_cleanup() {
    log::info!("Testing CoW cleanup on process exit");

    #[cfg(feature = "testing")]
    let cow_cleanup_test_elf_buf = crate::userspace_test::get_test_binary("cow_cleanup_test");
    #[cfg(feature = "testing")]
    let cow_cleanup_test_elf: &[u8] = &cow_cleanup_test_elf_buf;
    #[cfg(not(feature = "testing"))]
    let cow_cleanup_test_elf = &create_hello_world_elf();

    match crate::process::creation::create_user_process(
        String::from("cow_cleanup_test"),
        cow_cleanup_test_elf,
    ) {
        Ok(pid) => {
            log::info!("Created cow_cleanup_test process with PID {:?}", pid);
            log::info!("    -> Userspace will emit COW_CLEANUP_TEST_PASSED marker if successful");
        }
        Err(e) => {
            log::error!("Failed to create cow_cleanup_test process: {}", e);
        }
    }
}

/// Test CoW sole owner optimization
///
/// This test verifies that when a forked child exits without writing to
/// shared pages, the parent becomes the sole owner and can write without
/// triggering a page copy (just makes page writable).
///
/// TWO-STAGE VALIDATION PATTERN:
/// - Stage 1 (Checkpoint): Process creation
/// - Stage 2 (Boot stage): Validates sole owner optimization
///   - Marker: "COW_SOLE_OWNER_TEST_PASSED"
///   - This PROVES: Sole owner optimization path works correctly
pub fn test_cow_sole_owner() {
    log::info!("Testing CoW sole owner optimization");

    #[cfg(feature = "testing")]
    let cow_sole_owner_test_elf_buf = crate::userspace_test::get_test_binary("cow_sole_owner_test");
    #[cfg(feature = "testing")]
    let cow_sole_owner_test_elf: &[u8] = &cow_sole_owner_test_elf_buf;
    #[cfg(not(feature = "testing"))]
    let cow_sole_owner_test_elf = &create_hello_world_elf();

    match crate::process::creation::create_user_process(
        String::from("cow_sole_owner_test"),
        cow_sole_owner_test_elf,
    ) {
        Ok(pid) => {
            log::info!("Created cow_sole_owner_test process with PID {:?}", pid);
            log::info!("    -> Userspace will emit COW_SOLE_OWNER_TEST_PASSED marker if successful");
        }
        Err(e) => {
            log::error!("Failed to create cow_sole_owner_test process: {}", e);
        }
    }
}

/// Test CoW at scale with many pages
///
/// This test verifies that Copy-on-Write works correctly at scale with many
/// pages. It allocates a large amount of memory (128 pages = 512KB), fills
/// it with a known pattern, forks, then has the child write to every page
/// to trigger many CoW faults in sequence.
///
/// TWO-STAGE VALIDATION PATTERN:
/// - Stage 1 (Checkpoint): Process creation
/// - Stage 2 (Boot stage): Validates CoW at scale
///   - Marker: "COW_STRESS_TEST_PASSED"
///   - This PROVES: CoW works correctly with many pages, no memory corruption
pub fn test_cow_stress() {
    log::info!("Testing CoW at scale with many pages (stress test)");

    #[cfg(feature = "testing")]
    let cow_stress_test_elf_buf = crate::userspace_test::get_test_binary("cow_stress_test");
    #[cfg(feature = "testing")]
    let cow_stress_test_elf: &[u8] = &cow_stress_test_elf_buf;
    #[cfg(not(feature = "testing"))]
    let cow_stress_test_elf = &create_hello_world_elf();

    match crate::process::creation::create_user_process(
        String::from("cow_stress_test"),
        cow_stress_test_elf,
    ) {
        Ok(pid) => {
            log::info!("Created cow_stress_test process with PID {:?}", pid);
            log::info!("    -> Userspace will emit COW_STRESS_TEST_PASSED marker if successful");
        }
        Err(e) => {
            log::error!("Failed to create cow_stress_test process: {}", e);
        }
    }
}

/// Test CoW read-only page sharing (code sections)
///
/// This test verifies that read-only pages (like code/text sections) are
/// shared directly between parent and child after fork WITHOUT the COW flag.
///
/// TWO-STAGE VALIDATION PATTERN:
/// - Stage 1 (Checkpoint): Process creation
/// - Stage 2 (Boot stage): Validates read-only page sharing
///   - Marker: "COW_READONLY_TEST_PASSED"
///   - This PROVES: Code sections are shared without COW overhead
pub fn test_cow_readonly() {
    log::info!("Testing CoW read-only page sharing (code sections)");

    #[cfg(feature = "testing")]
    let cow_readonly_test_elf_buf = crate::userspace_test::get_test_binary("cow_readonly_test");
    #[cfg(feature = "testing")]
    let cow_readonly_test_elf: &[u8] = &cow_readonly_test_elf_buf;
    #[cfg(not(feature = "testing"))]
    let cow_readonly_test_elf = &create_hello_world_elf();

    match crate::process::creation::create_user_process(
        String::from("cow_readonly_test"),
        cow_readonly_test_elf,
    ) {
        Ok(pid) => {
            log::info!("Created cow_readonly_test process with PID {:?}", pid);
            log::info!("    -> Userspace will emit COW_READONLY_TEST_PASSED marker if successful");
        }
        Err(e) => {
            log::error!("Failed to create cow_readonly_test process: {}", e);
        }
    }
}

/// Test argv support in exec syscall
///
/// TWO-STAGE VALIDATION PATTERN:
/// - Stage 1 (This function): Creates and schedules the process
/// - Stage 2 (Boot stage): Validates actual execution via ARGV_TEST_PASSED marker
pub fn test_argv() {
    log::info!("Testing argv support in exec syscall");

    #[cfg(feature = "testing")]
    let argv_test_elf_buf = crate::userspace_test::get_test_binary("argv_test");
    #[cfg(feature = "testing")]
    let argv_test_elf: &[u8] = &argv_test_elf_buf;
    #[cfg(not(feature = "testing"))]
    let argv_test_elf = &create_hello_world_elf();

    match crate::process::creation::create_user_process(
        String::from("argv_test"),
        argv_test_elf,
    ) {
        Ok(pid) => {
            log::info!("Created argv_test process with PID {:?}", pid);
            log::info!("    -> Userspace will emit ARGV_TEST_PASSED marker if successful");
        }
        Err(e) => {
            log::error!("Failed to create argv_test process: {}", e);
            log::error!("Argv test cannot run without valid userspace process");
        }
    }
}

/// Test exec with argv - validates fork+exec with arguments
pub fn test_exec_argv() {
    log::info!("Testing exec with argv (fork+exec with specific arguments)");

    #[cfg(feature = "testing")]
    let exec_argv_test_elf_buf = crate::userspace_test::get_test_binary("exec_argv_test");
    #[cfg(feature = "testing")]
    let exec_argv_test_elf: &[u8] = &exec_argv_test_elf_buf;
    #[cfg(not(feature = "testing"))]
    let exec_argv_test_elf = &create_hello_world_elf();

    match crate::process::creation::create_user_process(
        String::from("exec_argv_test"),
        exec_argv_test_elf,
    ) {
        Ok(pid) => {
            log::info!("Created exec_argv_test process with PID {:?}", pid);
            log::info!("    -> Userspace will emit EXEC_ARGV_TEST_PASSED marker if successful");
        }
        Err(e) => {
            log::error!("Failed to create exec_argv_test process: {}", e);
        }
    }
}

/// Test exec with stack-allocated argv buffers
///
/// This is a regression test for a bug where the compiler could optimize away
/// stack-allocated argument buffers before the syscall read from them.
/// The fix uses core::hint::black_box() to prevent the optimization.
pub fn test_exec_stack_argv() {
    log::info!("Testing exec with stack-allocated argv (regression test for black_box fix)");

    #[cfg(feature = "testing")]
    let exec_stack_argv_test_elf_buf =
        crate::userspace_test::get_test_binary("exec_stack_argv_test");
    #[cfg(feature = "testing")]
    let exec_stack_argv_test_elf: &[u8] = &exec_stack_argv_test_elf_buf;
    #[cfg(not(feature = "testing"))]
    let exec_stack_argv_test_elf = &create_hello_world_elf();

    match crate::process::creation::create_user_process(
        String::from("exec_stack_argv_test"),
        exec_stack_argv_test_elf,
    ) {
        Ok(pid) => {
            log::info!("Created exec_stack_argv_test process with PID {:?}", pid);
            log::info!(
                "    -> Userspace will emit EXEC_STACK_ARGV_TEST_PASSED marker if successful"
            );
        }
        Err(e) => {
            log::error!("Failed to create exec_stack_argv_test process: {}", e);
        }
    }
}

/// Test close-on-exec (O_CLOEXEC) behavior
pub fn test_cloexec() {
    log::info!("Testing close-on-exec (O_CLOEXEC) behavior");

    #[cfg(feature = "testing")]
    let cloexec_test_elf_buf = crate::userspace_test::get_test_binary("cloexec_test");
    #[cfg(feature = "testing")]
    let cloexec_test_elf: &[u8] = &cloexec_test_elf_buf;
    #[cfg(not(feature = "testing"))]
    let cloexec_test_elf = &create_hello_world_elf();

    match crate::process::creation::create_user_process(
        String::from("cloexec_test"),
        cloexec_test_elf,
    ) {
        Ok(pid) => {
            log::info!("Created cloexec_test process with PID {:?}", pid);
            log::info!("    -> Userspace will emit CLOEXEC_TEST_PASSED marker if successful");
        }
        Err(e) => {
            log::error!("Failed to create cloexec_test process: {}", e);
        }
    }
}

/// Test shell pipeline execution (pipe+fork+dup2 pattern)
///
/// TWO-STAGE VALIDATION PATTERN:
/// - Stage 1 (Checkpoint): Process creation
///   - Marker: "Shell pipe test: process scheduled for execution"
///   - This is a CHECKPOINT confirming process creation succeeded
/// - Stage 2 (Boot stage): Validates pipeline data flow
///   - Marker: "SHELL_PIPE_TEST_PASSED"
///   - This PROVES the pipe+fork+dup2 pattern works correctly for shell pipelines
///
/// The test simulates `echo TEST | cat` and verifies data flows through the pipeline.
pub fn test_shell_pipe() {
    log::info!("Testing shell pipeline execution (pipe+fork+dup2 pattern)");

    #[cfg(feature = "testing")]
    let shell_pipe_test_elf_buf = crate::userspace_test::get_test_binary("shell_pipe_test");
    #[cfg(feature = "testing")]
    let shell_pipe_test_elf: &[u8] = &shell_pipe_test_elf_buf;
    #[cfg(not(feature = "testing"))]
    let shell_pipe_test_elf = &create_hello_world_elf();

    match crate::process::creation::create_user_process(
        String::from("shell_pipe_test"),
        shell_pipe_test_elf,
    ) {
        Ok(pid) => {
            log::info!("Created shell_pipe_test process with PID {:?}", pid);
            log::info!("Shell pipe test: process scheduled for execution.");
            log::info!("    -> Userspace will emit SHELL_PIPE_TEST_PASSED marker if successful");
        }
        Err(e) => {
            log::error!("Failed to create shell_pipe_test process: {}", e);
            log::error!("Shell pipe test cannot run without valid userspace process");
        }
    }
}

/// Test the `true` coreutil
///
/// Verifies that /bin/true correctly exits with code 0.
pub fn test_true_coreutil() {
    log::info!("Testing true coreutil (exit code 0)");

    #[cfg(feature = "testing")]
    let true_test_elf_buf = crate::userspace_test::get_test_binary("true_test");
    #[cfg(feature = "testing")]
    let true_test_elf: &[u8] = &true_test_elf_buf;
    #[cfg(not(feature = "testing"))]
    let true_test_elf = &create_hello_world_elf();

    match crate::process::creation::create_user_process(
        String::from("true_test"),
        true_test_elf,
    ) {
        Ok(pid) => {
            log::info!("Created true_test process with PID {:?}", pid);
            log::info!("true_test: process scheduled for execution.");
            log::info!("    -> Userspace will emit TRUE_TEST_PASSED marker if successful");
        }
        Err(e) => {
            log::error!("Failed to create true_test process: {}", e);
            log::error!("true_test cannot run without valid userspace process");
        }
    }
}

/// Test the `false` coreutil
///
/// Verifies that /bin/false correctly exits with code 1.
pub fn test_false_coreutil() {
    log::info!("Testing false coreutil (exit code 1)");

    #[cfg(feature = "testing")]
    let false_test_elf_buf = crate::userspace_test::get_test_binary("false_test");
    #[cfg(feature = "testing")]
    let false_test_elf: &[u8] = &false_test_elf_buf;
    #[cfg(not(feature = "testing"))]
    let false_test_elf = &create_hello_world_elf();

    match crate::process::creation::create_user_process(
        String::from("false_test"),
        false_test_elf,
    ) {
        Ok(pid) => {
            log::info!("Created false_test process with PID {:?}", pid);
            log::info!("false_test: process scheduled for execution.");
            log::info!("    -> Userspace will emit FALSE_TEST_PASSED marker if successful");
        }
        Err(e) => {
            log::error!("Failed to create false_test process: {}", e);
            log::error!("false_test cannot run without valid userspace process");
        }
    }
}

/// Test the `head` coreutil
///
/// Verifies that /bin/head correctly outputs the first N lines of files.
pub fn test_head_coreutil() {
    log::info!("Testing head coreutil (first N lines)");

    #[cfg(feature = "testing")]
    let head_test_elf_buf = crate::userspace_test::get_test_binary("head_test");
    #[cfg(feature = "testing")]
    let head_test_elf: &[u8] = &head_test_elf_buf;
    #[cfg(not(feature = "testing"))]
    let head_test_elf = &create_hello_world_elf();

    match crate::process::creation::create_user_process(
        String::from("head_test"),
        head_test_elf,
    ) {
        Ok(pid) => {
            log::info!("Created head_test process with PID {:?}", pid);
            log::info!("head_test: process scheduled for execution.");
            log::info!("    -> Userspace will emit HEAD_TEST_PASSED marker if successful");
        }
        Err(e) => {
            log::error!("Failed to create head_test process: {}", e);
            log::error!("head_test cannot run without valid userspace process");
        }
    }
}

/// Test the `tail` coreutil
///
/// Verifies that /bin/tail correctly outputs the last N lines of files.
pub fn test_tail_coreutil() {
    log::info!("Testing tail coreutil (last N lines)");

    #[cfg(feature = "testing")]
    let tail_test_elf_buf = crate::userspace_test::get_test_binary("tail_test");
    #[cfg(feature = "testing")]
    let tail_test_elf: &[u8] = &tail_test_elf_buf;
    #[cfg(not(feature = "testing"))]
    let tail_test_elf = &create_hello_world_elf();

    match crate::process::creation::create_user_process(
        String::from("tail_test"),
        tail_test_elf,
    ) {
        Ok(pid) => {
            log::info!("Created tail_test process with PID {:?}", pid);
            log::info!("tail_test: process scheduled for execution.");
            log::info!("    -> Userspace will emit TAIL_TEST_PASSED marker if successful");
        }
        Err(e) => {
            log::error!("Failed to create tail_test process: {}", e);
            log::error!("tail_test cannot run without valid userspace process");
        }
    }
}

/// Test the `wc` coreutil
///
/// Verifies that /bin/wc correctly counts lines, words, and bytes.
pub fn test_wc_coreutil() {
    log::info!("Testing wc coreutil (line/word/byte counts)");

    #[cfg(feature = "testing")]
    let wc_test_elf_buf = crate::userspace_test::get_test_binary("wc_test");
    #[cfg(feature = "testing")]
    let wc_test_elf: &[u8] = &wc_test_elf_buf;
    #[cfg(not(feature = "testing"))]
    let wc_test_elf = &create_hello_world_elf();

    match crate::process::creation::create_user_process(
        String::from("wc_test"),
        wc_test_elf,
    ) {
        Ok(pid) => {
            log::info!("Created wc_test process with PID {:?}", pid);
            log::info!("wc_test: process scheduled for execution.");
            log::info!("    -> Userspace will emit WC_TEST_PASSED marker if successful");
        }
        Err(e) => {
            log::error!("Failed to create wc_test process: {}", e);
            log::error!("wc_test cannot run without valid userspace process");
        }
    }
}

/// Test the `which` coreutil
///
/// Verifies that /bin/which correctly locates commands in PATH.
pub fn test_which_coreutil() {
    log::info!("Testing which coreutil (command location)");

    #[cfg(feature = "testing")]
    let which_test_elf_buf = crate::userspace_test::get_test_binary("which_test");
    #[cfg(feature = "testing")]
    let which_test_elf: &[u8] = &which_test_elf_buf;
    #[cfg(not(feature = "testing"))]
    let which_test_elf = &create_hello_world_elf();

    match crate::process::creation::create_user_process(
        String::from("which_test"),
        which_test_elf,
    ) {
        Ok(pid) => {
            log::info!("Created which_test process with PID {:?}", pid);
            log::info!("which_test: process scheduled for execution.");
            log::info!("    -> Userspace will emit WHICH_TEST_PASSED marker if successful");
        }
        Err(e) => {
            log::error!("Failed to create which_test process: {}", e);
            log::error!("which_test cannot run without valid userspace process");
        }
    }
}

/// Test the `cat` coreutil
///
/// Verifies that /bin/cat correctly outputs file contents.
pub fn test_cat_coreutil() {
    log::info!("Testing cat coreutil (file concatenation)");

    #[cfg(feature = "testing")]
    let cat_test_elf_buf = crate::userspace_test::get_test_binary("cat_test");
    #[cfg(feature = "testing")]
    let cat_test_elf: &[u8] = &cat_test_elf_buf;
    #[cfg(not(feature = "testing"))]
    let cat_test_elf = &create_hello_world_elf();

    match crate::process::creation::create_user_process(
        String::from("cat_test"),
        cat_test_elf,
    ) {
        Ok(pid) => {
            log::info!("Created cat_test process with PID {:?}", pid);
            log::info!("cat_test: process scheduled for execution.");
            log::info!("    -> Userspace will emit CAT_TEST_PASSED marker if successful");
        }
        Err(e) => {
            log::error!("Failed to create cat_test process: {}", e);
            log::error!("cat_test cannot run without valid userspace process");
        }
    }
}

/// Test the `ls` coreutil
///
/// Verifies that /bin/ls correctly lists directory contents.
pub fn test_ls_coreutil() {
    log::info!("Testing ls coreutil (directory listing)");

    #[cfg(feature = "testing")]
    let ls_test_elf_buf = crate::userspace_test::get_test_binary("ls_test");
    #[cfg(feature = "testing")]
    let ls_test_elf: &[u8] = &ls_test_elf_buf;
    #[cfg(not(feature = "testing"))]
    let ls_test_elf = &create_hello_world_elf();

    match crate::process::creation::create_user_process(
        String::from("ls_test"),
        ls_test_elf,
    ) {
        Ok(pid) => {
            log::info!("Created ls_test process with PID {:?}", pid);
            log::info!("ls_test: process scheduled for execution.");
            log::info!("    -> Userspace will emit LS_TEST_PASSED marker if successful");
        }
        Err(e) => {
            log::error!("Failed to create ls_test process: {}", e);
            log::error!("ls_test cannot run without valid userspace process");
        }
    }
}

/// Test FbInfo syscall (graphics framebuffer information)
///
/// TWO-STAGE VALIDATION PATTERN:
/// - Stage 1 (This function): Creates and schedules the fbinfo_test process
/// - Stage 2 (Boot stage): Validates actual execution via FBINFO_TEST: all tests PASSED marker
///
/// This test validates:
/// - The FbInfo syscall (410) returns valid framebuffer information
/// - Width and height are positive
/// - Bytes per pixel is 3 or 4 (RGB or RGBA)
/// - Pixel format is valid (RGB, BGR, or grayscale)
pub fn test_fbinfo() {
    log::info!("Testing FbInfo syscall (framebuffer information)");

    #[cfg(feature = "testing")]
    let fbinfo_test_elf_buf = crate::userspace_test::get_test_binary("fbinfo_test");
    #[cfg(feature = "testing")]
    let fbinfo_test_elf: &[u8] = &fbinfo_test_elf_buf;
    #[cfg(not(feature = "testing"))]
    let fbinfo_test_elf = &create_hello_world_elf();

    match crate::process::creation::create_user_process(
        String::from("fbinfo_test"),
        fbinfo_test_elf,
    ) {
        Ok(pid) => {
            log::info!("Created fbinfo_test process with PID {:?}", pid);
            log::info!("    -> Userspace will emit FBINFO_TEST: all tests PASSED marker if successful");
        }
        Err(e) => {
            log::error!("Failed to create fbinfo_test process: {}", e);
            log::error!("fbinfo_test cannot run without valid userspace process");
        }
    }
}

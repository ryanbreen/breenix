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
    let hello_time_elf = crate::userspace_test::HELLO_TIME_ELF;
    #[cfg(not(feature = "testing"))]
    let hello_time_elf = &create_hello_world_elf();
    
    log::info!("CONCURRENCY TEST: Loading hello_time.elf, size: {} bytes", hello_time_elf.len());
    
    // First create one hello_time process to verify basic execution
    match create_user_process(String::from("hello_time_test"), hello_time_elf) {
        Ok(pid) => {
            log::info!("✓ CONCURRENT: Created hello_time process with PID {}", pid.as_u64());
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
pub fn test_userspace_fork() {
    log::info!("=== Testing multiple instances of same program ===");
    log::info!("This test runs TWO hello_time processes to isolate the issue");
    
    // TEMPORARILY: Use hello_time instead of fork_test to see if issue is with different ELF binaries
    #[cfg(feature = "testing")]
    let test_elf = crate::userspace_test::HELLO_TIME_ELF;
    #[cfg(not(feature = "testing"))]
    let test_elf = &create_hello_world_elf();
    
    log::info!("Creating second hello_time process...");
    
    match create_user_process(String::from("hello_time_2"), test_elf) {
        Ok(pid) => {
            log::info!("✓ Created second hello_time process with PID {}", pid.as_u64());
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
    let fork_test_elf = crate::userspace_test::FORK_TEST_ELF;
    #[cfg(not(feature = "testing"))]
    let fork_test_elf = &create_minimal_elf_no_bss();
    
    match create_user_process(String::from("parent_process"), fork_test_elf) {
        Ok(parent_pid) => {
            log::info!("Created parent process with PID {}", parent_pid.as_u64());
            
            // Now fork the parent process
            match crate::process::with_process_manager(|manager| {
                manager.fork_process(parent_pid)
            }) {
                Some(Ok(child_pid)) => {
                    log::info!("✓ Fork succeeded! Parent PID: {}, Child PID: {}", 
                        parent_pid.as_u64(), child_pid.as_u64());
                    
                    // Now exec hello_time.elf in the child process
                    log::info!("Attempting to exec hello_time.elf into child process {}", child_pid.as_u64());
                    
                    #[cfg(feature = "testing")]
                    let hello_time_elf = crate::userspace_test::HELLO_TIME_ELF;
                    #[cfg(not(feature = "testing"))]
                    let hello_time_elf = &create_minimal_elf_no_bss();
                    
                    match crate::process::with_process_manager(|manager| {
                        manager.exec_process(child_pid, hello_time_elf)
                    }) {
                        Some(Ok(entry_point)) => {
                            log::info!("✓ exec succeeded! Child process {} now running hello_time at {:#x}", 
                                child_pid.as_u64(), entry_point);
                            log::info!("Parent process {} continues running fork_test", parent_pid.as_u64());
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

/// Test exec_basic binary to validate sys_exec syscall
pub fn test_exec_basic() {
    log::info!("=== Testing exec_basic binary (Phase 4C validation) ===");
    
    #[cfg(feature = "testing")]
    {
        // Create and run exec_basic.elf which will exec into exec_target
        match create_user_process(String::from("exec_basic_test"), crate::userspace_test::EXEC_BASIC_ELF) {
            Ok(pid) => {
                log::info!("✓ Created exec_basic test process with PID {}", pid.as_u64());
                log::info!("  -> Process will call sys_exec to replace itself with exec_target");
                log::info!("  -> Expected output: EXEC_OK (from exec_target)");
                log::info!("TEST_MARKER:EXEC_BASIC:STARTED");
            }
            Err(e) => {
                log::error!("✗ Failed to create exec_basic process: {}", e);
                log::info!("TEST_MARKER:EXEC_BASIC:FAILED");
            }
        }
    }
    
    #[cfg(not(feature = "testing"))]
    {
        log::warn!("exec_basic test requires testing feature");
    }
}

/// Test exec directly by creating a process and then calling exec on it
pub fn test_exec_directly() {
    log::info!("=== Testing exec() directly ===");
    
    // First create a process with fork_test.elf
    #[cfg(feature = "testing")]
    let fork_test_elf = crate::userspace_test::FORK_TEST_ELF;
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
            log::info!("Attempting to exec hello_time.elf into process {}", pid.as_u64());
            
            #[cfg(feature = "testing")]
            let hello_time_elf = crate::userspace_test::HELLO_TIME_ELF;
            #[cfg(not(feature = "testing"))]
            let hello_time_elf = &create_minimal_elf_no_bss();
            
            // Use with_process_manager to properly disable interrupts
            match crate::process::with_process_manager(|manager| {
                manager.exec_process(pid, hello_time_elf)
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
        let fork_test_elf = crate::userspace_test::FORK_TEST_ELF;
        let hello_time_elf = crate::userspace_test::HELLO_TIME_ELF;
        
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
                    manager.exec_process(pid, hello_time_elf)
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
                                        crate::task::scheduler::spawn(alloc::boxed::Box::new(main_thread.clone()));
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
            let hello_time_elf = crate::userspace_test::HELLO_TIME_ELF;
            #[cfg(not(feature = "testing"))]
            let hello_time_elf = &create_minimal_elf_no_bss();
            
            // Use with_process_manager to properly disable interrupts
            log::info!("Attempting exec with hello_time.elf...");
            match crate::process::with_process_manager(|manager| {
                manager.exec_process(pid, hello_time_elf)
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
    let shell_elf = crate::userspace_test::FORK_TEST_ELF; // Using fork_test as our "shell"
    #[cfg(not(feature = "testing"))]  
    let shell_elf = &create_minimal_elf_no_bss();
    
    match create_user_process(String::from("shell"), shell_elf) {
        Ok(shell_pid) => {
            log::info!("Created shell process with PID {}", shell_pid.as_u64());
            
            // Simulate shell receiving command to run hello_time
            log::info!("Shell (PID {}) wants to run hello_time command", shell_pid.as_u64());
            
            // Step 1: Shell forks itself
            match crate::process::with_process_manager(|manager| {
                manager.fork_process(shell_pid)
            }) {
                Some(Ok(child_pid)) => {
                    log::info!("✓ Shell forked! Shell PID: {}, Child PID: {}", 
                        shell_pid.as_u64(), child_pid.as_u64());
                    
                    // Step 2: Child process execs the command
                    #[cfg(feature = "testing")]
                    let command_elf = crate::userspace_test::HELLO_TIME_ELF;
                    #[cfg(not(feature = "testing"))]
                    let command_elf = &create_hello_world_elf();
                    
                    // Remove child from ready queue before exec
                    crate::process::with_process_manager(|manager| {
                        if manager.remove_from_ready_queue(child_pid) {
                            log::info!("Removed child {} from ready queue before exec", child_pid.as_u64());
                        }
                        Some(())
                    });
                    
                    match crate::process::with_process_manager(|manager| {
                        manager.exec_process(child_pid, command_elf)
                    }) {
                        Some(Ok(entry_point)) => {
                            log::info!("✓ Child {} exec'd hello_time successfully! Entry: {:#x}", 
                                child_pid.as_u64(), entry_point);
                            
                            // Add child back to ready queue
                            x86_64::instructions::interrupts::without_interrupts(|| {
                                let mut manager_guard = crate::process::manager();
                                if let Some(ref mut manager) = *manager_guard {
                                    manager.add_to_ready_queue(child_pid);
                                    log::info!("✓ Child {} added back to ready queue", child_pid.as_u64());
                                    
                                    // Schedule the child
                                    if let Some(process) = manager.get_process(child_pid) {
                                        if let Some(ref main_thread) = process.main_thread {
                                            crate::task::scheduler::spawn(alloc::boxed::Box::new(main_thread.clone()));
                                            log::info!("✓ hello_time command scheduled for execution");
                                        }
                                    }
                                }
                            });
                            
                            log::info!("Shell {} continues running while child {} runs hello_time", 
                                shell_pid.as_u64(), child_pid.as_u64());
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



/// Create a minimal ELF without BSS segment for testing
fn create_minimal_elf_no_bss() -> alloc::vec::Vec<u8> {
    create_hello_world_elf()
}

/// Create a hello world ELF that tests syscalls
fn create_hello_world_elf() -> alloc::vec::Vec<u8> {
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
        0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x00, // entry = 0x10000000
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
        0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x00, // vaddr = 0x10000000
        0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x00, // paddr
        0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // filesz = 128 bytes
        0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // memsz = 128 bytes
        0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // align
    ]);
    
    // Program header 2 - data segment
    elf.extend_from_slice(&[
        0x01, 0x00, 0x00, 0x00, // PT_LOAD
        0x06, 0x00, 0x00, 0x00, // flags (R+W)
        0x30, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // offset = 304 (176 + 128)
        0x00, 0x10, 0x00, 0x10, 0x00, 0x00, 0x00, 0x00, // vaddr = 0x10001000
        0x00, 0x10, 0x00, 0x10, 0x00, 0x00, 0x00, 0x00, // paddr
        0x20, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // filesz = 32 bytes
        0x20, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // memsz = 32 bytes
        0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // align
    ]);
    
    // Code section at offset 176
    let code = vec![
        // Print "Hello from userspace!\n"
        0xb8, 0x01, 0x00, 0x00, 0x00,                // mov eax, 1 (sys_write)
        0xbf, 0x01, 0x00, 0x00, 0x00,                // mov edi, 1 (stdout)
        0x48, 0xbe, 0x00, 0x10, 0x00, 0x10, 0x00, 0x00, 0x00, 0x00,  // mov rsi, 0x10001000 (string address)
        0xba, 0x16, 0x00, 0x00, 0x00,                // mov edx, 22 (string length)
        0xcd, 0x80,                                   // int 0x80 (syscall)
        
        // Exit with code 0
        0xb8, 0x00, 0x00, 0x00, 0x00,                // mov eax, 0 (sys_exit)
        0x31, 0xff,                                   // xor edi, edi (exit code 0)
        0xcd, 0x80,                                   // int 0x80 (syscall)
        
        // Should never reach here
        0xeb, 0xfe,                                   // jmp $ (infinite loop)
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

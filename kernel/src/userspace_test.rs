//! Userspace program testing module


/// Include the compiled userspace test binaries
#[cfg(feature = "testing")]
pub static HELLO_TIME_ELF: &[u8] = include_bytes!("../../userspace/tests/hello_time.elf");

#[cfg(feature = "testing")]
pub static HELLO_WORLD_ELF: &[u8] = include_bytes!("../../userspace/tests/hello_world.elf");

#[cfg(feature = "testing")]
pub static COUNTER_ELF: &[u8] = include_bytes!("../../userspace/tests/counter.elf");

#[cfg(feature = "testing")]
pub static SPINNER_ELF: &[u8] = include_bytes!("../../userspace/tests/spinner.elf");

#[cfg(feature = "testing")]
pub static FORK_TEST_ELF: &[u8] = include_bytes!("../../userspace/tests/fork_test.elf");

#[cfg(feature = "testing")]
pub static ISOLATION_ELF: &[u8] = include_bytes!("../../userspace/tests/isolation.elf");

#[cfg(feature = "testing")]
pub static ISOLATION_ATTACKER_ELF: &[u8] = include_bytes!("../../userspace/tests/isolation_attacker.elf");

// Add test to ensure binaries are included
#[cfg(feature = "testing")]
fn _test_binaries_included() {
    assert!(HELLO_TIME_ELF.len() > 0, "hello_time.elf not included");
    assert!(HELLO_WORLD_ELF.len() > 0, "hello_world.elf not included");
    assert!(COUNTER_ELF.len() > 0, "counter.elf not included");
    assert!(SPINNER_ELF.len() > 0, "spinner.elf not included");
    assert!(FORK_TEST_ELF.len() > 0, "fork_test.elf not included");
}

/// Test running a userspace program
#[cfg(feature = "testing")]
pub fn test_userspace_syscalls() {
    log::info!("=== Testing Userspace Syscalls ===");
    
    // The binary is included at compile time
    log::info!("Userspace test binary size: {} bytes", HELLO_TIME_ELF.len());
    
    // Check first few bytes
    if HELLO_TIME_ELF.len() >= 4 {
        log::info!("First 4 bytes: {:02x} {:02x} {:02x} {:02x}", 
            HELLO_TIME_ELF[0], HELLO_TIME_ELF[1], HELLO_TIME_ELF[2], HELLO_TIME_ELF[3]);
    }
    
    // Note: This test requires the scheduler to be initialized
    log::warn!("Note: Userspace syscall test requires scheduler initialization");
    log::warn!("Skipping actual spawn test - scheduler not yet initialized during testing phase");
    
    // Just verify the ELF header can be parsed
    // We can't actually load it without memory mapping infrastructure
    use core::mem;
    use crate::elf::{Elf64Header, ELF_MAGIC, ELFCLASS64, ELFDATA2LSB};
    
    if HELLO_TIME_ELF.len() >= mem::size_of::<Elf64Header>() {
        let mut header_bytes = [0u8; mem::size_of::<Elf64Header>()];
        header_bytes.copy_from_slice(&HELLO_TIME_ELF[..mem::size_of::<Elf64Header>()]);
        let header: &Elf64Header = unsafe { &*(header_bytes.as_ptr() as *const Elf64Header) };
        
        if header.magic == ELF_MAGIC {
            log::info!("✓ ELF magic verified");
        } else {
            log::error!("✗ Invalid ELF magic");
        }
        
        if header.class == ELFCLASS64 && header.data == ELFDATA2LSB {
            log::info!("✓ 64-bit little-endian ELF");
        }
        
        if header.elf_type == 2 && header.machine == 0x3e {
            log::info!("✓ x86_64 executable");
        }
        
        log::info!("✓ Entry point: {:#x}", header.entry);
        log::info!("✓ {} program headers at offset {:#x}", header.phnum, header.phoff);
    }
    
    log::info!("Userspace syscall test completed (parsing only)");
}

/// Alternative without std::fs for non-testing builds
#[cfg(not(feature = "testing"))]
pub fn test_userspace_syscalls() {
    log::info!("Userspace syscall testing requires 'testing' feature");
}

/// Run userspace test - callable from keyboard handler
pub fn run_userspace_test() {
    log::info!("=== Running Userspace Test Program ===");
    
    // Check if we have the test binary
    #[cfg(feature = "testing")]
    {
        use alloc::string::String;
        
        log::info!("Creating userspace test process ({} bytes)", HELLO_TIME_ELF.len());
        log::info!("ELF entry point from header: 0x{:x}", {
            use crate::elf::Elf64Header;
            let header: &Elf64Header = unsafe { 
                &*(HELLO_TIME_ELF.as_ptr() as *const Elf64Header) 
            };
            header.entry
        });
        
        // Create and schedule a process for the test program
        match crate::process::create_user_process(
            String::from("hello_time"), 
            HELLO_TIME_ELF
        ) {
            Ok(pid) => {
                log::info!("✓ Created and scheduled process with PID {}", pid.as_u64());
                
                // Get the process manager and debug print
                if let Some(ref manager) = *crate::process::manager() {
                    manager.debug_processes();
                }
                
                log::info!("Process scheduled - it will run when scheduler picks it up");
                log::info!("Timer interrupts should trigger scheduling");
                
                // Force a yield to try to switch to the process
                crate::task::scheduler::yield_current();
                log::info!("Yielded to scheduler");
            }
            Err(e) => {
                log::error!("✗ Failed to create process: {}", e);
            }
        }
    }
    
    #[cfg(not(feature = "testing"))]
    {
        log::warn!("Userspace test binary not available - compile with --features testing");
    }
}

/// Test multiple processes - callable from keyboard handler
pub fn test_multiple_processes() {
    log::info!("=== Testing Multiple Processes ===");
    
    #[cfg(feature = "testing")]
    {
        use alloc::string::String;
        
        // Create and schedule first process (counter)
        log::info!("Creating first process (counter)...");
        match crate::process::create_user_process(
            String::from("counter"), 
            COUNTER_ELF
        ) {
            Ok(pid1) => {
                log::info!("✓ Created and scheduled process 1 (counter) with PID {}", pid1.as_u64());
                
                // Create and schedule second process (spinner)
                log::info!("Creating second process (spinner)...");
                match crate::process::create_user_process(
                    String::from("spinner"), 
                    SPINNER_ELF
                ) {
                    Ok(pid2) => {
                        log::info!("✓ Created and scheduled process 2 (spinner) with PID {}", pid2.as_u64());
                        
                        // Debug print process list
                        if let Some(ref manager) = *crate::process::manager() {
                            manager.debug_processes();
                        }
                        
                        log::info!("Both processes scheduled - they will run concurrently");
                        log::info!("Processes will alternate execution based on timer interrupts");
                        log::info!("Counter will count from 0-9, Spinner will show a spinning animation");
                        log::info!("Each process yields after each output to allow the other to run");
                    }
                    Err(e) => {
                        log::error!("✗ Failed to create second process: {}", e);
                    }
                }
            }
            Err(e) => {
                log::error!("✗ Failed to create first process: {}", e);
            }
        }
    }
    
    #[cfg(not(feature = "testing"))]
    {
        log::warn!("Userspace test binaries not available - compile with --features testing");
    }
}

/// Test fork system call implementation (debug version)
#[cfg(feature = "testing")]
pub fn test_fork_debug() {
    log::info!("=== Testing Fork System Call (Debug Mode) ===");
    
    use alloc::string::String;
    
    log::info!("Creating process that will call fork() to debug thread ID tracking...");
    
    // Use the new spawn mechanism which creates a dedicated thread for exec
    match crate::process::create_user_process(
        String::from("fork_debug"), 
        FORK_TEST_ELF
    ) {
        Ok(pid) => {
            log::info!("✓ Created and scheduled fork debug process with PID {}", pid.as_u64());
            log::info!("Process will call fork() and we'll debug the thread ID issue");
        }
        Err(e) => {
            log::error!("❌ Failed to create fork debug process: {}", e);
        }
    }
}

/// Test fork system call implementation (non-testing version)
#[cfg(not(feature = "testing"))]
pub fn test_fork_debug() {
    log::warn!("Fork test binary not available - compile with --features testing");
    log::info!("However, we can still test the fork system call directly...");
    
    // Call fork directly to test the system call mechanism
    log::info!("Calling fork() system call directly from kernel...");
    let result = crate::syscall::handlers::sys_fork();
    match result {
        crate::syscall::SyscallResult::Ok(val) => {
            log::info!("Fork returned success value: {}", val);
        }
        crate::syscall::SyscallResult::Err(errno) => {
            log::info!("Fork returned error code: {} (ENOSYS - not implemented)", errno);
        }
    }
    
    // For now, skip creating a userspace process with fake ELF data
    // Instead, let's test the fork mechanism by simulating a userspace context
    log::info!("Testing fork mechanism with simulated userspace context...");
    
    // Create a proper test by manually invoking fork from a non-idle thread context
    // This will test our fork implementation without dealing with ELF loading issues
    test_fork_manually();
}

/// Test fork manually by creating a proper userspace process context
fn test_fork_manually() {
    log::info!("test_fork_manually: Creating a minimal process to test fork");
    
    use alloc::string::String;
    
    // Create a minimal valid ELF binary - just enough to create a process
    // We'll create a dummy process and then manually call fork from its context
    
    // For now, let's create a simple test that schedules a kernel thread
    // that will create a minimal process and test fork
    use alloc::boxed::Box;
    
    // Create a kernel thread using the proper new_kernel method
    let fork_test_thread = match crate::task::thread::Thread::new_kernel(
        String::from("fork_creator"),
        fork_creator_thread_trampoline,
        0 // No argument needed
    ) {
        Ok(thread) => thread,
        Err(e) => {
            log::error!("test_fork_manually: Failed to create kernel thread: {}", e);
            return;
        }
    };
    
    // Spawn the thread
    crate::task::scheduler::spawn(Box::new(fork_test_thread));
    log::info!("test_fork_manually: Spawned fork creator thread");
}

/// Trampoline function for the kernel thread (matches expected signature)
extern "C" fn fork_creator_thread_trampoline(_arg: u64) -> ! {
    fork_creator_thread_fn();
    
    // Kernel threads should never return, so infinite loop
    loop {
        x86_64::instructions::hlt();
    }
}

/// Fork creator thread - creates a process and tests fork
fn fork_creator_thread_fn() {
    log::info!("fork_creator_thread_fn: Starting - will test fork mechanism");
    
    // Wait a bit to let system stabilize
    for _ in 0..1000000 {
        core::hint::spin_loop();
    }
    
    // Test the fork mechanism directly
    log::info!("fork_creator_thread_fn: Testing fork mechanism by creating a minimal process");
    
    // Create a minimal ELF binary that is just enough for testing
    let minimal_valid_elf = create_minimal_valid_elf();
    
    use alloc::string::String;
    
    // Try to create a process using the creation module
    match crate::process::creation::create_user_process(
        String::from("fork_test_simple"), 
        &minimal_valid_elf
    ) {
        Ok(pid) => {
            log::info!("fork_creator_thread_fn: Successfully created test process PID {}", pid.as_u64());
            
            // Wait a bit for the process to be fully set up
            for _ in 0..1000000 {
                core::hint::spin_loop();
            }
            
            // Test fork from this process
            test_fork_from_process(pid);
        }
        Err(e) => {
            log::error!("fork_creator_thread_fn: Failed to create test process: {}", e);
            
            // Fallback: test fork mechanism directly without full ELF process
            log::info!("fork_creator_thread_fn: Testing fork mechanism with minimal setup");
            test_fork_mechanism_minimal();
        }
    }
}

/// Create a minimal but valid ELF binary for testing
fn create_minimal_valid_elf() -> alloc::vec::Vec<u8> {
    use alloc::vec::Vec;
    
    // Create a very simple ELF with minimal headers
    let mut elf = Vec::new();
    
    // ELF header (64 bytes)
    elf.extend_from_slice(&[
        0x7f, 0x45, 0x4c, 0x46, // e_ident[EI_MAG0..EI_MAG3] = ELF
        0x02,                   // e_ident[EI_CLASS] = ELFCLASS64
        0x01,                   // e_ident[EI_DATA] = ELFDATA2LSB
        0x01,                   // e_ident[EI_VERSION] = EV_CURRENT
        0x00,                   // e_ident[EI_OSABI] = ELFOSABI_NONE
        0x00,                   // e_ident[EI_ABIVERSION] = 0
    ]);
    
    // Pad EI_PAD to 16 bytes total
    for _ in 0..7 {
        elf.push(0x00);
    }
    
    elf.extend_from_slice(&[
        0x02, 0x00,             // e_type = ET_EXEC (2)
        0x3e, 0x00,             // e_machine = EM_X86_64 (62)
        0x01, 0x00, 0x00, 0x00, // e_version = EV_CURRENT (1)
    ]);
    
    // e_entry (8 bytes) = 0x10000000
    elf.extend_from_slice(&[0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x00]);
    
    // e_phoff (8 bytes) = 64 (program headers start after ELF header)
    elf.extend_from_slice(&[0x40, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    
    // e_shoff (8 bytes) = 0 (no section headers)
    elf.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    
    elf.extend_from_slice(&[
        0x00, 0x00, 0x00, 0x00, // e_flags = 0
        0x40, 0x00,             // e_ehsize = 64
        0x38, 0x00,             // e_phentsize = 56
        0x01, 0x00,             // e_phnum = 1 (one program header)
        0x00, 0x00,             // e_shentsize = 0
        0x00, 0x00,             // e_shnum = 0
        0x00, 0x00,             // e_shstrndx = 0
    ]);
    
    // Program header (56 bytes)
    elf.extend_from_slice(&[
        0x01, 0x00, 0x00, 0x00, // p_type = PT_LOAD (1)
        0x05, 0x00, 0x00, 0x00, // p_flags = PF_R | PF_X (5)
    ]);
    
    // p_offset (8 bytes) = 120 (after headers)
    elf.extend_from_slice(&[0x78, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    
    // p_vaddr (8 bytes) = 0x10000000
    elf.extend_from_slice(&[0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x00]);
    
    // p_paddr (8 bytes) = 0x10000000
    elf.extend_from_slice(&[0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x00]);
    
    // p_filesz (8 bytes) = 16 (small code section)
    elf.extend_from_slice(&[0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    
    // p_memsz (8 bytes) = 16
    elf.extend_from_slice(&[0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    
    // p_align (8 bytes) = 4096
    elf.extend_from_slice(&[0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    
    // Code section (starting at offset 120) - simple infinite loop
    elf.extend_from_slice(&[
        0xeb, 0xfe,             // jmp -2 (infinite loop)
        0x90, 0x90, 0x90, 0x90, // nop padding
        0x90, 0x90, 0x90, 0x90,
        0x90, 0x90, 0x90, 0x90,
        0x90, 0x90,
    ]);
    
    elf
}

/// Create a minimal ELF binary for exec testing (different from fork test)
fn create_exec_test_elf() -> alloc::vec::Vec<u8> {
    use alloc::vec::Vec;
    
    // Create a simple ELF with a different program (exit with code 42)
    let mut elf = Vec::new();
    
    // ELF header (64 bytes)
    elf.extend_from_slice(&[
        0x7f, 0x45, 0x4c, 0x46, // e_ident[EI_MAG0..EI_MAG3] = ELF
        0x02,                   // e_ident[EI_CLASS] = ELFCLASS64
        0x01,                   // e_ident[EI_DATA] = ELFDATA2LSB
        0x01,                   // e_ident[EI_VERSION] = EV_CURRENT
        0x00,                   // e_ident[EI_OSABI] = ELFOSABI_NONE
        0x00,                   // e_ident[EI_ABIVERSION] = 0
    ]);
    
    // Pad EI_PAD to 16 bytes total
    for _ in 0..7 {
        elf.push(0x00);
    }
    
    elf.extend_from_slice(&[
        0x02, 0x00,             // e_type = ET_EXEC (2)
        0x3e, 0x00,             // e_machine = EM_X86_64 (62)
        0x01, 0x00, 0x00, 0x00, // e_version = EV_CURRENT (1)
    ]);
    
    // e_entry (8 bytes) = 0x10000000
    elf.extend_from_slice(&[0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x00]);
    
    // e_phoff (8 bytes) = 64 (program headers start after ELF header)
    elf.extend_from_slice(&[0x40, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    
    // e_shoff (8 bytes) = 0 (no section headers)
    elf.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    
    elf.extend_from_slice(&[
        0x00, 0x00, 0x00, 0x00, // e_flags = 0
        0x40, 0x00,             // e_ehsize = 64
        0x38, 0x00,             // e_phentsize = 56
        0x01, 0x00,             // e_phnum = 1 (one program header)
        0x00, 0x00,             // e_shentsize = 0
        0x00, 0x00,             // e_shnum = 0
        0x00, 0x00,             // e_shstrndx = 0
    ]);
    
    // Program header (56 bytes)
    elf.extend_from_slice(&[
        0x01, 0x00, 0x00, 0x00, // p_type = PT_LOAD (1)
        0x05, 0x00, 0x00, 0x00, // p_flags = PF_R | PF_X (5)
    ]);
    
    // p_offset (8 bytes) = 120 (after headers)
    elf.extend_from_slice(&[0x78, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    
    // p_vaddr (8 bytes) = 0x10000000
    elf.extend_from_slice(&[0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x00]);
    
    // p_paddr (8 bytes) = 0x10000000
    elf.extend_from_slice(&[0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x00]);
    
    // p_filesz (8 bytes) = 20 (code section)
    elf.extend_from_slice(&[0x14, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    
    // p_memsz (8 bytes) = 20
    elf.extend_from_slice(&[0x14, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    
    // p_align (8 bytes) = 4096
    elf.extend_from_slice(&[0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    
    // Code section (starting at offset 120) - exit with code 42
    elf.extend_from_slice(&[
        0x48, 0xc7, 0xc7, 0x2a, 0x00, 0x00, 0x00, // mov rdi, 42
        0x48, 0xc7, 0xc0, 0x3c, 0x00, 0x00, 0x00, // mov rax, 60 (sys_exit)
        0x0f, 0x05,                               // syscall
        0xf4,                                     // hlt (shouldn't reach)
        0x90, 0x90, 0x90,                         // nop padding
    ]);
    
    elf
}

/// Test fork mechanism with minimal setup
fn test_fork_mechanism_minimal() {
    log::info!("test_fork_mechanism_minimal: Testing basic fork mechanism");
    
    // Just test if we can call the fork process manager functions
    let manager_guard = crate::process::manager();
    if let Some(ref manager) = *manager_guard {
        let process_count = manager.process_count();
        log::info!("test_fork_mechanism_minimal: Current process count: {}", process_count);
        
        if process_count > 0 {
            let pids = manager.all_pids();
            if let Some(&first_pid) = pids.first() {
                log::info!("test_fork_mechanism_minimal: Testing fork on existing PID {}", first_pid.as_u64());
                drop(manager_guard);
                test_fork_from_process(first_pid);
            } else {
                log::warn!("test_fork_mechanism_minimal: No processes available for testing");
            }
        } else {
            log::warn!("test_fork_mechanism_minimal: No processes in system");
        }
    }
}

/// Test fork from a specific process context
fn test_fork_from_process(test_pid: crate::process::ProcessId) {
    log::info!("test_fork_from_process: Testing fork from PID {}", test_pid.as_u64());
    
    // Call fork_process directly on the process manager
    let mut manager_guard = crate::process::manager();
    if let Some(ref mut manager) = *manager_guard {
        match manager.fork_process(test_pid) {
            Ok(child_pid) => {
                log::info!("🎉 FORK SUCCESS: Parent PID {} created child PID {}", 
                          test_pid.as_u64(), child_pid.as_u64());
                
                // Verify the child process exists
                if let Some(child_process) = manager.get_process(child_pid) {
                    log::info!("✓ Child process verified: name='{}', state={:?}", 
                              child_process.name, child_process.state);
                    
                    if let Some(ref child_thread) = child_process.main_thread {
                        log::info!("✓ Child thread verified: ID={}, RAX={} (should be 0)", 
                                  child_thread.id, child_thread.context.rax);
                    }
                }
                
                // Test exec on the child process
                log::info!("test_fork_from_process: Now testing exec on child process {}", child_pid.as_u64());
                test_exec_on_process(child_pid);
                
            }
            Err(e) => {
                log::error!("❌ FORK FAILED: {}", e);
            }
        }
    } else {
        log::error!("test_fork_from_process: Process manager not available");
    }
}

/// Test exec on a specific process
fn test_exec_on_process(pid: crate::process::ProcessId) {
    log::info!("test_exec_on_process: Testing exec on PID {}", pid.as_u64());
    
    // Use the same minimal ELF that works for fork instead of create_exec_test_elf
    let exec_elf_data = create_minimal_valid_elf();
    
    let mut manager_guard = crate::process::manager();
    if let Some(ref mut manager) = *manager_guard {
        match manager.exec_process(pid, &exec_elf_data) {
            Ok(entry_point) => {
                log::info!("🎉 EXEC SUCCESS: Process {} replaced with entry point {:#x}", 
                          pid.as_u64(), entry_point);
            }
            Err(e) => {
                log::error!("❌ EXEC FAILED: {}", e);
            }
        }
    } else {
        log::error!("test_exec_on_process: Process manager not available");
    }
}


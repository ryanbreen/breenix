//! Test exec functionality directly

use crate::process::creation::create_user_process;
use alloc::string::String;

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

/// Test exec without scheduling - creates process without adding to scheduler
pub fn test_exec_without_scheduling() {
    log::info!("=== Testing exec() without immediate scheduling ===");
    
    // Create a process without scheduling it
    #[cfg(feature = "testing")]
    let initial_elf = crate::userspace_test::FORK_TEST_ELF;
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
                    log::info!("Removed process {} from ready queue to prevent early scheduling", pid.as_u64());
                }
                
                Some(pid)
            }
            Err(e) => {
                log::error!("Failed to create process: {}", e);
                None
            }
        }
    }).flatten();
    
    if let Some(pid) = pid {
        // Now exec the actual program we want to run
        #[cfg(feature = "testing")]
        let target_elf = crate::userspace_test::HELLO_TIME_ELF;
        #[cfg(not(feature = "testing"))]
        let target_elf = &create_exec_test_elf();
        
        log::info!("Calling exec to load target program...");
        
        match crate::process::with_process_manager(|manager| {
            manager.exec_process(pid, target_elf)
        }) {
            Some(Ok(entry_point)) => {
                log::info!("✓ exec succeeded! New entry point: {:#x}", entry_point);
                
                // Now add process back to ready queue after exec
                x86_64::instructions::interrupts::without_interrupts(|| {
                    let mut manager_guard = crate::process::manager();
                    if let Some(ref mut manager) = *manager_guard {
                        // Add back to ready queue
                        manager.add_to_ready_queue(pid);
                        log::info!("✓ Process {} added back to ready queue after exec", pid.as_u64());
                        
                        // Also need to spawn the thread
                        if let Some(process) = manager.get_process(pid) {
                            if let Some(ref main_thread) = process.main_thread {
                                crate::task::scheduler::spawn(alloc::boxed::Box::new(main_thread.clone()));
                                log::info!("✓ Process {} thread scheduled with exec'd program", pid.as_u64());
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

/// Create a minimal ELF without BSS segment for testing
fn create_minimal_elf_no_bss() -> alloc::vec::Vec<u8> {
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
        0x01, 0x00, // phnum (1 segment)
        0x00, 0x00, // shentsize
        0x00, 0x00, // shnum
        0x00, 0x00, // shstrndx
    ]);
    
    // Program header - just code segment, no BSS
    elf.extend_from_slice(&[
        0x01, 0x00, 0x00, 0x00, // PT_LOAD
        0x05, 0x00, 0x00, 0x00, // flags (R+X)
        0x78, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // offset
        0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x00, // vaddr = 0x10000000
        0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x00, // paddr
        0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // filesz = 16 bytes
        0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // memsz = filesz (no BSS)
        0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // align
    ]);
    
    // Code: simple infinite loop
    elf.extend_from_slice(&[
        0xb8, 0x00, 0x00, 0x00, 0x00,  // mov eax, 0
        0xeb, 0xfe,                     // jmp $ (infinite loop)
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // padding
    ]);
    
    elf
}

/// Create a minimal ELF binary for exec testing (different from fork test)
fn create_exec_test_elf() -> alloc::vec::Vec<u8> {
    use alloc::vec::Vec;
    
    // Create a simple ELF that just loops with NOPs to test basic execution
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
    
    // p_filesz (8 bytes) = 20 (code section with int3)
    elf.extend_from_slice(&[0x14, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    
    // p_memsz (8 bytes) = 20
    elf.extend_from_slice(&[0x14, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    
    // p_align (8 bytes) = 4096
    elf.extend_from_slice(&[0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    
    // Code section (starting at offset 120) - NOPs then breakpoint for proof of execution
    elf.extend_from_slice(&[
        0x90, 0x90, 0x90, 0x90, 0x90,  // 5 NOPs (0x10000000-0x10000004)
        0xcc,                           // int3 breakpoint (0x10000005) - PROOF OF EXECUTION
        0xeb, 0xfe,                     // jmp $ (infinite loop after breakpoint)
        0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, // 12 bytes padding = 20 total
    ]);
    
    elf
}
//! Userspace program testing module


/// Include the compiled userspace test binaries
#[cfg(feature = "testing")]
pub static HELLO_TIME_ELF: &[u8] = include_bytes!("../../userspace/tests/hello_time.elf");

#[cfg(feature = "testing")]
pub static HELLO_WORLD_ELF: &[u8] = include_bytes!("../../userspace/tests/hello_world.elf");

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
        use x86_64::VirtAddr;
        
        log::info!("Creating userspace test process ({} bytes)", HELLO_TIME_ELF.len());
        
        // Create and schedule a process for the test program
        match crate::task::process_task::ProcessScheduler::create_and_schedule_process(
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
                log::info!("Use timer interrupts or sys_yield to trigger scheduling");
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
        
        // Create and schedule first process
        log::info!("Creating first process (hello_time)...");
        match crate::task::process_task::ProcessScheduler::create_and_schedule_process(
            String::from("hello_time"), 
            HELLO_TIME_ELF
        ) {
            Ok(pid1) => {
                log::info!("✓ Created and scheduled process 1 with PID {}", pid1.as_u64());
                
                // Create and schedule second process
                log::info!("Creating second process (hello_world)...");
                match crate::task::process_task::ProcessScheduler::create_and_schedule_process(
                    String::from("hello_world"), 
                    HELLO_WORLD_ELF
                ) {
                    Ok(pid2) => {
                        log::info!("✓ Created and scheduled process 2 with PID {}", pid2.as_u64());
                        
                        // Debug print process list
                        if let Some(ref manager) = *crate::process::manager() {
                            manager.debug_processes();
                        }
                        
                        log::info!("Both processes scheduled - they will run when scheduler picks them up");
                        log::info!("Processes will alternate execution based on timer interrupts");
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


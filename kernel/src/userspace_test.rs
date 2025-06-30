//! Userspace program testing module

use alloc::boxed::Box;

/// Include the compiled userspace test binary
#[cfg(feature = "testing")]
pub static HELLO_TIME_ELF: &[u8] = include_bytes!("../../userspace/tests/hello_time.elf");

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
        use crate::elf;
        
        log::info!("Loading userspace test program ({} bytes)", HELLO_TIME_ELF.len());
        
        // Just load the ELF without spawning a thread
        match elf::load_elf(HELLO_TIME_ELF) {
            Ok(loaded_elf) => {
                log::info!("✓ Successfully loaded userspace program at {:#x}", loaded_elf.entry_point);
                
                // For now, let's try a direct jump to userspace
                log::info!("Attempting direct execution of userspace code...");
                
                // Allocate a stack for the userspace program
                use crate::memory::stack;
                use crate::task::thread::ThreadPrivilege;
                
                const USER_STACK_SIZE: usize = 64 * 1024;
                let user_stack = stack::allocate_stack_with_privilege(
                    USER_STACK_SIZE,
                    ThreadPrivilege::User
                ).expect("Failed to allocate user stack");
                
                let stack_top = user_stack.top();
                
                // Keep stack alive
                let _stack = Box::leak(Box::new(user_stack));
                
                log::info!("Switching to userspace with proper ring transition...");
                
                unsafe {
                    // Get selectors and ensure Ring 3 RPL bits are set
                    let user_cs = crate::gdt::USER_CODE_SELECTOR.0 | 3;
                    let user_ds = crate::gdt::USER_DATA_SELECTOR.0 | 3;
                    
                    log::debug!("Using selectors - CS: {:#x}, DS/SS: {:#x}", user_cs, user_ds);
                    
                    // This will switch to Ring 3 and never return
                    crate::task::userspace_switch::switch_to_userspace(
                        loaded_elf.entry_point,
                        stack_top,
                        user_cs,
                        user_ds,
                    );
                }
            }
            Err(e) => {
                log::error!("✗ Failed to load userspace program: {}", e);
            }
        }
    }
    
    #[cfg(not(feature = "testing"))]
    {
        log::warn!("Userspace test binary not available - compile with --features testing");
    }
}
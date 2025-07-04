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
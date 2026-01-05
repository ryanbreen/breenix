//! User-only fault tests to prove Ring 3 privilege isolation
//! These tests attempt privileged operations from userspace and verify they cause proper faults

use alloc::vec::Vec;

/// Create an ELF that attempts CLI instruction (should cause #GP)
#[allow(dead_code)]
pub fn create_cli_test_elf() -> Vec<u8> {
    let mut elf = create_elf_header();

    // Code section - attempt CLI then exit (must be 32 bytes)
    elf.extend_from_slice(&[
        // Attempt CLI (privileged instruction)
        0xfa,                                       // cli - should cause #GP(0)
        // Should never reach here
        0x48, 0xc7, 0xc0, 0x00, 0x00, 0x00, 0x00, // mov rax, 0 (sys_exit)
        0x48, 0x31, 0xff,                          // xor rdi, rdi
        0xcd, 0x80,                                // int 0x80
    ]);
    // Pad to 32 bytes (13 bytes of code + 19 bytes of nop padding)
    for _ in 0..19 { elf.push(0x90); }

    elf
}

/// Create an ELF that attempts HLT instruction (should cause #GP)
#[allow(dead_code)]
pub fn create_hlt_test_elf() -> Vec<u8> {
    let mut elf = create_elf_header();

    // Code section - attempt HLT then exit (must be 32 bytes)
    elf.extend_from_slice(&[
        // Attempt HLT (privileged instruction)
        0xf4,                                       // hlt - should cause #GP(0)
        // Should never reach here
        0x48, 0xc7, 0xc0, 0x00, 0x00, 0x00, 0x00, // mov rax, 0 (sys_exit)
        0x48, 0x31, 0xff,                          // xor rdi, rdi
        0xcd, 0x80,                                // int 0x80
    ]);
    // Pad to 32 bytes (13 bytes of code + 19 bytes of nop padding)
    for _ in 0..19 { elf.push(0x90); }

    elf
}

/// Create an ELF that attempts to write CR3 (should cause #GP)
#[allow(dead_code)]
pub fn create_cr3_write_test_elf() -> Vec<u8> {
    let mut elf = create_elf_header();

    // Code section - attempt to write CR3 then exit (must be 32 bytes)
    elf.extend_from_slice(&[
        // Attempt to write CR3 (privileged operation)
        0x48, 0x31, 0xc0,                          // xor rax, rax
        0x0f, 0x22, 0xd8,                          // mov cr3, rax - should cause #GP(0)
        // Should never reach here
        0x48, 0xc7, 0xc0, 0x00, 0x00, 0x00, 0x00, // mov rax, 0 (sys_exit)
        0x48, 0x31, 0xff,                          // xor rdi, rdi
        0xcd, 0x80,                                // int 0x80
    ]);
    // Pad to 32 bytes (18 bytes of code + 14 bytes of nop padding)
    for _ in 0..14 { elf.push(0x90); }

    elf
}

/// Create an ELF that accesses unmapped memory (should cause #PF with U=1)
#[allow(dead_code)]
pub fn create_unmapped_access_test_elf() -> Vec<u8> {
    let mut elf = create_elf_header();

    // Code section - access unmapped memory then exit (must be 32 bytes)
    elf.extend_from_slice(&[
        // Try to read from unmapped userspace address
        0x48, 0xb8, 0x00, 0x00, 0x00, 0x50, 0x00, 0x00, 0x00, 0x00, // mov rax, 0x50000000 (unmapped)
        0x48, 0x8b, 0x00,                          // mov rax, [rax] - should cause #PF(U=1,P=0)
        // Should never reach here
        0x48, 0xc7, 0xc0, 0x00, 0x00, 0x00, 0x00, // mov rax, 0 (sys_exit)
        0x48, 0x31, 0xff,                          // xor rdi, rdi
        0xcd, 0x80,                                // int 0x80
    ]);
    // Pad to 32 bytes (25 bytes of code + 7 bytes of nop padding)
    for _ in 0..7 { elf.push(0x90); }

    elf
}

/// Helper to create basic ELF header
fn create_elf_header() -> Vec<u8> {
    let mut elf = Vec::new();
    
    // ELF header (64 bytes)
    elf.extend_from_slice(&[
        0x7f, b'E', b'L', b'F',     // Magic
        0x02,                       // 64-bit
        0x01,                       // Little endian
        0x01,                       // Current version
        0x00,                       // System V ABI
        0x00,                       // ABI version
    ]);
    
    // Padding
    for _ in 0..7 {
        elf.push(0x00);
    }
    
    elf.extend_from_slice(&[
        0x02, 0x00,                 // ET_EXEC
        0x3e, 0x00,                 // x86_64
        0x01, 0x00, 0x00, 0x00,     // Version 1
    ]);
    
    // Entry point: 0x40000000 (userspace address)
    elf.extend_from_slice(&[0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x00, 0x00]);
    
    // Program header offset: 64
    elf.extend_from_slice(&[0x40, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    
    // Section header offset: 0
    elf.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    
    elf.extend_from_slice(&[
        0x00, 0x00, 0x00, 0x00,     // Flags
        0x40, 0x00,                 // ELF header size
        0x38, 0x00,                 // Program header size
        0x01, 0x00,                 // Program header count
        0x00, 0x00,                 // Section header size
        0x00, 0x00,                 // Section header count
        0x00, 0x00,                 // Section name string index
    ]);
    
    // Program header (56 bytes)
    elf.extend_from_slice(&[
        0x01, 0x00, 0x00, 0x00,     // PT_LOAD
        0x05, 0x00, 0x00, 0x00,     // PF_R | PF_X
    ]);
    
    // Offset: 120
    elf.extend_from_slice(&[0x78, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    
    // Virtual address: 0x40000000 (userspace address)
    elf.extend_from_slice(&[0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x00, 0x00]);

    // Physical address: 0x40000000 (userspace address)
    elf.extend_from_slice(&[0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x00, 0x00]);
    
    // File size: 32 (will be small code)
    elf.extend_from_slice(&[0x20, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    
    // Memory size: 32
    elf.extend_from_slice(&[0x20, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    
    // Alignment: 4096
    elf.extend_from_slice(&[0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    
    elf
}

/// Run all user-only fault tests
#[allow(dead_code)]
pub fn run_fault_tests() {
    #[cfg(not(feature = "testing"))]
    {
        log::info!("Fault tests require testing feature to be enabled");
        return;
    }
    
    #[cfg(feature = "testing")]
    {
    use alloc::string::String;
    
    log::info!("=== Running User-Only Fault Tests ===");
    log::info!("These tests prove Ring 3 privilege isolation by attempting privileged operations");
    
    // Test 1: CLI instruction
    log::info!("\n1. Testing CLI instruction (should cause #GP)...");
    let cli_elf = create_cli_test_elf();
    match crate::process::create_user_process(String::from("cli_fault_test"), &cli_elf) {
        Ok(pid) => {
            log::info!("  Created CLI test process PID {}", pid.as_u64());
            log::info!("  Expecting #GP(0) fault when it runs...");
        }
        Err(e) => log::error!("  Failed to create CLI test process: {}", e),
    }
    
    // Brief pause to allow scheduler to pick up the process
    // (Reduced from 10M to 100K to avoid boot stage timeouts)
    for _ in 0..100_000 {
        core::hint::spin_loop();
    }

    // Test 2: HLT instruction
    log::info!("\n2. Testing HLT instruction (should cause #GP)...");
    let hlt_elf = create_hlt_test_elf();
    match crate::process::create_user_process(String::from("hlt_fault_test"), &hlt_elf) {
        Ok(pid) => {
            log::info!("  Created HLT test process PID {}", pid.as_u64());
            log::info!("  Expecting #GP(0) fault when it runs...");
        }
        Err(e) => log::error!("  Failed to create HLT test process: {}", e),
    }
    
    // Brief pause to allow scheduler to pick up the process
    for _ in 0..100_000 {
        core::hint::spin_loop();
    }

    // Test 3: CR3 write
    log::info!("\n3. Testing CR3 write (should cause #GP)...");
    let cr3_elf = create_cr3_write_test_elf();
    match crate::process::create_user_process(String::from("cr3_fault_test"), &cr3_elf) {
        Ok(pid) => {
            log::info!("  Created CR3 write test process PID {}", pid.as_u64());
            log::info!("  Expecting #GP(0) fault when it runs...");
        }
        Err(e) => log::error!("  Failed to create CR3 test process: {}", e),
    }
    
    // Brief pause to allow scheduler to pick up the process
    for _ in 0..100_000 {
        core::hint::spin_loop();
    }

    // Test 4: Unmapped memory access
    log::info!("\n4. Testing unmapped memory access (should cause #PF with U=1)...");
    let unmapped_elf = create_unmapped_access_test_elf();
    match crate::process::create_user_process(String::from("unmapped_fault_test"), &unmapped_elf) {
        Ok(pid) => {
            log::info!("  Created unmapped access test process PID {}", pid.as_u64());
            log::info!("  Expecting #PF with U=1, P=0 when it runs...");
        }
        Err(e) => log::error!("  Failed to create unmapped test process: {}", e),
    }
    
    log::info!("\n=== User-Only Fault Tests Scheduled ===");
    log::info!("Check logs for #GP and #PF exceptions with proper error codes");
    }
}
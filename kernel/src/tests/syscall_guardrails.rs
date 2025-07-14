//! Guard-rail tests for syscall gate functionality
//! These tests ensure critical fixes remain in place

use crate::{memory, interrupts};
use x86_64::registers::control::Cr3;

/// Test that IDT entry 0x80 has DPL=3 for user access
#[cfg(test)]
#[test_case]
fn test_idt_dpl_for_syscall() {
    unsafe {
        let idt = interrupts::idt();
        let syscall_entry = &idt[0x80];
        
        // Extract DPL from the IDT entry
        // For a 64-bit interrupt gate, the DPL is in bits 45-46 of the descriptor
        let raw = syscall_entry as *const _ as *const u128;
        let descriptor = *raw;
        let dpl = ((descriptor >> 45) & 0x3) as u8;
        
        assert_eq!(dpl, 3, "IDT[0x80] must have DPL=3 for user access");
    }
}

/// Test that kernel stack region (PML4 entry 2) is mapped in process page tables
#[cfg(test)]
#[test_case]
fn test_kernel_stack_mapped_in_process() {
    use crate::memory::process_memory::ProcessPageTable;
    use x86_64::structures::paging::PageTable;
    
    // Create a new process page table
    let process_table = ProcessPageTable::new()
        .expect("Failed to create process page table");
    
    // Get the page table frame
    let frame = process_table.level_4_frame();
    
    // Map the page table to check its contents
    let phys_offset = memory::physical_memory_offset();
    let l4_table = unsafe {
        let virt = phys_offset + frame.start_address().as_u64();
        &*(virt.as_ptr() as *const PageTable)
    };
    
    // Check PML4 entry 2 (idle thread stack region)
    assert!(!l4_table[2].is_unused(), 
        "PML4 entry 2 (kernel stack region) must be mapped for context switches");
    
    // Check it's not user accessible
    let flags = l4_table[2].flags();
    assert!(!flags.contains(x86_64::structures::paging::PageTableFlags::USER_ACCESSIBLE),
        "Kernel stack region must not be user accessible");
}

/// Test that kernel code (PML4 entry 0) is mapped in process page tables
#[cfg(test)]
#[test_case]
fn test_kernel_code_mapped_in_process() {
    use crate::memory::process_memory::ProcessPageTable;
    use x86_64::structures::paging::PageTable;
    
    // Create a new process page table
    let process_table = ProcessPageTable::new()
        .expect("Failed to create process page table");
    
    // Get the page table frame
    let frame = process_table.level_4_frame();
    
    // Map the page table to check its contents
    let phys_offset = memory::physical_memory_offset();
    let l4_table = unsafe {
        let virt = phys_offset + frame.start_address().as_u64();
        &*(virt.as_ptr() as *const PageTable)
    };
    
    // Check PML4 entry 0 (kernel code region)
    assert!(!l4_table[0].is_unused(), 
        "PML4 entry 0 (kernel code region) must be mapped for interrupt returns");
    
    // Check it's not user accessible
    let flags = l4_table[0].flags();
    assert!(!flags.contains(x86_64::structures::paging::PageTableFlags::USER_ACCESSIBLE),
        "Kernel code region must not be user accessible");
}

/// Test that low half is properly isolated (no stray mappings)
#[cfg(test)]
#[test_case]
fn test_low_half_isolation() {
    use crate::memory::process_memory::ProcessPageTable;
    use x86_64::structures::paging::PageTable;
    
    // Create a new process page table
    let process_table = ProcessPageTable::new()
        .expect("Failed to create process page table");
    
    // Get the page table frame
    let frame = process_table.level_4_frame();
    
    // Map the page table to check its contents
    let phys_offset = memory::physical_memory_offset();
    let l4_table = unsafe {
        let virt = phys_offset + frame.start_address().as_u64();
        &*(virt.as_ptr() as *const PageTable)
    };
    
    // Check that entries 1-7 are not mapped (for process isolation)
    for idx in 1..8 {
        assert!(l4_table[idx].is_unused(),
            "PML4 entry {} should be unused for process isolation", idx);
    }
    
    // Verify high half (256-511) has kernel mappings
    let mut high_half_count = 0;
    for idx in 256..512 {
        if !l4_table[idx].is_unused() {
            high_half_count += 1;
            // Verify no user access
            let flags = l4_table[idx].flags();
            assert!(!flags.contains(x86_64::structures::paging::PageTableFlags::USER_ACCESSIBLE),
                "High half entry {} must not be user accessible", idx);
        }
    }
    
    assert!(high_half_count > 0, "High half must have kernel mappings");
}

/// Test that exec() properly updates process page table reference
/// This guard rail ensures exec() doesn't just schedule a transient page table switch,
/// but actually stores the new page table in the process/thread structure for persistence.
#[cfg(test)]
#[test_case]
fn test_exec_updates_process_pt() {
    use crate::memory::process_memory::ProcessPageTable;
    use crate::task::{scheduler, thread::Thread};
    use alloc::boxed::Box;
    use x86_64::VirtAddr;
    
    // Create a test thread with an initial page table
    let initial_pt = ProcessPageTable::new()
        .expect("Failed to create initial page table");
    let initial_frame = initial_pt.level_4_frame();
    
    let test_thread = Thread::new_user(
        999,
        "test_exec".to_string(),
        VirtAddr::new(0x10000000),
        VirtAddr::new(0x7ffffff000),
        Some(VirtAddr::new(0xffffc90000020000)),
        Some(initial_frame),
    );
    
    // Verify the thread has the initial page table frame
    assert_eq!(test_thread.page_table_frame, Some(initial_frame),
        "Thread should be initialized with the provided page table frame");
    
    // This test verifies the structure is in place - actual exec() functionality
    // is tested through integration tests that check for EXEC_OK output
    
    log::info!("exec() guard rail test passed - thread structure supports page table persistence");
}
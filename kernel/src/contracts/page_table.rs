//! Page table contract verification
//!
//! Verifies critical invariants for PML4/page table configuration.

use x86_64::structures::paging::{PageTable, PageTableFlags};
use x86_64::PhysAddr;

/// Contract: PML4[402] (kernel stacks) and PML4[403] (IST stacks) must point to DIFFERENT frames
/// This was the root cause of stack corruption bug we fixed
pub fn verify_kernel_ist_frame_separation(pml4: &PageTable) -> Result<(), alloc::string::String> {
    let entry_402 = &pml4[402];
    let entry_403 = &pml4[403];

    // Both entries must be present
    if entry_402.is_unused() {
        return Err(alloc::format!("PML4[402] (kernel stacks) is not present"));
    }
    if entry_403.is_unused() {
        return Err(alloc::format!("PML4[403] (IST stacks) is not present"));
    }

    let frame_402 = entry_402.frame().map_err(|_| "PML4[402] has invalid frame")?;
    let frame_403 = entry_403.frame().map_err(|_| "PML4[403] has invalid frame")?;

    if frame_402 == frame_403 {
        return Err(alloc::format!(
            "CRITICAL: PML4[402] and PML4[403] both point to frame {:?}. \
             This causes stack corruption during exception handling!",
            frame_402
        ));
    }

    Ok(())
}

/// Contract: PML4[2] (direct physical memory mapping) must be present for kernel code execution
pub fn verify_kernel_code_mapping(pml4: &PageTable) -> Result<(), alloc::string::String> {
    let entry_2 = &pml4[2];

    if entry_2.is_unused() {
        return Err(alloc::format!(
            "PML4[2] (direct physical memory mapping) is not present. \
             Kernel code at 0x100_xxxx_xxxx will not be accessible!"
        ));
    }

    let flags = entry_2.flags();
    if !flags.contains(PageTableFlags::PRESENT) {
        return Err(alloc::format!(
            "PML4[2] does not have PRESENT flag set"
        ));
    }

    Ok(())
}

/// Contract: All process page tables must inherit upper-half kernel mappings (256-511)
pub fn verify_kernel_mapping_inheritance(
    process_pml4: &PageTable,
    master_pml4: &PageTable,
) -> Result<(), alloc::string::String> {
    let mut mismatches = alloc::vec::Vec::new();

    for i in 256..512 {
        let master_entry = &master_pml4[i];
        let process_entry = &process_pml4[i];

        if !master_entry.is_unused() {
            if process_entry.is_unused() {
                mismatches.push(alloc::format!("PML4[{}] missing in process", i));
            } else {
                let master_frame = master_entry.frame();
                let process_frame = process_entry.frame();

                if master_frame != process_frame {
                    mismatches.push(alloc::format!(
                        "PML4[{}] frame mismatch: master={:?}, process={:?}",
                        i, master_frame, process_frame
                    ));
                }
            }
        }
    }

    if !mismatches.is_empty() {
        return Err(alloc::format!(
            "Kernel mapping inheritance violations: {}",
            mismatches.join("; ")
        ));
    }

    Ok(())
}

/// Contract: PML4[402] and PML4[403] must both be present in any process page table
pub fn verify_stack_regions_present(pml4: &PageTable) -> Result<(), alloc::string::String> {
    let entry_402 = &pml4[402];
    let entry_403 = &pml4[403];

    let mut errors = alloc::vec::Vec::new();

    if entry_402.is_unused() {
        errors.push("PML4[402] (kernel stacks at 0xffffc90000000000) is missing");
    }

    if entry_403.is_unused() {
        errors.push("PML4[403] (IST stacks at 0xffffc98000000000) is missing");
    }

    if !errors.is_empty() {
        return Err(alloc::format!(
            "Missing critical stack regions: {}",
            errors.join("; ")
        ));
    }

    Ok(())
}

/// Contract: Page table flags must be correct (PRESENT, WRITABLE, etc.)
#[allow(dead_code)]
pub fn verify_entry_flags(
    pml4: &PageTable,
    index: usize,
    required_flags: PageTableFlags,
) -> Result<(), alloc::string::String> {
    if index >= 512 {
        return Err(alloc::format!("Invalid PML4 index: {}", index));
    }

    let entry = &pml4[index];

    if entry.is_unused() {
        return Err(alloc::format!("PML4[{}] is not present", index));
    }

    let actual_flags = entry.flags();

    if !actual_flags.contains(required_flags) {
        let missing = required_flags - (actual_flags & required_flags);
        return Err(alloc::format!(
            "PML4[{}] missing required flags: {:?}",
            index, missing
        ));
    }

    Ok(())
}

/// Contract: Verify that a PML4 entry points to a valid physical frame
pub fn verify_valid_frame(pml4: &PageTable, index: usize) -> Result<PhysAddr, alloc::string::String> {
    if index >= 512 {
        return Err(alloc::format!("Invalid PML4 index: {}", index));
    }

    let entry = &pml4[index];

    if entry.is_unused() {
        return Err(alloc::format!("PML4[{}] is unused", index));
    }

    let addr = entry.addr();

    // Basic sanity checks on the physical address
    if addr.as_u64() == 0 && entry.flags().contains(PageTableFlags::PRESENT) {
        return Err(alloc::format!(
            "PML4[{}] has PRESENT flag but points to physical address 0x0",
            index
        ));
    }

    // Check that address is page-aligned
    if !addr.is_aligned(4096u64) {
        return Err(alloc::format!(
            "PML4[{}] frame address {:#x} is not page-aligned",
            index, addr.as_u64()
        ));
    }

    Ok(addr)
}

/// Contract: Verify all critical kernel PML4 entries are valid
pub fn verify_all_kernel_entries(pml4: &PageTable) -> Result<(), alloc::string::String> {
    let mut errors = alloc::vec::Vec::new();

    // PML4[2] - direct physical memory mapping (kernel code execution)
    if let Err(e) = verify_kernel_code_mapping(pml4) {
        errors.push(e);
    }

    // PML4[402] and PML4[403] - kernel and IST stacks
    if let Err(e) = verify_stack_regions_present(pml4) {
        errors.push(e);
    }

    // Verify frame separation
    if let Err(e) = verify_kernel_ist_frame_separation(pml4) {
        errors.push(e);
    }

    // Verify upper-half entries have valid frames
    for i in 256..512 {
        if !pml4[i].is_unused() {
            if let Err(e) = verify_valid_frame(pml4, i) {
                errors.push(e);
            }
        }
    }

    if !errors.is_empty() {
        return Err(alloc::format!(
            "Kernel PML4 validation failed:\n  {}",
            errors.join("\n  ")
        ));
    }

    Ok(())
}

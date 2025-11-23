//! Kernel stack contract verification
//!
//! Verifies invariants related to kernel stack mapping and TSS RSP0.

use x86_64::VirtAddr;
use x86_64::structures::paging::{PageTable, PageTableFlags};

/// Contract: Kernel stack at given address must be mapped with correct flags
pub fn verify_stack_mapping(
    vaddr: VirtAddr,
    phys_offset: VirtAddr,
) -> Result<(), alloc::string::String> {
    // Get current PML4
    let (pml4_frame, _) = x86_64::registers::control::Cr3::read();

    let pml4_virt = phys_offset + pml4_frame.start_address().as_u64();
    let pml4 = unsafe { &*(pml4_virt.as_ptr() as *const PageTable) };

    // Calculate page table indices
    let pml4_idx = ((vaddr.as_u64() >> 39) & 0x1FF) as usize;
    let pdpt_idx = ((vaddr.as_u64() >> 30) & 0x1FF) as usize;
    let pd_idx = ((vaddr.as_u64() >> 21) & 0x1FF) as usize;
    let pt_idx = ((vaddr.as_u64() >> 12) & 0x1FF) as usize;

    // Walk the page tables
    if pml4[pml4_idx].is_unused() {
        return Err(alloc::format!(
            "Stack address {:#x}: PML4[{}] is not present",
            vaddr.as_u64(), pml4_idx
        ));
    }

    let pdpt_frame = pml4[pml4_idx].frame().map_err(|_|
        alloc::format!("Stack address {:#x}: PML4[{}] has invalid frame", vaddr.as_u64(), pml4_idx)
    )?;
    let pdpt_virt = phys_offset + pdpt_frame.start_address().as_u64();
    let pdpt = unsafe { &*(pdpt_virt.as_ptr() as *const PageTable) };

    if pdpt[pdpt_idx].is_unused() {
        return Err(alloc::format!(
            "Stack address {:#x}: PDPT[{}] is not present",
            vaddr.as_u64(), pdpt_idx
        ));
    }

    // Check for huge page at PDPT level
    if pdpt[pdpt_idx].flags().contains(PageTableFlags::HUGE_PAGE) {
        // 1GB huge page - stack is mapped
        return Ok(());
    }

    let pd_frame = pdpt[pdpt_idx].frame().map_err(|_|
        alloc::format!("Stack address {:#x}: PDPT[{}] has invalid frame", vaddr.as_u64(), pdpt_idx)
    )?;
    let pd_virt = phys_offset + pd_frame.start_address().as_u64();
    let pd = unsafe { &*(pd_virt.as_ptr() as *const PageTable) };

    if pd[pd_idx].is_unused() {
        return Err(alloc::format!(
            "Stack address {:#x}: PD[{}] is not present",
            vaddr.as_u64(), pd_idx
        ));
    }

    // Check for huge page at PD level
    if pd[pd_idx].flags().contains(PageTableFlags::HUGE_PAGE) {
        // 2MB huge page - stack is mapped
        return Ok(());
    }

    let pt_frame = pd[pd_idx].frame().map_err(|_|
        alloc::format!("Stack address {:#x}: PD[{}] has invalid frame", vaddr.as_u64(), pd_idx)
    )?;
    let pt_virt = phys_offset + pt_frame.start_address().as_u64();
    let pt = unsafe { &*(pt_virt.as_ptr() as *const PageTable) };

    if pt[pt_idx].is_unused() {
        return Err(alloc::format!(
            "Stack address {:#x}: PT[{}] is not present (page not mapped)",
            vaddr.as_u64(), pt_idx
        ));
    }

    // Verify flags
    let flags = pt[pt_idx].flags();
    if !flags.contains(PageTableFlags::PRESENT) {
        return Err(alloc::format!(
            "Stack address {:#x}: page not present",
            vaddr.as_u64()
        ));
    }
    if !flags.contains(PageTableFlags::WRITABLE) {
        return Err(alloc::format!(
            "Stack address {:#x}: page not writable",
            vaddr.as_u64()
        ));
    }

    Ok(())
}

/// Contract: Guard pages must NOT be mapped (to catch stack overflow)
pub fn verify_guard_page_unmapped(
    guard_addr: VirtAddr,
    phys_offset: VirtAddr,
) -> Result<(), alloc::string::String> {
    // Get current PML4
    let (pml4_frame, _) = x86_64::registers::control::Cr3::read();

    let pml4_virt = phys_offset + pml4_frame.start_address().as_u64();
    let pml4 = unsafe { &*(pml4_virt.as_ptr() as *const PageTable) };

    // Calculate page table indices
    let pml4_idx = ((guard_addr.as_u64() >> 39) & 0x1FF) as usize;
    let pdpt_idx = ((guard_addr.as_u64() >> 30) & 0x1FF) as usize;
    let pd_idx = ((guard_addr.as_u64() >> 21) & 0x1FF) as usize;
    let pt_idx = ((guard_addr.as_u64() >> 12) & 0x1FF) as usize;

    // Walk the page tables - it's OK if any level is not present
    if pml4[pml4_idx].is_unused() {
        return Ok(()); // Guard page is effectively unmapped
    }

    let pdpt_frame = match pml4[pml4_idx].frame() {
        Ok(f) => f,
        Err(_) => return Ok(()), // Invalid = unmapped
    };
    let pdpt_virt = phys_offset + pdpt_frame.start_address().as_u64();
    let pdpt = unsafe { &*(pdpt_virt.as_ptr() as *const PageTable) };

    if pdpt[pdpt_idx].is_unused() {
        return Ok(());
    }

    if pdpt[pdpt_idx].flags().contains(PageTableFlags::HUGE_PAGE) {
        // Guard page falls in a 1GB huge page - this is wrong!
        return Err(alloc::format!(
            "Guard page at {:#x} is within a 1GB huge page - not protected!",
            guard_addr.as_u64()
        ));
    }

    let pd_frame = match pdpt[pdpt_idx].frame() {
        Ok(f) => f,
        Err(_) => return Ok(()),
    };
    let pd_virt = phys_offset + pd_frame.start_address().as_u64();
    let pd = unsafe { &*(pd_virt.as_ptr() as *const PageTable) };

    if pd[pd_idx].is_unused() {
        return Ok(());
    }

    if pd[pd_idx].flags().contains(PageTableFlags::HUGE_PAGE) {
        // Guard page falls in a 2MB huge page - this is wrong!
        return Err(alloc::format!(
            "Guard page at {:#x} is within a 2MB huge page - not protected!",
            guard_addr.as_u64()
        ));
    }

    let pt_frame = match pd[pd_idx].frame() {
        Ok(f) => f,
        Err(_) => return Ok(()),
    };
    let pt_virt = phys_offset + pt_frame.start_address().as_u64();
    let pt = unsafe { &*(pt_virt.as_ptr() as *const PageTable) };

    // The guard page SHOULD be unmapped at the PT level
    if !pt[pt_idx].is_unused() && pt[pt_idx].flags().contains(PageTableFlags::PRESENT) {
        return Err(alloc::format!(
            "Guard page at {:#x} is mapped! Stack overflow will not be caught.",
            guard_addr.as_u64()
        ));
    }

    Ok(())
}

/// Contract: TSS RSP0 must point to valid kernel stack region (PML4[402])
pub fn verify_tss_rsp0_valid() -> Result<(), alloc::string::String> {
    let tss_ptr = crate::gdt::get_tss_ptr();
    if tss_ptr.is_null() {
        return Err("TSS not initialized".into());
    }

    let rsp0 = unsafe { (*tss_ptr).privilege_stack_table[0].as_u64() };

    if rsp0 == 0 {
        return Err("TSS RSP0 is null".into());
    }

    // RSP0 should be in the kernel stack region (PML4[402] = 0xffffc90000000000)
    let pml4_idx = (rsp0 >> 39) & 0x1FF;

    if pml4_idx != 402 {
        return Err(alloc::format!(
            "TSS RSP0 ({:#x}) is in PML4[{}], expected PML4[402] (kernel stack region)",
            rsp0, pml4_idx
        ));
    }

    Ok(())
}

/// Contract: Verify that a stack region is properly set up
pub fn verify_stack_region(
    stack_top: VirtAddr,
    stack_size: usize,
    phys_offset: VirtAddr,
) -> Result<(), alloc::string::String> {
    // Check that all pages in the stack region are mapped
    let stack_bottom = stack_top - stack_size as u64;
    let page_size = 4096u64;

    let mut addr = stack_bottom.as_u64();
    while addr < stack_top.as_u64() {
        if let Err(e) = verify_stack_mapping(VirtAddr::new(addr), phys_offset) {
            return Err(alloc::format!(
                "Stack region {:#x}-{:#x} validation failed at {:#x}: {}",
                stack_bottom.as_u64(), stack_top.as_u64(), addr, e
            ));
        }
        addr += page_size;
    }

    Ok(())
}

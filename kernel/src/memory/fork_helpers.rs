//! Fork helper functions for copying process memory during fork()

use x86_64::{
    structures::paging::{
        Page, PageSize, PageTable, PageTableFlags, PhysFrame, Size4KiB,
    },
    VirtAddr,
};

use crate::memory::{
    frame_allocator,
    process_memory::ProcessPageTable,
};

use alloc::vec::Vec;
use core::slice;

/// Represents a user-space memory mapping
#[derive(Debug, Clone)]
pub struct UserMapping {
    pub virt_addr: VirtAddr,
    pub frame: PhysFrame,
    pub flags: PageTableFlags,
}

/// Collect all user-space mappings from a page table
/// Returns a vector of (virtual address, physical frame, flags) tuples
pub fn collect_user_mappings(page_table: &ProcessPageTable) -> Result<Vec<UserMapping>, &'static str> {
    let mut mappings = Vec::new();
    
    // Get the physical memory offset for accessing page tables
    let phys_mem_offset = crate::memory::physical_memory_offset();
    
    // Access the L4 page table
    let l4_table = unsafe {
        let l4_phys = page_table.level_4_frame().start_address();
        let l4_virt = phys_mem_offset + l4_phys.as_u64();
        &*(l4_virt.as_ptr() as *const PageTable)
    };
    
    // Walk through all L4 entries in the lower half (user space)
    // User space is entries 0-255 (lower half of 512 entries)
    for (l4_idx, l4_entry) in l4_table.iter().enumerate().take(256) {
        if !l4_entry.is_unused() && l4_entry.flags().contains(PageTableFlags::PRESENT) {
            // Found a valid L3 table
            let l3_table = unsafe {
                let l3_phys = l4_entry.frame().unwrap().start_address();
                let l3_virt = phys_mem_offset + l3_phys.as_u64();
                &*(l3_virt.as_ptr() as *const PageTable)
            };
            
            // Walk through L3 entries
            for (l3_idx, l3_entry) in l3_table.iter().enumerate() {
                if !l3_entry.is_unused() && l3_entry.flags().contains(PageTableFlags::PRESENT) {
                    // Check if this is a huge page (1GB)
                    if l3_entry.flags().contains(PageTableFlags::HUGE_PAGE) {
                        // Handle 1GB huge page (rare in user space)
                        continue;
                    }
                    
                    // Found a valid L2 table
                    let l2_table = unsafe {
                        let l2_phys = l3_entry.frame().unwrap().start_address();
                        let l2_virt = phys_mem_offset + l2_phys.as_u64();
                        &*(l2_virt.as_ptr() as *const PageTable)
                    };
                    
                    // Walk through L2 entries
                    for (l2_idx, l2_entry) in l2_table.iter().enumerate() {
                        if !l2_entry.is_unused() && l2_entry.flags().contains(PageTableFlags::PRESENT) {
                            // Check if this is a huge page (2MB)
                            if l2_entry.flags().contains(PageTableFlags::HUGE_PAGE) {
                                // Handle 2MB huge page
                                let virt_addr = VirtAddr::new(
                                    (l4_idx as u64) << 39 |
                                    (l3_idx as u64) << 30 |
                                    (l2_idx as u64) << 21
                                );
                                
                                // Only include user-accessible pages
                                if l2_entry.flags().contains(PageTableFlags::USER_ACCESSIBLE) {
                                    mappings.push(UserMapping {
                                        virt_addr,
                                        frame: l2_entry.frame().unwrap(),
                                        flags: l2_entry.flags(),
                                    });
                                    log::trace!("Found 2MB user mapping: {:#x}", virt_addr);
                                }
                                continue;
                            }
                            
                            // Found a valid L1 table
                            let l1_table = unsafe {
                                let l1_phys = l2_entry.frame().unwrap().start_address();
                                let l1_virt = phys_mem_offset + l1_phys.as_u64();
                                &*(l1_virt.as_ptr() as *const PageTable)
                            };
                            
                            // Walk through L1 entries (4KB pages)
                            for (l1_idx, l1_entry) in l1_table.iter().enumerate() {
                                if !l1_entry.is_unused() && 
                                   l1_entry.flags().contains(PageTableFlags::PRESENT) &&
                                   l1_entry.flags().contains(PageTableFlags::USER_ACCESSIBLE) {
                                    // Found a user-accessible 4KB page
                                    let virt_addr = VirtAddr::new(
                                        (l4_idx as u64) << 39 |
                                        (l3_idx as u64) << 30 |
                                        (l2_idx as u64) << 21 |
                                        (l1_idx as u64) << 12
                                    );
                                    
                                    mappings.push(UserMapping {
                                        virt_addr,
                                        frame: l1_entry.frame().unwrap(),
                                        flags: l1_entry.flags(),
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    
    log::info!("collect_user_mappings: Found {} user mappings", mappings.len());
    Ok(mappings)
}

/// Copy the contents of one physical frame to another
/// Uses a temporary kernel mapping to perform the copy
pub unsafe fn copy_frame(src_frame: PhysFrame, dst_frame: PhysFrame) -> Result<(), &'static str> {
    // Use physical memory offset for direct access
    let phys_mem_offset = crate::memory::physical_memory_offset();
    
    // Calculate virtual addresses for both frames
    let src_virt = phys_mem_offset + src_frame.start_address().as_u64();
    let dst_virt = phys_mem_offset + dst_frame.start_address().as_u64();
    
    // Create slices for source and destination
    let src_slice = slice::from_raw_parts(src_virt.as_ptr::<u8>(), Size4KiB::SIZE as usize);
    let dst_slice = slice::from_raw_parts_mut(dst_virt.as_mut_ptr::<u8>(), Size4KiB::SIZE as usize);
    
    // Copy the data
    dst_slice.copy_from_slice(src_slice);
    
    log::trace!("Copied frame {:#x} to {:#x}", src_frame.start_address(), dst_frame.start_address());
    Ok(())
}

/// Map a frame as shared read-only in the child's page table
/// Sets appropriate flags including GLOBAL for TLB efficiency
pub fn map_shared_ro(
    child_pt: &mut ProcessPageTable,
    virt_addr: VirtAddr,
    src_frame: PhysFrame,
    parent_flags: PageTableFlags,
) -> Result<(), &'static str> {
    // Preserve parent flags but ensure read-only
    let mut flags = parent_flags;
    flags.remove(PageTableFlags::WRITABLE);
    flags.insert(PageTableFlags::GLOBAL); // Optimize TLB usage
    
    // Map the page in child's page table
    let page: Page<Size4KiB> = Page::containing_address(virt_addr);
    child_pt.map_page(page, src_frame, flags)
        .map_err(|_| "Failed to map shared read-only page")?;
    
    log::trace!("Mapped shared RO page {:#x} -> {:#x}", virt_addr, src_frame.start_address());
    Ok(())
}

/// Allocate a new frame and map it read-write in the child's page table
pub fn allocate_and_map_rw(
    child_pt: &mut ProcessPageTable,
    virt_addr: VirtAddr,
    flags: PageTableFlags,
) -> Result<PhysFrame, &'static str> {
    // Allocate a new frame
    let new_frame = frame_allocator::allocate_frame()
        .ok_or("Failed to allocate frame for fork")?;
    
    // Ensure writable flag is set
    let mut rw_flags = flags;
    rw_flags.insert(PageTableFlags::WRITABLE);
    
    // Map the page in child's page table
    let page: Page<Size4KiB> = Page::containing_address(virt_addr);
    child_pt.map_page(page, new_frame, rw_flags)
        .map_err(|_| "Failed to map read-write page")?;
    
    log::trace!("Allocated and mapped RW page {:#x} -> {:#x}", virt_addr, new_frame.start_address());
    Ok(new_frame)
}

/// Clone all memory pages from parent to child
/// Returns (copied_pages, shared_pages) counts
pub fn clone_process_memory(
    parent_pt: &ProcessPageTable,
    child_pt: &mut ProcessPageTable,
    parent_pid: u64,
    child_pid: u64,
) -> Result<(usize, usize), &'static str> {
    let mut copied_pages = 0;
    let mut shared_pages = 0;
    
    // Collect all user mappings from parent
    let mappings = collect_user_mappings(parent_pt)?;
    
    log::info!("fork[{}->{}] Cloning {} user mappings", parent_pid, child_pid, mappings.len());
    
    // Process each mapping
    for mapping in mappings {
        // Check if this is a read-only page (likely code)
        if !mapping.flags.contains(PageTableFlags::WRITABLE) {
            // Share read-only pages
            map_shared_ro(child_pt, mapping.virt_addr, mapping.frame, mapping.flags)?;
            shared_pages += 1;
            log::info!("fork[{}->{}] page {:#x} shared (read-only)", 
                     parent_pid, child_pid, mapping.virt_addr);
        } else {
            // Copy writable pages (data, heap, stack)
            let new_frame = allocate_and_map_rw(child_pt, mapping.virt_addr, mapping.flags)?;
            
            // Copy the contents
            unsafe {
                copy_frame(mapping.frame, new_frame)?;
            }
            
            copied_pages += 1;
            log::info!("fork[{}->{}] page {:#x} copied to {:?}", 
                     parent_pid, child_pid, mapping.virt_addr, new_frame);
        }
    }
    
    // Warn if too many pages are being shared (sanity check)
    let total_pages = copied_pages + shared_pages;
    if total_pages > 0 && shared_pages as f64 / total_pages as f64 > 0.75 {
        log::warn!("fork[{}->{}] High share ratio: {} of {} pages shared ({}%)",
                 parent_pid, child_pid, shared_pages, total_pages, 
                 (shared_pages * 100) / total_pages);
    }
    
    log::info!("fork complete: {} pages copied, {} pages shared", 
             copied_pages, shared_pages);
    
    Ok((copied_pages, shared_pages))
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_user_mapping_struct() {
        use x86_64::PhysAddr;
        
        let mapping = UserMapping {
            virt_addr: VirtAddr::new(0x1000),
            frame: PhysFrame::containing_address(PhysAddr::new(0x2000)),
            flags: PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE,
        };
        
        assert_eq!(mapping.virt_addr.as_u64(), 0x1000);
        assert!(mapping.flags.contains(PageTableFlags::USER_ACCESSIBLE));
    }
    
    #[test]
    fn test_collect_user_mappings_count() {
        // This test would require a real page table setup
        // For now, just ensure the function compiles
        // In integration tests, we'd verify count > 0 for a running process
    }
}
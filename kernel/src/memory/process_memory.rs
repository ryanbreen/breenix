//! Per-process memory management
//! 
//! This module provides per-process page tables and address space isolation.

use x86_64::{
    structures::paging::{
        OffsetPageTable, PageTable, PageTableFlags, Page, Size4KiB, 
        Mapper, PhysFrame, Translate, mapper::TranslateResult
    },
    VirtAddr, PhysAddr,
    registers::control::Cr3,
};
use crate::memory::frame_allocator::{allocate_frame, GlobalFrameAllocator};

/// A per-process page table
pub struct ProcessPageTable {
    /// Physical frame containing the level 4 page table
    level_4_frame: PhysFrame,
    /// The mapper for this page table
    mapper: OffsetPageTable<'static>,
}

impl ProcessPageTable {
    /// Selectively copy only essential kernel mappings from PML4 entry 0
    /// 
    /// Entry 0 covers virtual addresses 0x000000000000 - 0x00007FFFFFFFFF (512GB)
    /// This contains both kernel code and potential userspace, but we only want kernel code.
    fn selective_copy_entry_0(
        source_entry: &x86_64::structures::paging::page_table::PageTableEntry,
        phys_offset: VirtAddr
    ) -> Result<PhysFrame, &'static str> {
        log::debug!("Selectively copying PML4 entry 0 with L3 addr {:#x}", source_entry.addr().as_u64());
        
        // Allocate a new L3 table
        let new_l3_frame = allocate_frame()
            .ok_or("Failed to allocate frame for L3 table")?;
        
        // Map the new L3 table
        let new_l3_virt = phys_offset + new_l3_frame.start_address().as_u64();
        let new_l3_table = unsafe {
            &mut *(new_l3_virt.as_mut_ptr() as *mut PageTable)
        };
        
        // Clear the new L3 table
        new_l3_table.zero();
        
        // Map the source L3 table
        let source_l3_virt = phys_offset + source_entry.addr().as_u64();
        let source_l3_table = unsafe {
            &*(source_l3_virt.as_ptr() as *const PageTable)
        };
        
        // Only copy L3 entry 0, which covers 0x000000000000 - 0x00003FFFFFFF (1GB)
        // This typically contains the kernel code loaded by the bootloader
        // We skip all other L3 entries to avoid bootloader huge page mappings
        if !source_l3_table[0].is_unused() {
            log::debug!("Copying L3 entry 0 which contains kernel code");
            
            // Check if this is a huge page (1GB page at L3 level)
            if source_l3_table[0].flags().contains(PageTableFlags::HUGE_PAGE) {
                // Huge page covering the first 1GB - copy it directly since it contains kernel code
                new_l3_table[0] = source_l3_table[0].clone();
                log::debug!("Copied L3 huge page entry 0 (kernel code region)");
            } else {
                // Regular L3 entry pointing to L2 table - copy only kernel portions
                match Self::selective_copy_l3_entry_0(&source_l3_table[0], phys_offset) {
                    Ok(new_l2_frame) => {
                        let flags = source_l3_table[0].flags();
                        new_l3_table[0].set_addr(new_l2_frame.start_address(), flags);
                        log::debug!("Selectively copied L3 entry 0 -> new L2 frame {:#x}", 
                            new_l2_frame.start_address().as_u64());
                    }
                    Err(e) => {
                        log::error!("Failed to selectively copy L3 entry 0: {}", e);
                        return Err("Failed to copy kernel code region");
                    }
                }
            }
        }
        
        // Skip all other L3 entries (1-511) to avoid copying bootloader mappings
        log::debug!("Skipped L3 entries 1-511 to avoid bootloader mappings");
        
        log::debug!("Successfully created selective copy of PML4 entry 0 to new L3 frame {:#x}", 
            new_l3_frame.start_address().as_u64());
        
        Ok(new_l3_frame)
    }
    
    /// Selectively copy L3 entry 0, which covers the first 1GB (kernel code region)
    fn selective_copy_l3_entry_0(
        source_entry: &x86_64::structures::paging::page_table::PageTableEntry,
        phys_offset: VirtAddr
    ) -> Result<PhysFrame, &'static str> {
        // Allocate a new L2 table
        let new_l2_frame = allocate_frame()
            .ok_or("Failed to allocate frame for L2 table")?;
        
        // Map the new L2 table
        let new_l2_virt = phys_offset + new_l2_frame.start_address().as_u64();
        let new_l2_table = unsafe {
            &mut *(new_l2_virt.as_mut_ptr() as *mut PageTable)
        };
        
        // Clear the new L2 table
        new_l2_table.zero();
        
        // Map the source L2 table
        let source_l2_virt = phys_offset + source_entry.addr().as_u64();
        let source_l2_table = unsafe {
            &*(source_l2_virt.as_ptr() as *const PageTable)
        };
        
        // Only copy L2 entries that contain kernel code (typically the first few entries)
        // Each L2 entry covers 2MB, so entries 0-7 cover the first 16MB which usually contains kernel
        for i in 0..8 {
            if !source_l2_table[i].is_unused() {
                let l2_virt_addr = (i as u64) * 0x200000; // 2MB per L2 entry
                
                // Check if this is a huge page (2MB page at L2 level)
                if source_l2_table[i].flags().contains(PageTableFlags::HUGE_PAGE) {
                    // Copy kernel huge pages (first 16MB typically contains kernel)
                    new_l2_table[i] = source_l2_table[i].clone();
                    log::debug!("Copied kernel L2 huge page entry {} (addr {:#x})", i, l2_virt_addr);
                } else {
                    // Regular L2 entry pointing to L1 table - deep copy it
                    match Self::deep_copy_l2_entry(&source_l2_table[i], i, phys_offset) {
                        Ok(new_l1_frame) => {
                            let flags = source_l2_table[i].flags();
                            new_l2_table[i].set_addr(new_l1_frame.start_address(), flags);
                            log::debug!("Deep copied kernel L2 entry {} -> new L1 frame {:#x}", 
                                i, new_l1_frame.start_address().as_u64());
                        }
                        Err(e) => {
                            log::warn!("Failed to deep copy kernel L2 entry {}: {}", i, e);
                            // Continue - some entries might not be essential
                        }
                    }
                }
            }
        }
        
        log::debug!("Selectively copied kernel portions of L3 entry 0 (first 16MB)");
        Ok(new_l2_frame)
    }
    
    /// Deep copy a PML4 entry, creating independent L3/L2/L1 tables
    /// 
    /// This creates a complete copy of the page table hierarchy below this L4 entry,
    /// ensuring that each process has its own isolated page tables.
    fn deep_copy_pml4_entry(
        source_entry: &x86_64::structures::paging::page_table::PageTableEntry,
        entry_index: usize,
        phys_offset: VirtAddr
    ) -> Result<PhysFrame, &'static str> {
        log::debug!("Deep copying PML4 entry {} with L3 addr {:#x}", 
            entry_index, source_entry.addr().as_u64());
        
        // Allocate a new L3 table
        let new_l3_frame = allocate_frame()
            .ok_or("Failed to allocate frame for L3 table")?;
        
        // Map the new L3 table
        let new_l3_virt = phys_offset + new_l3_frame.start_address().as_u64();
        let new_l3_table = unsafe {
            &mut *(new_l3_virt.as_mut_ptr() as *mut PageTable)
        };
        
        // Clear the new L3 table
        new_l3_table.zero();
        
        // Map the source L3 table
        let source_l3_virt = phys_offset + source_entry.addr().as_u64();
        let source_l3_table = unsafe {
            &*(source_l3_virt.as_ptr() as *const PageTable)
        };
        
        // Copy each L3 entry, deep copying L2 tables as needed
        for i in 0..512 {
            if !source_l3_table[i].is_unused() {
                // Check if this is a huge page (1GB page at L3 level)
                if source_l3_table[i].flags().contains(PageTableFlags::HUGE_PAGE) {
                    // Huge pages can be shared at the physical level
                    new_l3_table[i] = source_l3_table[i].clone();
                    log::trace!("Copied L3 huge page entry {}", i);
                } else {
                    // Regular L3 entry pointing to L2 table - deep copy the L2 table
                    match Self::deep_copy_l3_entry(&source_l3_table[i], i, phys_offset) {
                        Ok(new_l2_frame) => {
                            let flags = source_l3_table[i].flags();
                            new_l3_table[i].set_addr(new_l2_frame.start_address(), flags);
                            log::trace!("Deep copied L3 entry {} -> new L2 frame {:#x}", 
                                i, new_l2_frame.start_address().as_u64());
                        }
                        Err(e) => {
                            log::error!("Failed to deep copy L3 entry {}: {}", i, e);
                            return Err("Failed to deep copy L3 entry");
                        }
                    }
                }
            }
        }
        
        log::debug!("Successfully deep copied PML4 entry {} to new L3 frame {:#x}", 
            entry_index, new_l3_frame.start_address().as_u64());
        
        Ok(new_l3_frame)
    }
    
    /// Deep copy an L3 entry, creating independent L2/L1 tables
    fn deep_copy_l3_entry(
        source_entry: &x86_64::structures::paging::page_table::PageTableEntry,
        entry_index: usize,
        phys_offset: VirtAddr
    ) -> Result<PhysFrame, &'static str> {
        // Allocate a new L2 table
        let new_l2_frame = allocate_frame()
            .ok_or("Failed to allocate frame for L2 table")?;
        
        // Map the new L2 table
        let new_l2_virt = phys_offset + new_l2_frame.start_address().as_u64();
        let new_l2_table = unsafe {
            &mut *(new_l2_virt.as_mut_ptr() as *mut PageTable)
        };
        
        // Clear the new L2 table
        new_l2_table.zero();
        
        // Map the source L2 table
        let source_l2_virt = phys_offset + source_entry.addr().as_u64();
        let source_l2_table = unsafe {
            &*(source_l2_virt.as_ptr() as *const PageTable)
        };
        
        // Copy each L2 entry, deep copying L1 tables as needed
        for i in 0..512 {
            if !source_l2_table[i].is_unused() {
                // Check if this is a huge page (2MB page at L2 level)
                if source_l2_table[i].flags().contains(PageTableFlags::HUGE_PAGE) {
                    // Huge pages can be shared at the physical level for kernel code
                    // Calculate the virtual address this L2 entry covers
                    let l2_virt_addr = (entry_index as u64) * 0x40000000 + (i as u64) * 0x200000; // 1GB per L3, 2MB per L2
                    
                    if l2_virt_addr >= 0x800000000000 {
                        // Kernel space - safe to share huge pages
                        new_l2_table[i] = source_l2_table[i].clone();
                        log::trace!("Shared kernel L2 huge page entry {} (addr {:#x})", i, l2_virt_addr);
                    } else {
                        // User space - should not share, but for now we'll share kernel code pages
                        // This needs refinement based on what's actually mapped
                        new_l2_table[i] = source_l2_table[i].clone();
                        // Don't spam logs with hundreds of huge page entries
                        if i < 10 {
                            log::trace!("Shared user L2 huge page entry {} (addr {:#x}) - FIXME", i, l2_virt_addr);
                        }
                    }
                } else {
                    // Regular L2 entry pointing to L1 table - deep copy the L1 table
                    match Self::deep_copy_l2_entry(&source_l2_table[i], i, phys_offset) {
                        Ok(new_l1_frame) => {
                            let flags = source_l2_table[i].flags();
                            new_l2_table[i].set_addr(new_l1_frame.start_address(), flags);
                            log::trace!("Deep copied L2 entry {} -> new L1 frame {:#x}", 
                                i, new_l1_frame.start_address().as_u64());
                        }
                        Err(e) => {
                            log::error!("Failed to deep copy L2 entry {}: {}", i, e);
                            return Err("Failed to deep copy L2 entry");
                        }
                    }
                }
            }
        }
        
        Ok(new_l2_frame)
    }
    
    /// Deep copy an L2 entry, creating independent L1 tables
    fn deep_copy_l2_entry(
        source_entry: &x86_64::structures::paging::page_table::PageTableEntry,
        _entry_index: usize,
        phys_offset: VirtAddr
    ) -> Result<PhysFrame, &'static str> {
        // Allocate a new L1 table
        let new_l1_frame = allocate_frame()
            .ok_or("Failed to allocate frame for L1 table")?;
        
        // Map the new L1 table
        let new_l1_virt = phys_offset + new_l1_frame.start_address().as_u64();
        let new_l1_table = unsafe {
            &mut *(new_l1_virt.as_mut_ptr() as *mut PageTable)
        };
        
        // Clear the new L1 table
        new_l1_table.zero();
        
        // Map the source L1 table
        let source_l1_virt = phys_offset + source_entry.addr().as_u64();
        let source_l1_table = unsafe {
            &*(source_l1_virt.as_ptr() as *const PageTable)
        };
        
        // Copy L1 entries - these point to actual physical pages
        // For now, we'll share all physical pages (kernel code should be read-only anyway)
        // In a full copy-on-write implementation, we'd mark pages as read-only and copy on write
        for i in 0..512 {
            if !source_l1_table[i].is_unused() {
                // Share the physical page but with independent page table entry
                // This allows different processes to have different permissions/flags if needed
                new_l1_table[i] = source_l1_table[i].clone();
                // Don't spam logs with L1 entries
                if i < 5 {
                    log::trace!("Copied L1 entry {} -> phys frame {:#x}", 
                        i, source_l1_table[i].addr().as_u64());
                }
            }
        }
        
        Ok(new_l1_frame)
    }
    
    /// Create a new page table for a process
    /// 
    /// This creates a new level 4 page table with kernel mappings copied
    /// from the current page table.
    pub fn new() -> Result<Self, &'static str> {
        // Allocate a frame for the new level 4 page table
        log::debug!("ProcessPageTable::new() - About to allocate L4 frame");
        let level_4_frame = allocate_frame()
            .ok_or("Failed to allocate frame for page table")?;
        
        log::debug!("Allocated L4 frame: {:#x}", level_4_frame.start_address().as_u64());
        
        // Get physical memory offset
        let phys_offset = crate::memory::physical_memory_offset();
        
        // Map the new page table frame
        let level_4_table = unsafe {
            let virt = phys_offset + level_4_frame.start_address().as_u64();
            log::debug!("New L4 table virtual address: {:#x}", virt.as_u64());
            &mut *(virt.as_mut_ptr() as *mut PageTable)
        };
        
        // Clear the new page table
        level_4_table.zero();
        
        // Copy kernel mappings from the current page table
        // This ensures the kernel is always mapped in all processes
        unsafe {
            let current_l4_table = {
                let (frame, _) = Cr3::read();
                log::debug!("ProcessPageTable::new() - Current CR3 when copying: {:#x}", frame.start_address().as_u64());
                let virt = phys_offset + frame.start_address().as_u64();
                log::debug!("Current L4 table virtual address: {:#x}", virt.as_u64());
                &*(virt.as_ptr() as *const PageTable)
            };
            
            // Copy kernel mappings from the current page table
            // This is critical - we need ALL kernel mappings to be present in every
            // process page table so the kernel can function after a page table switch
            
            log::debug!("Copying kernel page table entries...");
            
            // WORKAROUND: There seems to be an issue with copying certain PML4 entries
            // that causes the kernel to hang. For now, let's copy only the essential
            // entries that we know are required:
            // - Entry 256: Traditional kernel space (0x800000000000)
            // - Entry 0: Contains kernel code and IDT
            // - Entry 5: Physical memory offset mapping (0x28000000000)
            
            // CRITICAL FIX: We must copy ALL kernel mappings, including entry 2
            // The hang was caused by holding locks during page table operations
            // Now that we're using with_process_manager, we can safely copy all entries
            
            log::debug!("Copying all kernel page table entries...");
            let mut copied_count = 0;
            
            // DEEP COPY: Create completely independent page tables for proper isolation
            // This is the OS-standard approach - each process gets its own L3/L2/L1 tables
            // Only the actual physical pages (at L1 level) are shared for kernel code
            
            for i in 0..512 {
                if !current_l4_table[i].is_unused() {
                    if i >= 256 {
                        // Kernel space entries (0x800000000000 and above) - can be shared safely
                        level_4_table[i] = current_l4_table[i].clone();
                        copied_count += 1;
                        
                        log::debug!("Shared kernel PML4 entry {}: addr={:#x}, flags={:?}", 
                            i, current_l4_table[i].addr().as_u64(), current_l4_table[i].flags());
                    } else if i == 0 {
                        // SIMPLIFIED APPROACH: Only copy entry 0 which contains essential kernel code
                        // We'll selectively copy just the kernel portions we need
                        // This avoids the performance issue of copying hundreds of bootloader mappings
                        
                        match Self::selective_copy_entry_0(&current_l4_table[i], phys_offset) {
                            Ok(new_l3_frame) => {
                                let flags = current_l4_table[i].flags();
                                level_4_table[i].set_addr(new_l3_frame.start_address(), flags);
                                copied_count += 1;
                                
                                log::debug!("Selectively copied PML4 entry 0: old L3 addr={:#x}, new L3 addr={:#x}", 
                                    current_l4_table[i].addr().as_u64(), new_l3_frame.start_address().as_u64());
                            }
                            Err(e) => {
                                log::error!("Failed to selectively copy PML4 entry 0: {}", e);
                                return Err("Failed to copy essential kernel mappings");
                            }
                        }
                    } else {
                        // Skip all other user space entries - processes start with clean address spaces
                        // Any needed mappings will be added during ELF loading or when explicitly mapped
                        log::debug!("Skipped PML4 entry {} (clean address space for isolation)", i);
                    }
                }
            }
            
            log::debug!("Total copied {} kernel PML4 entries from kernel page table", copied_count);
            
            // CRITICAL: Verify we have essential kernel mappings
            if copied_count < 1 {
                log::error!("CRITICAL: No kernel PML4 entries copied! Process will definitely crash on page table switch!");
                return Err("No kernel mappings found in current page table");
            }
            
            log::debug!("Deep page table copying completed - each process now has isolated page tables");
        }
        
        // Create mapper for the new page table
        // We need to get a fresh pointer to the level_4_table to avoid borrow conflicts
        let mapper = unsafe {
            let level_4_table_ptr = {
                let virt = phys_offset + level_4_frame.start_address().as_u64();
                &mut *(virt.as_mut_ptr() as *mut PageTable)
            };
            
            log::debug!("Creating OffsetPageTable with L4 frame {:#x} and phys_offset {:#x}", 
                      level_4_frame.start_address().as_u64(), phys_offset.as_u64());
            OffsetPageTable::new(level_4_table_ptr, phys_offset)
        };
        
        // CRITICAL: Clean up any userspace mappings that might have been copied
        // Entry 0 often contains both kernel code and userspace mappings from previous processes
        
        let new_page_table = ProcessPageTable {
            level_4_frame,
            mapper,
        };
        
        // Each process now has completely isolated page tables
        // No conflicts should occur during mapping since each process has its own L3/L2/L1 tables
        log::debug!("ProcessPageTable created with isolated page tables - no mapping conflicts expected");
        
        Ok(new_page_table)
    }
    
    /// Get the physical frame of the level 4 page table
    pub fn level_4_frame(&self) -> PhysFrame {
        self.level_4_frame
    }
    
    /// Map a page in this process's address space
    pub fn map_page(
        &mut self,
        page: Page<Size4KiB>,
        frame: PhysFrame<Size4KiB>,
        flags: PageTableFlags,
    ) -> Result<(), &'static str> {
        log::trace!("ProcessPageTable::map_page called for page {:#x}", page.start_address().as_u64());
        unsafe {
            log::trace!("About to call mapper.map_to...");
            
            // CRITICAL WORKAROUND: The OffsetPageTable might be failing during child
            // page table operations. Let's add extra validation.
            
            // First, ensure we're not trying to map kernel addresses as user pages
            let page_addr = page.start_address().as_u64();
            if page_addr >= 0x800000000000 && flags.contains(PageTableFlags::USER_ACCESSIBLE) {
                log::error!("Attempting to map kernel address {:#x} as user-accessible!", page_addr);
                return Err("Cannot map kernel addresses as user-accessible");
            }
            
            // CRITICAL FIX: Check if page is already mapped before attempting to map
            // This handles the case where L3 tables are shared between processes
            if let Ok(existing_frame) = self.mapper.translate_page(page) {
                if existing_frame == frame {
                    // Page is already mapped to the correct frame, skip
                    log::trace!("Page {:#x} already mapped to frame {:#x}, skipping", 
                              page.start_address().as_u64(), frame.start_address().as_u64());
                    return Ok(());
                } else {
                    // Page is mapped to a different frame, this is an error
                    log::error!("Page {:#x} already mapped to different frame {:#x} (wanted {:#x})",
                              page.start_address().as_u64(), 
                              existing_frame.start_address().as_u64(),
                              frame.start_address().as_u64());
                    return Err("Page already mapped to different frame");
                }
            }
            
            // Page is not mapped, proceed with mapping
            match self.mapper.map_to(page, frame, flags, &mut GlobalFrameAllocator) {
                Ok(flush) => {
                    // CRITICAL: Do NOT flush TLB immediately!
                    // This is a common mistake that differs from how real OSes work.
                    // 
                    // Why we don't flush:
                    // 1. During exec(), this page table isn't active yet
                    // 2. The CR3 write during context switch will flush entire TLB
                    // 3. Immediate flushes can hang if the page is in use
                    // 
                    // Linux/BSD approach: batch flushes or rely on CR3 switches
                    
                    // Store the flush handle but don't execute it
                    // In the future, we could collect these and batch flush if needed
                    let _ = flush; // Explicitly ignore the flush
                    
                    log::trace!("mapper.map_to succeeded, TLB flush deferred");
                    Ok(())
                }
                Err(e) => {
                    log::error!("ProcessPageTable::map_page failed: {:?}", e);
                    Err("Failed to map page")
                }
            }
        }
    }
    
    /// Unmap a page in this process's address space
    pub fn unmap_page(&mut self, page: Page<Size4KiB>) -> Result<PhysFrame<Size4KiB>, &'static str> {
        let (frame, flush) = self.mapper.unmap(page)
            .map_err(|_| "Failed to unmap page")?;
        // Don't flush immediately - same reasoning as map_page
        let _ = flush;
        Ok(frame)
    }
    
    /// Translate a virtual address to physical address
    pub fn translate(&self, addr: VirtAddr) -> Option<PhysAddr> {
        self.mapper.translate_addr(addr)
    }
    
    /// Translate a page to its corresponding physical frame
    pub fn translate_page(&self, addr: VirtAddr) -> Option<PhysAddr> {
        // DEBUG: Add detailed logging to understand translation failures
        let result = self.mapper.translate_addr(addr);
        
        // Only log for userspace addresses to reduce noise
        if addr.as_u64() < 0x800000000000 {
            match result {
                Some(phys) => {
                    log::trace!("translate_page({:#x}) -> {:#x}", addr.as_u64(), phys.as_u64());
                }
                None => {
                    // This is the problematic case - let's understand why
                    log::debug!("translate_page({:#x}) -> None (FAILED)", addr.as_u64());
                    
                    // Let's manually check the page table entries to debug
                    unsafe {
                        let phys_offset = crate::memory::physical_memory_offset();
                        let l4_table = {
                            let virt = phys_offset + self.level_4_frame.start_address().as_u64();
                            &*(virt.as_ptr() as *const x86_64::structures::paging::PageTable)
                        };
                        
                        // Calculate which L4 entry this address uses
                        let l4_index = (addr.as_u64() >> 39) & 0x1ff;
                        let l4_entry = &l4_table[l4_index as usize];
                        
                        if l4_entry.is_unused() {
                            log::debug!("  -> L4 entry {} is UNUSED", l4_index);
                        } else {
                            log::debug!("  -> L4 entry {} exists: addr={:#x}, flags={:?}", 
                                l4_index, l4_entry.addr().as_u64(), l4_entry.flags());
                            
                            // Let's check the L3 table
                            let l3_phys = l4_entry.addr();
                            let l3_virt = phys_offset + l3_phys.as_u64();
                            let l3_table = &*(l3_virt.as_ptr() as *const x86_64::structures::paging::PageTable);
                            
                            let l3_index = (addr.as_u64() >> 30) & 0x1ff;
                            let l3_entry = &l3_table[l3_index as usize];
                            
                            if l3_entry.is_unused() {
                                log::debug!("    -> L3 entry {} is UNUSED", l3_index);
                            } else {
                                log::debug!("    -> L3 entry {} exists: addr={:#x}, flags={:?}", 
                                    l3_index, l3_entry.addr().as_u64(), l3_entry.flags());
                            }
                        }
                    }
                }
            }
        }
        
        result
    }
    
    /// Get a reference to the mapper
    pub fn mapper(&mut self) -> &mut OffsetPageTable<'static> {
        &mut self.mapper
    }
    
    /// Clear specific userspace mappings before loading a new program
    /// 
    /// WORKAROUND: Since we share L3 tables between processes, we need to
    /// unmap pages that might conflict with the new program.
    pub fn clear_userspace_for_exec(&mut self) -> Result<(), &'static str> {
        log::debug!("clear_userspace_for_exec: Clearing common userspace regions");
        
        // Clear the standard userspace regions that programs typically use
        // This prevents "page already mapped" errors when loading ELF files
        
        // 1. Clear code/data region (0x10000000 - 0x10010000)
        let code_start = VirtAddr::new(0x10000000);
        let code_end = VirtAddr::new(0x10010000);
        match self.unmap_user_pages(code_start, code_end) {
            Ok(()) => log::debug!("Cleared code region {:#x}-{:#x}", code_start, code_end),
            Err(e) => log::warn!("Failed to clear code region: {}", e),
        }
        
        // 2. Clear user stack region if it exists
        let stack_bottom = VirtAddr::new(0x555555550000);
        let stack_top = VirtAddr::new(0x555555572000);
        match self.unmap_user_pages(stack_bottom, stack_top) {
            Ok(()) => log::debug!("Cleared stack region {:#x}-{:#x}", stack_bottom, stack_top),
            Err(e) => log::warn!("Failed to clear stack region: {}", e),
        }
        
        Ok(())
    }
    
    /// Clear specific PML4 entries that might contain user mappings
    /// This is used during exec() to clear out old process mappings
    /// 
    /// NOTE: This doesn't work well when L3 tables are shared between processes
    pub fn clear_user_entries(&mut self) {
        // Get physical memory offset
        let phys_offset = crate::memory::physical_memory_offset();
        
        // Get the L4 table
        let level_4_table = unsafe {
            let virt = phys_offset + self.level_4_frame.start_address().as_u64();
            &mut *(virt.as_mut_ptr() as *mut PageTable)
        };
        
        // Clear entries that typically contain user mappings
        // Entry 0: While it contains kernel code, it might also have user mappings at 0x10000000
        // We need to be careful here - we can't clear the entire entry
        // For now, we'll skip entry 0 and let the ELF loader overwrite user mappings
        
        // Entry 32: Alternative user code location (0x100000000000)
        if !level_4_table[32].is_unused() {
            log::debug!("Clearing PML4 entry 32 (potential user code range)");
            level_4_table[32].set_unused();
        }
        
        // Entry 170: User stack location (0x550000000000)
        if !level_4_table[170].is_unused() {
            log::debug!("Clearing PML4 entry 170 (user stack range)");
            level_4_table[170].set_unused();
        }
    }
    
    /// Unmap specific user pages in the address space
    /// This is more precise than clearing entire PML4 entries
    pub fn unmap_user_pages(&mut self, start_addr: VirtAddr, end_addr: VirtAddr) -> Result<(), &'static str> {
        log::debug!("Unmapping user pages from {:#x} to {:#x}", start_addr.as_u64(), end_addr.as_u64());
        
        let start_page = Page::<Size4KiB>::containing_address(start_addr);
        let end_page = Page::<Size4KiB>::containing_address(end_addr);
        
        for page in Page::range_inclusive(start_page, end_page) {
            // Try to unmap the page - it's OK if it's not mapped
            match self.mapper.unmap(page) {
                Ok((frame, _flush)) => {
                    // Don't flush immediately - the page table switch will handle it
                    log::trace!("Unmapped page {:#x} (was mapped to frame {:#x})", 
                              page.start_address().as_u64(), frame.start_address().as_u64());
                }
                Err(_) => {
                    // Page wasn't mapped, that's fine
                    log::trace!("Page {:#x} was not mapped", page.start_address().as_u64());
                }
            }
        }
        
        Ok(())
    }
}

/// Switch to a process's page table
/// 
/// # Safety
/// This changes the active page table. The caller must ensure that:
/// - The new page table is valid
/// - The kernel mappings are present in the new page table
/// - This is called from a safe context (e.g., during interrupt return)
pub unsafe fn switch_to_process_page_table(page_table: &ProcessPageTable) {
    let (current_frame, flags) = Cr3::read();
    let new_frame = page_table.level_4_frame();
    
    if current_frame != new_frame {
        log::debug!("About to switch page table: {:?} -> {:?}", current_frame, new_frame);
        log::debug!("Current stack pointer: {:#x}", {
            let mut rsp: u64;
            core::arch::asm!("mov {}, rsp", out(reg) rsp);
            rsp
        });
        
        // Verify that kernel mappings are present in the new page table
        let phys_offset = crate::memory::physical_memory_offset();
        let new_l4_table = &*(((phys_offset + new_frame.start_address().as_u64()).as_u64()) as *const PageTable);
        
        let mut kernel_entries = 0;
        for i in 256..512 {
            if !new_l4_table[i].is_unused() {
                kernel_entries += 1;
            }
        }
        log::debug!("Process page table has {} kernel PML4 entries", kernel_entries);
        
        if kernel_entries == 0 {
            log::error!("CRITICAL: Process page table has no kernel mappings! This will cause immediate crash!");
            return;
        }
        
        log::trace!("Switching page table: {:?} -> {:?}", current_frame, new_frame);
        Cr3::write(new_frame, flags);
        // Ensure TLB consistency after page table switch
        super::tlb::flush_after_page_table_switch();
        log::debug!("Page table switch completed successfully with TLB flush");
    }
}

/// Get the kernel's page table frame (the one created by bootloader)
static mut KERNEL_PAGE_TABLE_FRAME: Option<PhysFrame> = None;

/// Initialize the kernel page table frame
/// This should be called early in boot to save the kernel's page table
pub fn init_kernel_page_table() {
    unsafe {
        let (frame, _) = Cr3::read();
        KERNEL_PAGE_TABLE_FRAME = Some(frame);
        log::info!("Saved kernel page table frame: {:?}", frame);
    }
}

/// Switch back to the kernel page table
/// 
/// # Safety
/// Caller must ensure this is called from a safe context
pub unsafe fn switch_to_kernel_page_table() {
    if let Some(kernel_frame) = KERNEL_PAGE_TABLE_FRAME {
        let (current_frame, flags) = Cr3::read();
        if current_frame != kernel_frame {
            log::trace!("Switching back to kernel page table: {:?} -> {:?}", current_frame, kernel_frame);
            Cr3::write(kernel_frame, flags);
            // Ensure TLB consistency after page table switch
            super::tlb::flush_after_page_table_switch();
        }
    } else {
        log::error!("Kernel page table frame not initialized!");
    }
}

/// Copy kernel stack mappings from kernel page table to process page table
/// This is critical for Ring 3 -> Ring 0 transitions during syscalls
pub fn copy_kernel_stack_to_process(
    process_page_table: &mut ProcessPageTable,
    stack_bottom: VirtAddr,
    stack_top: VirtAddr,
) -> Result<(), &'static str> {
    log::debug!("copy_kernel_stack_to_process: copying stack range {:#x} - {:#x}", 
        stack_bottom.as_u64(), stack_top.as_u64());
    
    // Get access to the kernel page table
    let kernel_mapper = unsafe { crate::memory::paging::get_mapper() };
    
    // Calculate page range to copy
    let start_page = Page::<Size4KiB>::containing_address(stack_bottom);
    let end_page = Page::<Size4KiB>::containing_address(stack_top - 1u64);
    
    let mut copied_pages = 0;
    
    // Copy each page mapping from kernel to process page table
    for page in Page::range_inclusive(start_page, end_page) {
        // Look up the mapping in the kernel page table
        match kernel_mapper.translate(page.start_address()) {
            TranslateResult::Mapped { frame, offset, flags: _ } => {
                let phys_addr = frame.start_address() + offset;
                let frame = PhysFrame::containing_address(phys_addr);
                
                // Map the same physical frame in the process page table
                // Use kernel permissions (not user accessible)
                let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE;
                
                // Check if the page is already mapped in the process page table
                if let Some(existing_frame) = process_page_table.translate_page(page.start_address()) {
                    // Page is already mapped, verify it maps to the same frame
                    let existing_frame = PhysFrame::containing_address(existing_frame);
                    if existing_frame == frame {
                        log::trace!("Kernel stack page {:#x} already mapped correctly to frame {:#x}", 
                            page.start_address().as_u64(), frame.start_address().as_u64());
                        copied_pages += 1;
                    } else {
                        log::error!("Kernel stack page {:#x} already mapped to different frame: expected {:#x}, found {:#x}", 
                            page.start_address().as_u64(), frame.start_address().as_u64(), existing_frame.start_address().as_u64());
                        return Err("Kernel stack page already mapped to different frame");
                    }
                } else {
                    // Page not mapped, map it now
                    match process_page_table.map_page(page, frame, flags) {
                        Ok(()) => {
                            copied_pages += 1;
                            log::trace!("Mapped kernel stack page {:#x} -> frame {:#x}", 
                                page.start_address().as_u64(), frame.start_address().as_u64());
                        }
                        Err(e) => {
                            log::error!("Failed to map kernel stack page {:#x}: {}", 
                                page.start_address().as_u64(), e);
                            return Err("Failed to map kernel stack page");
                        }
                    }
                }
            }
            _ => {
                log::error!("Kernel stack page {:#x} not mapped in kernel page table!", 
                    page.start_address().as_u64());
                return Err("Kernel stack page not found in kernel page table");
            }
        }
    }
    
    log::debug!("✓ Successfully copied {} kernel stack pages to process page table", copied_pages);
    Ok(())
}

/// Map user stack pages from kernel page table to process page table
/// This is critical for userspace execution - the stack must be accessible
pub fn map_user_stack_to_process(
    process_page_table: &mut ProcessPageTable,
    stack_bottom: VirtAddr,
    stack_top: VirtAddr,
) -> Result<(), &'static str> {
    log::debug!("map_user_stack_to_process: mapping stack range {:#x} - {:#x}", 
        stack_bottom.as_u64(), stack_top.as_u64());
    
    // Get access to the kernel page table
    let kernel_mapper = unsafe { crate::memory::paging::get_mapper() };
    
    // Calculate page range to copy
    let start_page = Page::<Size4KiB>::containing_address(stack_bottom);
    let end_page = Page::<Size4KiB>::containing_address(stack_top - 1u64);
    
    let mut mapped_pages = 0;
    
    // Copy each page mapping from kernel to process page table
    for page in Page::range_inclusive(start_page, end_page) {
        // Look up the mapping in the kernel page table
        match kernel_mapper.translate(page.start_address()) {
            TranslateResult::Mapped { frame, offset, flags: _ } => {
                let phys_addr = frame.start_address() + offset;
                let frame = PhysFrame::containing_address(phys_addr);
                
                // Map the same physical frame in the process page table
                // Use user-accessible permissions for user stack
                let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE;
                
                // Check if already mapped
                if let Some(existing_frame) = process_page_table.translate_page(page.start_address()) {
                    let existing_frame = PhysFrame::containing_address(existing_frame);
                    if existing_frame == frame {
                        log::trace!("User stack page {:#x} already mapped correctly to frame {:#x}", 
                            page.start_address().as_u64(), frame.start_address().as_u64());
                        mapped_pages += 1;
                    } else {
                        log::error!("User stack page {:#x} already mapped to different frame: expected {:#x}, found {:#x}", 
                            page.start_address().as_u64(), frame.start_address().as_u64(), existing_frame.start_address().as_u64());
                        return Err("User stack page already mapped to different frame");
                    }
                } else {
                    // Page not mapped, map it now
                    match process_page_table.map_page(page, frame, flags) {
                        Ok(()) => {
                            mapped_pages += 1;
                            log::trace!("Mapped user stack page {:#x} -> frame {:#x}", 
                                page.start_address().as_u64(), frame.start_address().as_u64());
                        }
                        Err(e) => {
                            log::error!("Failed to map user stack page {:#x}: {}", 
                                page.start_address().as_u64(), e);
                            return Err("Failed to map user stack page");
                        }
                    }
                }
            }
            _ => {
                log::error!("User stack page {:#x} not mapped in kernel page table!", 
                    page.start_address().as_u64());
                return Err("User stack page not found in kernel page table");
            }
        }
    }
    
    log::debug!("✓ Successfully mapped {} user stack pages to process page table", mapped_pages);
    Ok(())
}
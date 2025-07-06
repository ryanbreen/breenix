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
    /// Create a new page table for a process
    /// 
    /// This creates a new level 4 page table with kernel mappings copied
    /// from the current page table.
    pub fn new() -> Result<Self, &'static str> {
        // Allocate a frame for the new level 4 page table
        let level_4_frame = allocate_frame()
            .ok_or("Failed to allocate frame for page table")?;
        
        // Get physical memory offset
        let phys_offset = crate::memory::physical_memory_offset();
        
        // Map the new page table frame
        let level_4_table = unsafe {
            let virt = phys_offset + level_4_frame.start_address().as_u64();
            &mut *(virt.as_mut_ptr() as *mut PageTable)
        };
        
        // Clear the new page table
        level_4_table.zero();
        
        // Copy kernel mappings from the current page table
        // This ensures the kernel is always mapped in all processes
        unsafe {
            let current_l4_table = {
                let (frame, _) = Cr3::read();
                let virt = phys_offset + frame.start_address().as_u64();
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
            
            // Copy ALL entries from the kernel page table
            // This ensures we don't miss any critical mappings
            for i in 0..512 {
                if !current_l4_table[i].is_unused() {
                    level_4_table[i] = current_l4_table[i].clone();
                    copied_count += 1;
                    
                    // Only log the first few to avoid spam
                    if copied_count <= 10 || i >= 256 {
                        log::debug!("Copied PML4 entry {}: addr={:#x}, flags={:?}", 
                            i, current_l4_table[i].addr().as_u64(), current_l4_table[i].flags());
                    }
                }
            }
            
            log::debug!("Total copied {} kernel PML4 entries from kernel page table", copied_count);
            
            // CRITICAL: Verify we have essential kernel mappings
            if copied_count < 1 {
                log::error!("CRITICAL: No kernel PML4 entries copied! Process will definitely crash on page table switch!");
                return Err("No kernel mappings found in current page table");
            }
        }
        
        // Create mapper for the new page table
        let mapper = unsafe {
            OffsetPageTable::new(level_4_table, phys_offset)
        };
        
        Ok(ProcessPageTable {
            level_4_frame,
            mapper,
        })
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
        self.mapper.translate_addr(addr)
    }
    
    /// Get a reference to the mapper
    pub fn mapper(&mut self) -> &mut OffsetPageTable<'static> {
        &mut self.mapper
    }
    
    /// Clear specific PML4 entries that might contain user mappings
    /// This is used during exec() to clear out old process mappings
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
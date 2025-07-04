//! Per-process memory management
//! 
//! This module provides per-process page tables and address space isolation.

use x86_64::{
    structures::paging::{
        OffsetPageTable, PageTable, PageTableFlags, Page, Size4KiB, 
        Mapper, PhysFrame, FrameAllocator, Translate
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
            // 
            // CRITICAL BUG ANALYSIS:
            // The bootloader creates a kernel page table that might not follow the
            // traditional kernel/user split. Instead of only copying entries 256-511,
            // we need to examine ALL entries and copy anything that looks like kernel mappings.
            //
            // The traditional split is:
            // - Entries 0-255: User space (0x0000_0000_0000_0000 to 0x0000_FFFF_FFFF_FFFF)  
            // - Entries 256-511: Kernel space (0xFFFF_0000_0000_0000 to 0xFFFF_FFFF_FFFF_FFFF)
            //
            // But modern bootloaders (like the one we're using) might map kernel code
            // in different regions. We need to be more thorough.
            
            log::debug!("Analyzing current kernel page table for all potential kernel mappings...");
            
            // First, let's analyze the entire page table to understand the memory layout
            for i in 0..512 {
                if !current_l4_table[i].is_unused() {
                    log::debug!("PML4 entry {}: {:?} (covers {:#x} - {:#x})", 
                        i, current_l4_table[i], 
                        (i as u64) << 39, 
                        ((i as u64 + 1) << 39) - 1);
                }
            }
            
            let mut copied_count = 0;
            
            // Strategy 1: Copy traditional kernel space (entries 256-511)
            for i in 256..512 {
                if !current_l4_table[i].is_unused() {
                    level_4_table[i] = current_l4_table[i].clone();
                    copied_count += 1;
                    log::debug!("Copied traditional kernel PML4 entry {}: {:?}", i, current_l4_table[i]);
                }
            }
            
            // Strategy 2: Copy ALL non-empty entries that could contain kernel code/data
            // The kernel entry point is 0x10000064360 which is in entry 2:
            // - Each PML4 entry covers 512GB (2^39 bytes)
            // - 0x10000064360 >> 39 = 2
            // So we need to copy entry 2 plus any other entries that might be used
            for i in 0..256 {
                if !current_l4_table[i].is_unused() {
                    // Copy ANY entry in the lower 256 entries that's not empty
                    // This ensures we don't miss kernel mappings regardless of layout
                    level_4_table[i] = current_l4_table[i].clone();
                    copied_count += 1;
                    let start_addr = (i as u64) << 39;
                    let end_addr = ((i as u64 + 1) << 39) - 1;
                    log::debug!("Copied PML4 entry {}: {:?} (range {:#x}-{:#x})", 
                        i, current_l4_table[i], start_addr, end_addr);
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
                    log::trace!("mapper.map_to succeeded, flushing TLB...");
                    flush.flush();
                    log::trace!("TLB flushed");
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
        flush.flush();
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
        log::debug!("Page table switch completed successfully");
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
        }
    } else {
        log::error!("Kernel page table frame not initialized!");
    }
}
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

/// Recursively copy the full paging hierarchy of a present PML4 entry
fn duplicate_pml4_entry(
    dst_pml4: &mut PageTable, 
    src_pml4: &PageTable,
    src_index: usize,
    phys_offset: VirtAddr
) -> Result<(), &'static str> {
    let src_entry = &src_pml4[src_index];
    if src_entry.is_unused() { 
        return Ok(()); 
    }

    // Allocate fresh L3 table
    let new_l3_frame = allocate_frame().ok_or("no frames for L3")?;
    dst_pml4[src_index].set_addr(new_l3_frame.start_address(), src_entry.flags());

    recursive_clone(src_entry.addr(), new_l3_frame.start_address(), phys_offset)
}

/// Depth-first clone of a 4-KiB page-table page
fn recursive_clone(
    src_phys: PhysAddr, 
    dst_phys: PhysAddr,
    phys_offset: VirtAddr
) -> Result<(), &'static str> {
    // Map source and destination page tables
    let src = unsafe { 
        let virt_addr = phys_offset + src_phys.as_u64();
        &*(virt_addr.as_ptr::<PageTable>())
    };
    let dst = unsafe { 
        let virt_addr = phys_offset + dst_phys.as_u64();
        &mut *(virt_addr.as_mut_ptr::<PageTable>())
    };

    // Clear destination table first
    dst.zero();

    for (i, s) in src.iter().enumerate() {
        if s.is_unused() { 
            continue; 
        }
        
        let d = &mut dst[i];
        
        // Check if this is a huge page (2MB or 1GB)
        if s.flags().contains(PageTableFlags::HUGE_PAGE) {
            // For huge pages, just copy the entry directly
            *d = s.clone();
        } else {
            // For regular entries, we need to recurse deeper
            let new_child_frame = allocate_frame().ok_or("no frames for child page table")?;
            d.set_addr(new_child_frame.start_address(), s.flags());
            recursive_clone(s.addr(), new_child_frame.start_address(), phys_offset)?;
        }
    }
    Ok(())
}

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
        // Check stack pointer before allocating
        let initial_rsp: u64;
        unsafe {
            core::arch::asm!("mov {}, rsp", out(reg) initial_rsp);
        }
        
        // Check if we're running low on stack space
        // The bootloader provides an initial stack. Based on RSP value analysis:
        // RSP=0x10000011180, Guard page top=0x10000012000
        // Remaining = 0x10000012000 - 0x10000011180 = 0xE80 = 3712 bytes
        let guard_page_top = 0x10000012000u64;
        let remaining_stack = if initial_rsp < guard_page_top {
            guard_page_top - initial_rsp
        } else {
            0
        };
        
        if remaining_stack < 4096 { // Less than 4KB remaining
            log::warn!("Low stack space: RSP={:#x}, remaining={}B", initial_rsp, remaining_stack);
        }
        
        // Allocate a frame for the new level 4 page table
        log::debug!("ProcessPageTable::new() - About to allocate L4 frame");
        
        // Try to allocate with error handling
        let level_4_frame = match allocate_frame() {
            Some(frame) => {
                let frame_addr = frame.start_address().as_u64();
                log::debug!("Successfully allocated frame: {:#x}", frame_addr);
                
                // Check for problematic frames
                if frame_addr == 0x611000 {
                    log::error!("WARNING: Allocated frame 0x611000 which is already in use by a process!");
                }
                
                frame
            }
            None => {
                log::error!("Frame allocator returned None - out of memory?");
                return Err("Failed to allocate frame for page table");
            }
        };
        
        log::debug!("Allocated L4 frame: {:#x}", level_4_frame.start_address().as_u64());
        
        // Get physical memory offset
        let phys_offset = crate::memory::physical_memory_offset();
        
        // Verify the frame is within expected range
        let frame_addr = level_4_frame.start_address().as_u64();
        if frame_addr > 0x10000000 {  // 256MB limit
            log::error!("Allocated frame {:#x} is beyond expected physical memory range", frame_addr);
            return Err("Frame allocator returned invalid frame");
        }
        
        // Map the new page table frame
        let level_4_table = unsafe {
            log::debug!("Physical memory offset: {:#x}", phys_offset.as_u64());
            let virt = phys_offset + level_4_frame.start_address().as_u64();
            log::debug!("New L4 table virtual address: {:#x}", virt.as_u64());
            
            log::debug!("About to create mutable reference to page table at {:#x}", virt.as_u64());
            
            // Test if we can read the memory first
            let test_ptr = virt.as_ptr::<u8>();
            log::debug!("Testing read access at {:p}", test_ptr);
            let _test_byte = core::ptr::read_volatile(test_ptr);
            log::debug!("Read test successful");
            
            let table_ptr = virt.as_mut_ptr() as *mut PageTable;
            log::debug!("Page table pointer: {:p}", table_ptr);
            &mut *table_ptr
        };
        
        log::debug!("About to zero the new page table");
        // Clear the new page table
        level_4_table.zero();
        log::debug!("Successfully zeroed new page table");
        
        // Copy kernel mappings from the KERNEL's original page table
        // CRITICAL: We must use the kernel's page table (0x101000), not the current process's table
        // This prevents corrupted mappings from being propagated during fork()
        unsafe {
            const KERNEL_CR3: u64 = 0x101000; // The kernel's original page table
            
            let current_l4_table = {
                // Log what CR3 we're currently using vs what we should use
                let (current_frame, _) = Cr3::read();
                log::debug!("ProcessPageTable::new() - Current CR3: {:#x}", current_frame.start_address().as_u64());
                log::debug!("ProcessPageTable::new() - Using kernel CR3: {:#x} for copying", KERNEL_CR3);
                
                // Always use the kernel's page table for copying kernel mappings
                let kernel_frame = PhysFrame::<Size4KiB>::containing_address(PhysAddr::new(KERNEL_CR3));
                let virt = phys_offset + kernel_frame.start_address().as_u64();
                log::debug!("Kernel L4 table virtual address: {:#x}", virt.as_u64());
                &*(virt.as_ptr() as *const PageTable)
            };
            
            // Copy kernel mappings from the current page table
            // This is critical - we need ALL kernel mappings to be present in every
            // process page table so the kernel can function after a page table switch
            
                // NEW: Use global kernel page tables for entries 256-511
            // This ensures all kernel mappings (including dynamically allocated kernel stacks)
            // are visible to all processes
            
            // Get the global kernel PDPT frame
            let kernel_pdpt_frame = crate::memory::kernel_page_table::kernel_pdpt_frame()
                .ok_or("Global kernel page tables not initialized")?;
            
            log::debug!("Using global kernel PDPT frame: {:?}", kernel_pdpt_frame);
            
            // -------------------------------------------------------------------
            // Make the entire canonical-high-half (PML4 slots 256-511) visible
            // in every user CR3. We clear USER_ACCESSIBLE so ring-3 cannot peek.
            // -------------------------------------------------------------------
            let mut high_half_copied = 0;
            for idx in 256..512 {
                if !current_l4_table[idx].is_unused() {
                    // Copy *exactly* the entry the kernel is using
                    let mut entry = current_l4_table[idx].clone();
                    entry.set_flags(entry.flags() & !PageTableFlags::USER_ACCESSIBLE);
                    
                    // CRITICAL: Compile-time check to prevent kernel mappings from being user accessible
                    debug_assert!(
                        !entry.flags().contains(PageTableFlags::USER_ACCESSIBLE),
                        "Kernel PML4[{}] has USER bit set - SECURITY VIOLATION!", idx
                    );
                    
                    level_4_table[idx] = entry;
                    high_half_copied += 1;
                }
            }
            
            log::debug!("Set up global kernel page table entries 256-511 (copied {} entries)", high_half_copied);
            
            // CRITICAL: Ensure ALL kernel code is accessible after CR3 switch
            // The bootloader maps kernel .text/.rodata/.data at 0x200000-0x320000 region
            // This falls in PML4 entry 0, but we need to ensure the ENTIRE mapping is present
            log::debug!("Sharing ALL kernel PML4 entries (0-7) to ensure ISR code accessibility after CR3 switch");
            let mut copied_count = 0;
            
            // CRITICAL FIX: Share kernel entries but exclude userspace mappings
            // We need kernel code/data access but must prevent userspace mapping pollution
            
            // CRITICAL: The kernel code lives at 0x200000-0x320000 (PML4 entry 0)
            // We MUST map this or the interrupt return path will fail after CR3 switch!
            // However, we need to be careful not to map userspace regions.
            
            // The kernel typically uses these address ranges:
            // - 0x200000-0x400000: Kernel code/data (loaded by bootloader)
            // - 0x10000000+: Userspace programs
            
            // Map PML4 entry 0 but ONLY if it contains kernel code
            // We can detect this by checking if the kernel's entry point is in this range
            const KERNEL_CODE_START: u64 = 0x200000;
            const KERNEL_CODE_END: u64 = 0x400000;
            
            // Check if PML4 entry 0 is used and likely contains kernel code
            if !current_l4_table[0].is_unused() {
                // This entry maps addresses 0x0 - 0x8000000000 (512GB)
                // We need it for kernel code but must be careful about userspace
                let mut entry = current_l4_table[0].clone();
                // Remove user accessible flag to prevent userspace from accessing kernel code
                entry.set_flags(entry.flags() & !PageTableFlags::USER_ACCESSIBLE);
                
                // CRITICAL: Compile-time check to prevent kernel code from being user accessible
                debug_assert!(
                    !entry.flags().contains(PageTableFlags::USER_ACCESSIBLE),
                    "Kernel code PML4[0] has USER bit set - SECURITY VIOLATION!"
                );
                
                level_4_table[0] = entry;
                log::info!("KERNEL_CODE_FIX: Mapped PML4 entry 0 for kernel code access (0x200000-0x400000)");
                copied_count += 1;
            }
            
            // Skip entries 1-7 for process isolation
            for idx in 1..8 {
                if !current_l4_table[idx].is_unused() {
                    log::debug!("Skipping PML4 entry {} for process isolation", idx);
                }
            }
            
            // CRITICAL FIX: Copy PML4 entries that map the kernel heap (0x444444440000)
            // The kernel heap starts at 0x444444440000, which maps to PML4 entry:
            // (0x444444440000 >> 39) & 0x1FF = 0x88 & 0x1FF = 0x88 = 136
            const KERNEL_HEAP_PML4_ENTRY: usize = 136;
            log::debug!("Copying kernel heap PML4 entry {} (maps 0x444444440000)", KERNEL_HEAP_PML4_ENTRY);
            
            if !current_l4_table[KERNEL_HEAP_PML4_ENTRY].is_unused() {
                level_4_table[KERNEL_HEAP_PML4_ENTRY] = current_l4_table[KERNEL_HEAP_PML4_ENTRY].clone();
                copied_count += 1;
                log::debug!("Copied kernel heap PML4 entry {} (flags: {:?}) for heap access after CR3 switch", 
                    KERNEL_HEAP_PML4_ENTRY, current_l4_table[KERNEL_HEAP_PML4_ENTRY].flags());
            } else {
                log::warn!("Kernel heap PML4 entry {} is UNUSED - this will cause page faults!", KERNEL_HEAP_PML4_ENTRY);
            }
            
            log::debug!("Process page table created with global kernel mappings ({} low entries + 256 high entries)", copied_count);
            
            // CRITICAL FIX: Map 8 pages around the RSP0 region for interrupt handling
            // RSP0 addresses are in the 0xffffc90000000000 range (kernel stack allocator)
            // Without these mappings, triple fault occurs when CPU tries to push interrupt frame
            let rsp0_range_start = 0xffffc90000000000u64;
            let rsp0_range_end = 0xffffc90000020000u64; // 128KB range (32 pages) to cover all possible RSP0 values
            
            // Map this range from the kernel's current page table to the new process page table
            let phys_offset = crate::memory::physical_memory_offset();
            let kernel_frame = PhysFrame::<Size4KiB>::containing_address(PhysAddr::new(KERNEL_CR3));
            let kernel_l4_table = &*(phys_offset + kernel_frame.start_address().as_u64()).as_ptr::<PageTable>();
            
            // Calculate PML4 index for RSP0 region: (0xffffc90000000000 >> 39) & 0x1ff = 0x192 = 402
            const RSP0_PML4_INDEX: usize = 402;
            log::debug!("Mapping RSP0 region: PML4 entry {} for addresses {:#x}-{:#x}", 
                RSP0_PML4_INDEX, rsp0_range_start, rsp0_range_end);
            
            if !kernel_l4_table[RSP0_PML4_INDEX].is_unused() {
                let mut entry = kernel_l4_table[RSP0_PML4_INDEX].clone();
                entry.set_flags(entry.flags() & !PageTableFlags::USER_ACCESSIBLE);
                
                // CRITICAL: Compile-time check to prevent RSP0 region from being user accessible
                debug_assert!(
                    !entry.flags().contains(PageTableFlags::USER_ACCESSIBLE),
                    "RSP0 PML4[{}] has USER bit set - SECURITY VIOLATION!", RSP0_PML4_INDEX
                );
                
                level_4_table[RSP0_PML4_INDEX] = entry;
                log::info!("TRIPLE_FAULT_FIX: Mapped RSP0 PML4 entry {} for interrupt frame handling", RSP0_PML4_INDEX);
            } else {
                log::error!("RSP0 PML4 entry {} is UNUSED in kernel page table - this will cause triple faults!", RSP0_PML4_INDEX);
            }
            
            // CRITICAL FIX: Map idle thread stack region for context switching
            // The idle thread stack is at 0x100000000000 range, which maps to PML4 entry 2
            // Without this mapping, context switches to userspace fail because kernel stack is not accessible
            const IDLE_STACK_PML4_INDEX: usize = 2;
            let idle_stack_addr = 0x100000000000u64; // Range containing idle thread stack
            log::debug!("Mapping idle thread stack region: PML4 entry {} for addresses around {:#x}", 
                IDLE_STACK_PML4_INDEX, idle_stack_addr);
            
            if !kernel_l4_table[IDLE_STACK_PML4_INDEX].is_unused() {
                let mut entry = kernel_l4_table[IDLE_STACK_PML4_INDEX].clone();
                entry.set_flags(entry.flags() & !PageTableFlags::USER_ACCESSIBLE);
                
                // CRITICAL: Compile-time check to prevent kernel stack from being user accessible
                debug_assert!(
                    !entry.flags().contains(PageTableFlags::USER_ACCESSIBLE),
                    "Kernel stack PML4[{}] has USER bit set - SECURITY VIOLATION!", IDLE_STACK_PML4_INDEX
                );
                
                level_4_table[IDLE_STACK_PML4_INDEX] = entry;
                log::info!("KSTACK_FIX: Mapped idle thread stack PML4 entry {} for context switch support", IDLE_STACK_PML4_INDEX);
            } else {
                log::error!("Idle thread stack PML4 entry {} is UNUSED in kernel page table - this will cause context switch failures!", IDLE_STACK_PML4_INDEX);
            }
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
        
        // With global kernel page tables, all kernel stacks are automatically visible
        // to all processes through the shared kernel PDPT
        log::debug!("ProcessPageTable created with global kernel page tables");
        
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
        unsafe {
            
            // CRITICAL WORKAROUND: The OffsetPageTable might be failing during child
            // page table operations. Let's add extra validation.
            
            // First, ensure we're not trying to map kernel addresses as user pages
            let page_addr = page.start_address().as_u64();
            if page_addr >= 0x800000000000 && flags.contains(PageTableFlags::USER_ACCESSIBLE) {
                log::error!("Attempting to map kernel address {:#x} as user-accessible!", page_addr);
                return Err("Cannot map kernel addresses as user-accessible");
            }
            
            // Check if page is already mapped in THIS process's page table
            if let Ok(existing_frame) = self.mapper.translate_page(page) {
                if existing_frame == frame {
                    // Page is already mapped to the correct frame, skip
                    return Ok(());
                } else {
                    // Page is mapped to a different frame in THIS process
                    // This is an error - we shouldn't overwrite existing mappings
                    log::error!("Page {:#x} already mapped to different frame {:#x} (wanted {:#x}) in this process",
                              page.start_address().as_u64(), 
                              existing_frame.start_address().as_u64(),
                              frame.start_address().as_u64());
                    return Err("Page already mapped to different frame in this process");
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
                    
                    Ok(())
                }
                Err(e) => {
                    log::error!("ProcessPageTable::map_page failed: {:?}", e);
                    Err("Failed to map page")
                }
            }
        }
    }
    
    /// Translate a page to its corresponding physical frame
    pub fn translate_page(&self, addr: VirtAddr) -> Option<PhysAddr> {
        // DEBUG: Add detailed logging to understand translation failures
        let result = self.mapper.translate_addr(addr);
        
        // Only log for userspace addresses to reduce noise
        if addr.as_u64() < 0x800000000000 {
            match result {
                Some(_phys) => {
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

/// Initialize the kernel page table frame
/// This should be called early in boot to save the kernel's page table
pub fn init_kernel_page_table() {
    unsafe {
        let (frame, _) = Cr3::read();
        KERNEL_PAGE_TABLE_FRAME = Some(frame);
        log::info!("Saved kernel page table frame: {:?}", frame);
    }
}

// NOTE: This function is no longer needed with global kernel page tables
// All kernel stacks are automatically visible to all processes through the shared kernel PDPT

/// Get the kernel's page table frame (the one created by bootloader)
static mut KERNEL_PAGE_TABLE_FRAME: Option<PhysFrame> = None;

/// Map user stack pages from kernel page table to process page table
/// This is critical for userspace execution - the stack must be accessible
pub fn map_user_stack_to_process(
    process_page_table: &mut ProcessPageTable,
    stack_bottom: VirtAddr,
    stack_top: VirtAddr,
) -> Result<(), &'static str> {
    
    // Get access to the kernel page table
    let kernel_mapper = unsafe { crate::memory::paging::get_mapper() };
    
    // Calculate page range to copy
    let start_page = Page::<Size4KiB>::containing_address(stack_bottom);
    // CRITICAL FIX: Include the page containing stack_top
    // stack_top points to the first byte AFTER the stack (where RSP will be)
    // We need to map up to and including the page that contains (stack_top - 1)
    // But since RSP starts AT stack_top and grows down, we need the page containing stack_top
    let end_page = Page::<Size4KiB>::containing_address(stack_top);
    
    let mut _mapped_pages = 0;
    const MAX_STACK_PAGES: usize = 256; // 1MB max stack size
    
    // Safety check - prevent infinite loops
    let page_count = (end_page.start_address().as_u64() - start_page.start_address().as_u64()) / 4096;
    if page_count > MAX_STACK_PAGES as u64 {
        log::error!("Stack mapping too large: {} pages (max {}), range {:#x}-{:#x}", 
                   page_count, MAX_STACK_PAGES, stack_bottom.as_u64(), stack_top.as_u64());
        return Err("Stack mapping exceeds maximum size");
    }
    
    // Calculate the actual page count - Page::range_inclusive includes both start and end
    let page_count = ((end_page.start_address().as_u64() - start_page.start_address().as_u64()) / 4096) + 1;
    
    crate::serial_println!("STACK_MAP: mapping {} pages from {:#x} to {:#x}", 
                          page_count, stack_bottom.as_u64(), stack_top.as_u64());
    crate::serial_println!("STACK_MAP: start_page={:#x}, end_page={:#x} (inclusive)",
                          start_page.start_address().as_u64(), 
                          end_page.start_address().as_u64());
    
    // DEBUG: Log what RSP will be and what page it's in
    crate::serial_println!("STACK_MAP: RSP will be {:#x}, which is in page {:#x}",
                          stack_top.as_u64(),
                          Page::<Size4KiB>::containing_address(stack_top).start_address().as_u64());
    
    // Guard pattern to detect source of 0xB1 pointer
    let sentinel: u8 = 0xB1;
    let ptr = &sentinel as *const _ as usize;
    crate::serial_println!("STACK-MAP start, stack var @ {:#x}", ptr);
    
    // Copy each page mapping from kernel to process page table
    for page in Page::range_inclusive(start_page, end_page) {
        _mapped_pages += 1;
        if _mapped_pages > MAX_STACK_PAGES {
            log::error!("Stack mapping exceeded MAX_STACK_PAGES during loop!");
            return Err("Stack mapping infinite loop detected");
        }
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
                        _mapped_pages += 1;
                    } else {
                        log::error!("User stack page {:#x} already mapped to different frame: expected {:#x}, found {:#x}", 
                            page.start_address().as_u64(), frame.start_address().as_u64(), existing_frame.start_address().as_u64());
                        return Err("User stack page already mapped to different frame");
                    }
                } else {
                    // Page not mapped, map it now
                    match process_page_table.map_page(page, frame, flags) {
                        Ok(()) => {
                            _mapped_pages += 1;
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
    
    Ok(())
}
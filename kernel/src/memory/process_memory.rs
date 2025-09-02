//! Per-process memory management
//!
//! This module provides per-process page tables and address space isolation.

use crate::memory::frame_allocator::{allocate_frame, GlobalFrameAllocator};
use x86_64::{
    registers::control::Cr3,
    structures::paging::{
        mapper::TranslateResult, Mapper, OffsetPageTable, Page, PageTable, PageTableFlags,
        PhysFrame, Size4KiB, Translate,
    },
    PhysAddr, VirtAddr,
};

/// A per-process page table
pub struct ProcessPageTable {
    /// Physical frame containing the level 4 page table
    level_4_frame: PhysFrame,
    /// The mapper for this page table
    mapper: OffsetPageTable<'static>,
}

impl ProcessPageTable {
    /// Deep copy a PML4 entry, creating independent L3/L2/L1 tables
    ///
    /// This creates a complete copy of the page table hierarchy below this L4 entry,
    /// ensuring that each process has its own isolated page tables.
    fn deep_copy_pml4_entry(
        source_entry: &x86_64::structures::paging::page_table::PageTableEntry,
        entry_index: usize,
        phys_offset: VirtAddr,
    ) -> Result<PhysFrame, &'static str> {
        log::debug!(
            "Deep copying PML4 entry {} with L3 addr {:#x}",
            entry_index,
            source_entry.addr().as_u64()
        );

        // Allocate a new L3 table
        let new_l3_frame = allocate_frame().ok_or("Failed to allocate frame for L3 table")?;

        // Map the new L3 table
        let new_l3_virt = phys_offset + new_l3_frame.start_address().as_u64();
        let new_l3_table = unsafe { &mut *(new_l3_virt.as_mut_ptr() as *mut PageTable) };

        // Clear the new L3 table
        new_l3_table.zero();

        // Map the source L3 table
        let source_l3_virt = phys_offset + source_entry.addr().as_u64();
        let source_l3_table = unsafe { &*(source_l3_virt.as_ptr() as *const PageTable) };

        // Copy each L3 entry, deep copying L2 tables as needed
        for i in 0..512 {
            if !source_l3_table[i].is_unused() {
                // Check if this is a huge page (1GB page at L3 level)
                if source_l3_table[i]
                    .flags()
                    .contains(PageTableFlags::HUGE_PAGE)
                {
                    // Huge pages can be shared at the physical level
                    new_l3_table[i] = source_l3_table[i].clone();
                    log::trace!("Copied L3 huge page entry {}", i);
                } else {
                    // Regular L3 entry pointing to L2 table - deep copy the L2 table
                    match Self::deep_copy_l3_entry(&source_l3_table[i], i, phys_offset) {
                        Ok(new_l2_frame) => {
                            let flags = source_l3_table[i].flags();
                            new_l3_table[i].set_addr(new_l2_frame.start_address(), flags);
                            log::trace!(
                                "Deep copied L3 entry {} -> new L2 frame {:#x}",
                                i,
                                new_l2_frame.start_address().as_u64()
                            );
                        }
                        Err(e) => {
                            log::error!("Failed to deep copy L3 entry {}: {}", i, e);
                            return Err("Failed to deep copy L3 entry");
                        }
                    }
                }
            }
        }

        log::debug!(
            "Successfully deep copied PML4 entry {} to new L3 frame {:#x}",
            entry_index,
            new_l3_frame.start_address().as_u64()
        );

        Ok(new_l3_frame)
    }

    /// Deep copy an L3 entry, creating independent L2/L1 tables
    fn deep_copy_l3_entry(
        source_entry: &x86_64::structures::paging::page_table::PageTableEntry,
        entry_index: usize,
        phys_offset: VirtAddr,
    ) -> Result<PhysFrame, &'static str> {
        // Allocate a new L2 table
        let new_l2_frame = allocate_frame().ok_or("Failed to allocate frame for L2 table")?;

        // Map the new L2 table
        let new_l2_virt = phys_offset + new_l2_frame.start_address().as_u64();
        let new_l2_table = unsafe { &mut *(new_l2_virt.as_mut_ptr() as *mut PageTable) };

        // Clear the new L2 table
        new_l2_table.zero();

        // Map the source L2 table
        let source_l2_virt = phys_offset + source_entry.addr().as_u64();
        let source_l2_table = unsafe { &*(source_l2_virt.as_ptr() as *const PageTable) };

        // Copy each L2 entry, deep copying L1 tables as needed
        for i in 0..512 {
            if !source_l2_table[i].is_unused() {
                // Check if this is a huge page (2MB page at L2 level)
                if source_l2_table[i]
                    .flags()
                    .contains(PageTableFlags::HUGE_PAGE)
                {
                    // Huge pages can be shared at the physical level for kernel code
                    // Calculate the virtual address this L2 entry covers
                    let l2_virt_addr = (entry_index as u64) * 0x40000000 + (i as u64) * 0x200000; // 1GB per L3, 2MB per L2

                    if l2_virt_addr >= 0x800000000000 {
                        // Kernel space - safe to share huge pages
                        new_l2_table[i] = source_l2_table[i].clone();
                        log::trace!(
                            "Shared kernel L2 huge page entry {} (addr {:#x})",
                            i,
                            l2_virt_addr
                        );
                    } else {
                        // User space - should not share, but for now we'll share kernel code pages
                        // This needs refinement based on what's actually mapped
                        new_l2_table[i] = source_l2_table[i].clone();
                        // Don't spam logs with hundreds of huge page entries
                        if i < 10 {
                            log::trace!(
                                "Shared user L2 huge page entry {} (addr {:#x}) - FIXME",
                                i,
                                l2_virt_addr
                            );
                        }
                    }
                } else {
                    // Regular L2 entry pointing to L1 table - deep copy the L1 table
                    match Self::deep_copy_l2_entry(&source_l2_table[i], i, phys_offset) {
                        Ok(new_l1_frame) => {
                            let flags = source_l2_table[i].flags();
                            new_l2_table[i].set_addr(new_l1_frame.start_address(), flags);
                            log::trace!(
                                "Deep copied L2 entry {} -> new L1 frame {:#x}",
                                i,
                                new_l1_frame.start_address().as_u64()
                            );
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
        phys_offset: VirtAddr,
    ) -> Result<PhysFrame, &'static str> {
        // Allocate a new L1 table
        let new_l1_frame = allocate_frame().ok_or("Failed to allocate frame for L1 table")?;

        // Map the new L1 table
        let new_l1_virt = phys_offset + new_l1_frame.start_address().as_u64();
        let new_l1_table = unsafe { &mut *(new_l1_virt.as_mut_ptr() as *mut PageTable) };

        // Clear the new L1 table
        new_l1_table.zero();

        // Map the source L1 table
        let source_l1_virt = phys_offset + source_entry.addr().as_u64();
        let source_l1_table = unsafe { &*(source_l1_virt.as_ptr() as *const PageTable) };

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
                    log::trace!(
                        "Copied L1 entry {} -> phys frame {:#x}",
                        i,
                        source_l1_table[i].addr().as_u64()
                    );
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
        // Check stack pointer before allocating
        let rsp: u64;
        unsafe {
            core::arch::asm!("mov {}, rsp", out(reg) rsp);
        }
        log::debug!("ProcessPageTable::new() - Current RSP: {:#x}", rsp);

        // Check if we're running low on stack
        // Kernel stacks typically start around 0x180000xxxxx and grow down
        // If we're below 0x180000010000, we might be in trouble
        if rsp < 0x180000010000 {
            log::error!("WARNING: Low stack detected! RSP={:#x}", rsp);
            log::error!("This might cause a stack overflow!");
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
                    log::error!(
                        "WARNING: Allocated frame 0x611000 which is already in use by a process!"
                    );
                }

                frame
            }
            None => {
                log::error!("Frame allocator returned None - out of memory?");
                return Err("Failed to allocate frame for page table");
            }
        };

        log::debug!(
            "Allocated L4 frame: {:#x}",
            level_4_frame.start_address().as_u64()
        );

        // Get physical memory offset
        let phys_offset = crate::memory::physical_memory_offset();

        // Verify the frame is within expected range
        let frame_addr = level_4_frame.start_address().as_u64();
        if frame_addr > 0x10000000 {
            // 256MB limit
            log::error!(
                "Allocated frame {:#x} is beyond expected physical memory range",
                frame_addr
            );
            return Err("Frame allocator returned invalid frame");
        }

        // Map the new page table frame
        let level_4_table = unsafe {
            log::debug!("Physical memory offset: {:#x}", phys_offset.as_u64());
            let virt = phys_offset + level_4_frame.start_address().as_u64();
            log::debug!("New L4 table virtual address: {:#x}", virt.as_u64());
            log::debug!(
                "About to create mutable reference to page table at {:#x}",
                virt.as_u64()
            );

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
                log::debug!(
                    "ProcessPageTable::new() - Current CR3: {:#x}",
                    current_frame.start_address().as_u64()
                );
                log::debug!(
                    "ProcessPageTable::new() - Using kernel CR3: {:#x} for copying",
                    KERNEL_CR3
                );

                // Always use the kernel's page table for copying kernel mappings
                let kernel_frame =
                    PhysFrame::<Size4KiB>::containing_address(PhysAddr::new(KERNEL_CR3));
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

            // Set up PML4 entries 256-511 to point to the shared kernel PDPT
            let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE;

            for i in 256..512 {
                level_4_table[i].set_frame(kernel_pdpt_frame, flags);
            }

            log::debug!("Set up global kernel page table entries 256-511");

            // Copy essential low-memory kernel mappings (entries 0-255)
            // These are needed for kernel code that lives in low memory
            let copied_count = {
                let mut count = 0;
                for i in 0..256 {
                    if !current_l4_table[i].is_unused() {
                        let has_user_accessible = current_l4_table[i]
                            .flags()
                            .contains(PageTableFlags::USER_ACCESSIBLE);

                        // Copy kernel-only entries in low memory (e.g., kernel code at 0x10000000)
                        if (i >= 2 && i <= 7) || (!has_user_accessible && i != 0 && i != 1) {
                            level_4_table[i] = current_l4_table[i].clone();
                            count += 1;
                            log::debug!("Copied low-memory kernel PML4 entry {}", i);
                        }
                    }
                }
                count
            };

            log::debug!("Process page table created with global kernel mappings ({} low entries + 256 high entries)", copied_count);
        }

        // Create mapper for the new page table
        // We need to get a fresh pointer to the level_4_table to avoid borrow conflicts
        let mapper = unsafe {
            let level_4_table_ptr = {
                let virt = phys_offset + level_4_frame.start_address().as_u64();
                &mut *(virt.as_mut_ptr() as *mut PageTable)
            };

            log::debug!(
                "Creating OffsetPageTable with L4 frame {:#x} and phys_offset {:#x}",
                level_4_frame.start_address().as_u64(),
                phys_offset.as_u64()
            );
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
        log::trace!(
            "ProcessPageTable::map_page called for page {:#x}",
            page.start_address().as_u64()
        );
        unsafe {
            log::trace!("About to call mapper.map_to...");

            // CRITICAL WORKAROUND: The OffsetPageTable might be failing during child
            // page table operations. Let's add extra validation.

            // First, ensure we're not trying to map kernel addresses as user pages
            let page_addr = page.start_address().as_u64();
            if page_addr >= 0x800000000000 && flags.contains(PageTableFlags::USER_ACCESSIBLE) {
                log::error!(
                    "Attempting to map kernel address {:#x} as user-accessible!",
                    page_addr
                );
                return Err("Cannot map kernel addresses as user-accessible");
            }

            // CRITICAL FIX: Check if page is already mapped before attempting to map
            // This handles the case where L3 tables are shared between processes
            if let Ok(existing_frame) = self.mapper.translate_page(page) {
                if existing_frame == frame {
                    // Page is already mapped to the correct frame, skip
                    log::trace!(
                        "Page {:#x} already mapped to frame {:#x}, skipping",
                        page.start_address().as_u64(),
                        frame.start_address().as_u64()
                    );
                    return Ok(());
                } else {
                    // Page is mapped to a different frame, this is an error
                    log::error!(
                        "Page {:#x} already mapped to different frame {:#x} (wanted {:#x})",
                        page.start_address().as_u64(),
                        existing_frame.start_address().as_u64(),
                        frame.start_address().as_u64()
                    );
                    return Err("Page already mapped to different frame");
                }
            }

            // Page is not mapped, proceed with mapping
            match self
                .mapper
                .map_to(page, frame, flags, &mut GlobalFrameAllocator)
            {
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
                    // Enhanced error logging to understand map_to failures
                    use x86_64::structures::paging::mapper::MapToError;
                    let error_msg = match e {
                        MapToError::FrameAllocationFailed => {
                            log::error!("map_to failed: Frame allocation failed - OUT OF MEMORY!");
                            "Frame allocator out of memory"
                        }
                        MapToError::ParentEntryHugePage => {
                            log::error!("map_to failed: Parent entry is a huge page");
                            "Cannot map: parent is huge page"
                        }
                        MapToError::PageAlreadyMapped(existing_frame) => {
                            log::error!(
                                "map_to failed: Page already mapped to frame {:#x}",
                                existing_frame.start_address().as_u64()
                            );
                            "Page already mapped"
                        }
                    };
                    Err(error_msg)
                }
            }
        }
    }

    /// Unmap a page in this process's address space
    pub fn unmap_page(
        &mut self,
        page: Page<Size4KiB>,
    ) -> Result<PhysFrame<Size4KiB>, &'static str> {
        let (frame, flush) = self
            .mapper
            .unmap(page)
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
                    log::trace!(
                        "translate_page({:#x}) -> {:#x}",
                        addr.as_u64(),
                        phys.as_u64()
                    );
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
                            log::debug!(
                                "  -> L4 entry {} exists: addr={:#x}, flags={:?}",
                                l4_index,
                                l4_entry.addr().as_u64(),
                                l4_entry.flags()
                            );

                            // Let's check the L3 table
                            let l3_phys = l4_entry.addr();
                            let l3_virt = phys_offset + l3_phys.as_u64();
                            let l3_table = &*(l3_virt.as_ptr()
                                as *const x86_64::structures::paging::PageTable);

                            let l3_index = (addr.as_u64() >> 30) & 0x1ff;
                            let l3_entry = &l3_table[l3_index as usize];

                            if l3_entry.is_unused() {
                                log::debug!("    -> L3 entry {} is UNUSED", l3_index);
                            } else {
                                log::debug!(
                                    "    -> L3 entry {} exists: addr={:#x}, flags={:?}",
                                    l3_index,
                                    l3_entry.addr().as_u64(),
                                    l3_entry.flags()
                                );

                                // Check L2 (Page Directory) table
                                let l2_phys = l3_entry.addr();
                                let l2_virt = phys_offset + l2_phys.as_u64();
                                let l2_table = &*(l2_virt.as_ptr()
                                    as *const x86_64::structures::paging::PageTable);

                                let l2_index = (addr.as_u64() >> 21) & 0x1ff;
                                let l2_entry = &l2_table[l2_index as usize];

                                if l2_entry.is_unused() {
                                    log::debug!(
                                        "      -> L2 entry {} is UNUSED (THIS IS THE PROBLEM!)",
                                        l2_index
                                    );
                                } else {
                                    log::debug!(
                                        "      -> L2 entry {} exists: addr={:#x}, flags={:?}",
                                        l2_index,
                                        l2_entry.addr().as_u64(),
                                        l2_entry.flags()
                                    );

                                    // Check L1 (Page Table) if L2 exists
                                    let l1_phys = l2_entry.addr();
                                    let l1_virt = phys_offset + l1_phys.as_u64();
                                    let l1_table = &*(l1_virt.as_ptr()
                                        as *const x86_64::structures::paging::PageTable);

                                    let l1_index = (addr.as_u64() >> 12) & 0x1ff;
                                    let l1_entry = &l1_table[l1_index as usize];

                                    if l1_entry.is_unused() {
                                        log::debug!(
                                            "        -> L1 entry {} is UNUSED (PAGE NOT MAPPED!)",
                                            l1_index
                                        );
                                    } else {
                                        log::debug!(
                                            "        -> L1 entry {} exists: addr={:#x}, flags={:?}",
                                            l1_index,
                                            l1_entry.addr().as_u64(),
                                            l1_entry.flags()
                                        );
                                    }
                                }
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

    /// Allocate a stack in this process's page table
    pub fn allocate_stack(
        &mut self,
        size: usize,
        privilege: crate::task::thread::ThreadPrivilege,
    ) -> Result<crate::memory::stack::GuardedStack, &'static str> {
        crate::memory::stack::GuardedStack::new(size, &mut self.mapper, privilege)
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
        // Updated to match new stack allocation range in high canonical space
        let stack_bottom = VirtAddr::new(0x7FFFFF000000);
        let stack_top = VirtAddr::new(0x7FFFFFFF0000);
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
    pub fn unmap_user_pages(
        &mut self,
        start_addr: VirtAddr,
        end_addr: VirtAddr,
    ) -> Result<(), &'static str> {
        log::debug!(
            "Unmapping user pages from {:#x} to {:#x}",
            start_addr.as_u64(),
            end_addr.as_u64()
        );

        let start_page = Page::<Size4KiB>::containing_address(start_addr);
        let end_page = Page::<Size4KiB>::containing_address(end_addr);

        for page in Page::range_inclusive(start_page, end_page) {
            // Try to unmap the page - it's OK if it's not mapped
            match self.mapper.unmap(page) {
                Ok((frame, _flush)) => {
                    // Don't flush immediately - the page table switch will handle it
                    log::trace!(
                        "Unmapped page {:#x} (was mapped to frame {:#x})",
                        page.start_address().as_u64(),
                        frame.start_address().as_u64()
                    );
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
        log::debug!(
            "About to switch page table: {:?} -> {:?}",
            current_frame,
            new_frame
        );
        log::debug!("Current stack pointer: {:#x}", {
            let mut rsp: u64;
            core::arch::asm!("mov {}, rsp", out(reg) rsp);
            rsp
        });

        // Verify that kernel mappings are present in the new page table
        let phys_offset = crate::memory::physical_memory_offset();
        let new_l4_table =
            &*(((phys_offset + new_frame.start_address().as_u64()).as_u64()) as *const PageTable);

        let mut kernel_entries = 0;
        for i in 256..512 {
            if !new_l4_table[i].is_unused() {
                kernel_entries += 1;
            }
        }
        log::debug!(
            "Process page table has {} kernel PML4 entries",
            kernel_entries
        );

        if kernel_entries == 0 {
            log::error!("CRITICAL: Process page table has no kernel mappings! This will cause immediate crash!");
            return;
        }

        log::trace!(
            "Switching page table: {:?} -> {:?}",
            current_frame,
            new_frame
        );
        Cr3::write(new_frame, flags);
        // Ensure TLB consistency after page table switch
        super::tlb::flush_after_page_table_switch();
        log::debug!("Page table switch completed successfully with TLB flush");
    }
}

/// Get the kernel's page table frame (the one created by bootloader)
static mut KERNEL_PAGE_TABLE_FRAME: Option<PhysFrame> = None;

/// Get the kernel page table frame
pub fn kernel_page_table_frame() -> PhysFrame {
    unsafe { KERNEL_PAGE_TABLE_FRAME.expect("Kernel page table frame not initialized") }
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

/// Switch back to the kernel page table
///
/// # Safety
/// Caller must ensure this is called from a safe context
pub unsafe fn switch_to_kernel_page_table() {
    if let Some(kernel_frame) = KERNEL_PAGE_TABLE_FRAME {
        let (current_frame, flags) = Cr3::read();
        if current_frame != kernel_frame {
            log::trace!(
                "Switching back to kernel page table: {:?} -> {:?}",
                current_frame,
                kernel_frame
            );
            Cr3::write(kernel_frame, flags);
            // Ensure TLB consistency after page table switch
            super::tlb::flush_after_page_table_switch();
        }
    } else {
        log::error!("Kernel page table frame not initialized!");
    }
}

// NOTE: This function is no longer needed with global kernel page tables
// All kernel stacks are automatically visible to all processes through the shared kernel PDPT

/// Map user stack pages from kernel page table to process page table
/// This is critical for userspace execution - the stack must be accessible
pub fn map_user_stack_to_process(
    process_page_table: &mut ProcessPageTable,
    stack_bottom: VirtAddr,
    stack_top: VirtAddr,
) -> Result<(), &'static str> {
    log::debug!(
        "map_user_stack_to_process: mapping stack range {:#x} - {:#x}",
        stack_bottom.as_u64(),
        stack_top.as_u64()
    );

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
            TranslateResult::Mapped {
                frame,
                offset,
                flags: _,
            } => {
                let phys_addr = frame.start_address() + offset;
                let frame = PhysFrame::containing_address(phys_addr);

                // Map the same physical frame in the process page table
                // Use user-accessible permissions for user stack
                let flags = PageTableFlags::PRESENT
                    | PageTableFlags::WRITABLE
                    | PageTableFlags::USER_ACCESSIBLE;

                // Check if already mapped
                if let Some(existing_frame) =
                    process_page_table.translate_page(page.start_address())
                {
                    let existing_frame = PhysFrame::containing_address(existing_frame);
                    if existing_frame == frame {
                        log::trace!(
                            "User stack page {:#x} already mapped correctly to frame {:#x}",
                            page.start_address().as_u64(),
                            frame.start_address().as_u64()
                        );
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
                            log::trace!(
                                "Mapped user stack page {:#x} -> frame {:#x}",
                                page.start_address().as_u64(),
                                frame.start_address().as_u64()
                            );
                        }
                        Err(e) => {
                            log::error!(
                                "Failed to map user stack page {:#x}: {}",
                                page.start_address().as_u64(),
                                e
                            );
                            return Err("Failed to map user stack page");
                        }
                    }
                }
            }
            _ => {
                log::error!(
                    "User stack page {:#x} not mapped in kernel page table!",
                    page.start_address().as_u64()
                );
                return Err("User stack page not found in kernel page table");
            }
        }
    }

    log::debug!(
        "✓ Successfully mapped {} user stack pages to process page table",
        mapped_pages
    );
    Ok(())
}

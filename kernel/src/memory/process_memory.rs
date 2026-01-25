//! Per-process memory management
//!
//! This module provides per-process page tables and address space isolation.

use crate::memory::frame_allocator::{allocate_frame, GlobalFrameAllocator};
#[cfg(target_arch = "x86_64")]
use x86_64::{
    registers::control::Cr3,
    structures::paging::{
        mapper::TranslateResult,
        page_table::PageTableEntry,
        Mapper, OffsetPageTable, Page, PageTable, PageTableFlags, PhysFrame, Size4KiB, Translate,
    },
    PhysAddr, VirtAddr,
};
#[cfg(not(target_arch = "x86_64"))]
use crate::memory::arch_stub::{
    mapper::TranslateResult,
    Cr3,
    Mapper, OffsetPageTable, Page, PageTable, PageTableEntry, PageTableFlags, PhysAddr, PhysFrame,
    Size4KiB, Translate, VirtAddr,
};

// ============================================================================
// Copy-on-Write (CoW) Support
// ============================================================================

/// Copy-on-Write flag - uses bit 9 (available for OS use in x86_64 page tables)
///
/// When set: page is read-only due to CoW sharing (was originally writable)
/// When clear: page is truly read-only or writable as intended
///
/// This flag distinguishes between:
/// - Pages that are read-only because they're CoW-shared (can become writable after copy)
/// - Pages that are genuinely read-only (e.g., code sections)
pub const COW_FLAG: PageTableFlags = PageTableFlags::BIT_9;

/// Check if a page has Copy-on-Write semantics
///
/// A CoW page is one that was originally writable but is currently marked
/// read-only for sharing. On write, it should be copied to a private page.
#[inline]
pub fn is_cow_page(flags: PageTableFlags) -> bool {
    flags.contains(COW_FLAG)
}

/// Convert writable page flags to CoW flags
///
/// Removes WRITABLE and adds COW_FLAG to mark the page as CoW-shared.
/// Used when setting up page sharing during fork().
#[inline]
#[allow(dead_code)]
pub fn make_cow_flags(original_flags: PageTableFlags) -> PageTableFlags {
    let mut flags = original_flags;
    flags.remove(PageTableFlags::WRITABLE);
    flags.insert(COW_FLAG);
    flags
}

/// Convert CoW page flags back to private writable flags
///
/// Adds WRITABLE and removes COW_FLAG after copying the page.
/// Used when handling a CoW fault - the new private copy becomes writable.
#[inline]
pub fn make_private_flags(original_flags: PageTableFlags) -> PageTableFlags {
    let mut flags = original_flags;
    flags.insert(PageTableFlags::WRITABLE);
    flags.remove(COW_FLAG);
    flags
}

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
    #[allow(dead_code)]
    fn deep_copy_pml4_entry(
        source_entry: &PageTableEntry,
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
        // Clear all entries properly (not using zero() which sets PRESENT | WRITABLE)
        for i in 0..512 {
            new_l3_table[i].set_unused();
        }

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
    #[allow(dead_code)]
    fn deep_copy_l3_entry(
        source_entry: &PageTableEntry,
        entry_index: usize,
        phys_offset: VirtAddr,
    ) -> Result<PhysFrame, &'static str> {
        // Allocate a new L2 table
        let new_l2_frame = allocate_frame().ok_or("Failed to allocate frame for L2 table")?;

        // Map the new L2 table
        let new_l2_virt = phys_offset + new_l2_frame.start_address().as_u64();
        let new_l2_table = unsafe { &mut *(new_l2_virt.as_mut_ptr() as *mut PageTable) };

        // Clear the new L2 table
        // Clear all entries properly (not using zero() which sets PRESENT | WRITABLE)
        for i in 0..512 {
            new_l2_table[i].set_unused();
        }

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
    #[allow(dead_code)]
    fn deep_copy_l2_entry(
        source_entry: &PageTableEntry,
        _entry_index: usize,
        phys_offset: VirtAddr,
    ) -> Result<PhysFrame, &'static str> {
        // Allocate a new L1 table
        let new_l1_frame = allocate_frame().ok_or("Failed to allocate frame for L1 table")?;

        // Map the new L1 table
        let new_l1_virt = phys_offset + new_l1_frame.start_address().as_u64();
        let new_l1_table = unsafe { &mut *(new_l1_virt.as_mut_ptr() as *mut PageTable) };

        // Clear the new L1 table
        // Clear all entries properly (not using zero() which sets PRESENT | WRITABLE)
        for i in 0..512 {
            new_l1_table[i].set_unused();
        }

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
        // NOTE: Removed serial_println here to avoid potential stack issues
        
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
                crate::serial_println!("🔍 ProcessPageTable::new() allocated L4 frame={:#x}", frame_addr);

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
        crate::serial_println!("🔍 ProcessPageTable will use L4 frame={:#x}", level_4_frame.start_address().as_u64());

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

        log::debug!("About to clear the new page table");
        // CRITICAL: Properly clear the new page table
        // Do NOT use zero() as it sets entries to PRESENT | WRITABLE with addr=0x0!
        // We need to set all entries to actually be empty (0x0)
        for i in 0..512 {
            level_4_table[i].set_unused();
        }
        log::debug!("Successfully cleared new page table (all entries set to unused)");

        // Copy kernel mappings from the CURRENT page table
        // The current CR3 has working mappings (kernel is running), so use those
        unsafe {
            let current_l4_table = {
                // Use the CURRENT CR3 which has working mappings
                let (current_frame, _) = Cr3::read();
                log::debug!(
                    "ProcessPageTable::new() - Using current CR3: {:#x} for copying",
                    current_frame.start_address().as_u64()
                );

                let virt = phys_offset + current_frame.start_address().as_u64();
                log::debug!("Current L4 table virtual address: {:#x}", virt.as_u64());
                &*(virt.as_ptr() as *const PageTable)
            };

            // Copy kernel mappings from the current page table
            // This is critical - we need ALL kernel mappings to be present in every
            // process page table so the kernel can function after a page table switch

            // NEW: Use global kernel page tables for entries 256-511
            // This ensures all kernel mappings (including dynamically allocated kernel stacks)
            // are visible to all processes

            // CRITICAL: Copy ALL kernel PML4 entries to ensure kernel code remains accessible
            // after CR3 switch. This follows standard OS practice of sharing kernel mappings
            // across all process page tables.
            
            let mut kernel_entries_count = 0;
            
            // Copy upper half (256-511) - traditional kernel space
            // First, let's debug what's actually in the kernel page table
            log::debug!("Examining kernel page table upper half entries:");
            let mut valid_upper_entries = 0;
            for i in 256..512 {
                if !current_l4_table[i].is_unused() {
                    let addr = current_l4_table[i].addr();
                    let flags = current_l4_table[i].flags();
                    
                    // Log ALL upper half entries for debugging
                    if i <= 260 || i >= 509 {  // First few and last few
                        log::debug!("  Kernel PML4[{}]: phys={:#x}, flags={:?}", i, addr.as_u64(), flags);
                    }
                    
                    // CRITICAL: Validate that the entry has a valid physical address
                    // An entry with PRESENT but addr=0x0 is invalid and would cause crashes
                    if flags.contains(PageTableFlags::PRESENT) && addr.as_u64() == 0 {
                        log::warn!("PML4[{}] has PRESENT flag but invalid address 0x0, skipping", i);
                        continue;
                    }
                    
                    if addr.as_u64() != 0 {
                        valid_upper_entries += 1;
                        // CRITICAL FIX: Keep kernel mappings EXACTLY as they are
                        // The kernel needs these exact flags to function after CR3 switch
                        // DO NOT modify flags - copy them verbatim
                        level_4_table[i].set_addr(addr, flags);
                        kernel_entries_count += 1;
                        //log::debug!("Copied kernel PML4[{}] with original flags", i);
                    }
                }
            }
            log::debug!("Found {} valid upper-half kernel PML4 entries (256-511)", valid_upper_entries);
            log::debug!("Copied {} upper-half kernel PML4 entries (256-511)", kernel_entries_count);
            
            // PHASE 2: Use master kernel PML4 if available
            if let Some(master_pml4_frame) = crate::memory::kernel_page_table::master_kernel_pml4() {
                log::info!("PHASE2: Using master kernel PML4 for process creation");
                
                // Copy upper-half entries from master instead of current
                let master_pml4_virt = phys_offset + master_pml4_frame.start_address().as_u64();
                let master_pml4 = &*(master_pml4_virt.as_ptr() as *const PageTable);
                
                // Log what we're about to copy for critical entries
                log::info!("PHASE2-DEBUG: Reading master PML4 from virtual address {:p}", master_pml4);
                log::info!("PHASE2-DEBUG: Master PML4[402] = {:?}", master_pml4[402].frame());
                log::info!("PHASE2-DEBUG: Master PML4[403] = {:?}", master_pml4[403].frame());
                log::info!("PHASE2-DEBUG: &master_pml4[403] is at {:p}", &master_pml4[403]);
                
                // CRITICAL FIX: Copy PML4[2] (direct physical memory mapping) where kernel code/data lives
                // The kernel is mapped at 0x100000000 (PML4[2]), not in the upper half!
                //
                // BUG FIX: DO NOT set USER_ACCESSIBLE on kernel mappings!
                // PML4[2] contains kernel code, data, GDT, IDT, TSS - these must be Ring 0 only.
                // The CPU can access these structures during exception handling because the exception
                // handler runs in Ring 0 (kernel mode), not Ring 3 (user mode).
                if !master_pml4[2].is_unused() {
                    let master_flags = master_pml4[2].flags();
                    // Keep the master flags EXACTLY as they are - no modifications
                    // The kernel structures at PML4[2] should remain kernel-only
                    level_4_table[2].set_addr(master_pml4[2].addr(), master_flags);
                    log::info!("CRITICAL: Copied PML4[2] (direct phys mapping) from master with kernel-only flags: {:?}", master_flags);
                }

                // Copy ONLY kernel-specific lower-half entries from master
                // CRITICAL BUG FIX: Do NOT copy entries that contain userspace content!
                // PML4[0] (0x0-0x7FFFFFFFFF) - userspace code/data, per-process
                // PML4[255] (0x7F8000000000-0x8000000000) - userspace stack region, per-process
                // PML4[2] is handled above (direct physical memory)
                // Only copy entries that are truly kernel-only (no USER_ACCESSIBLE)
                let mut lower_half_copied = 0;
                for i in 1..256 {
                    if i == 2 {
                        continue; // Already handled above with special flags
                    }
                    if !master_pml4[i].is_unused() {
                        // CRITICAL: Only copy if the entry does NOT have USER_ACCESSIBLE
                        // Entries with USER_ACCESSIBLE are userspace regions that should be per-process
                        let flags = master_pml4[i].flags();
                        if flags.contains(PageTableFlags::USER_ACCESSIBLE) {
                            log::trace!("Skipping PML4[{}] - has USER_ACCESSIBLE flag (userspace region)", i);
                            continue;
                        }
                        level_4_table[i] = master_pml4[i].clone();
                        lower_half_copied += 1;
                    }
                }
                log::info!("PHASE2: Copied {} kernel-only lower-half entries (skipped userspace), PML4[0] left empty", lower_half_copied);

                // CREATE a fresh PDPT for PML4[0] to enable userspace mappings
                // PML4[0] covers 0x0 - 0x7FFFFFFFFF (512GB) - this is the userspace region
                // Each process needs its own independent page table hierarchy here
                let fresh_pdpt_frame = allocate_frame().ok_or("Failed to allocate PDPT for PML4[0]")?;

                // Map and zero out the fresh PDPT
                let fresh_pdpt_virt = phys_offset + fresh_pdpt_frame.start_address().as_u64();
                let fresh_pdpt = &mut *(fresh_pdpt_virt.as_mut_ptr() as *mut PageTable);
                for i in 0..512 {
                    fresh_pdpt[i].set_unused();
                }

                // Install the fresh PDPT in PML4[0] with user-accessible flags
                // The intermediate page tables need USER_ACCESSIBLE for userspace to work
                let pml4_0_flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE;
                level_4_table[0].set_addr(fresh_pdpt_frame.start_address(), pml4_0_flags);
                log::info!("PHASE2: Created fresh PDPT for PML4[0] at frame {:#x}", fresh_pdpt_frame.start_address().as_u64());

                // Copy PML4[256-511] from master (shared kernel upper half)
                // This includes IDT, TSS, GDT, per-CPU, kernel stacks, IST stacks, and all kernel structures
                let mut upper_half_copied = 0;
                for i in 256..512 {
                    if !master_pml4[i].is_unused() {
                        // CRITICAL FIX: Keep master kernel mappings EXACTLY as they are
                        // DO NOT modify flags - the master has the correct flags already
                        let master_flags = master_pml4[i].flags();
                        
                        level_4_table[i].set_addr(master_pml4[i].addr(), master_flags);
                        upper_half_copied += 1;
                        // Log critical entries for debugging
                        match i {
                            402 => {
                                let master_frame = master_pml4[i].frame().unwrap();
                                let copied_frame = level_4_table[i].frame().unwrap();
                                log::info!("PHASE2: PML4[402] (kernel stacks): master={:?}, copied={:?}", 
                                         master_frame, copied_frame);
                                if master_frame != copied_frame {
                                    log::error!("ERROR: Frame mismatch for PML4[402]!");
                                }
                            },
                            403 => {
                                let master_frame = master_pml4[i].frame().unwrap();
                                let copied_frame = level_4_table[i].frame().unwrap();
                                log::info!("PHASE2: PML4[403] (IST stacks): master={:?}, copied={:?}", 
                                         master_frame, copied_frame);
                                if master_frame != copied_frame {
                                    log::error!("ERROR: Frame mismatch for PML4[403]!");
                                }
                            },
                            510 => {
                                if !master_pml4[i].is_unused() {
                                    let master_frame = master_pml4[i].frame().unwrap();
                                    let copied_frame = level_4_table[i].frame().unwrap();
                                    log::info!("PHASE2: PML4[510]: master={:?}, copied={:?}", 
                                             master_frame, copied_frame);
                                }
                            },
                            511 => {
                                let master_frame = master_pml4[i].frame().unwrap();
                                let copied_frame = level_4_table[i].frame().unwrap();
                                log::info!("PHASE2: PML4[511] (kernel high-half): master={:?}, copied={:?}", 
                                         master_frame, copied_frame);
                            },
                            _ => {}
                        }
                    }
                }
                log::info!("PHASE2: Inherited {} upper-half kernel mappings (256-511) from master PML4", upper_half_copied);

                // INVARIANT ASSERTION: Kernel stacks and IST stacks must be in different frames
                // This catches bugs where PML4[402] and PML4[403] alias to the same PDPT,
                // which causes stack corruption during exception handling
                let pml4_402_frame = level_4_table[402].frame();
                let pml4_403_frame = level_4_table[403].frame();

                if let (Ok(f402), Ok(f403)) = (pml4_402_frame, pml4_403_frame) {
                    assert_ne!(
                        f402, f403,
                        "BUG: PML4[402] (kernel stacks) and PML4[403] (IST stacks) point to same frame {:?}. \
                         This will cause stack corruption during exception handling. \
                         Master PML4[402]={:?}, Master PML4[403]={:?}",
                        f402,
                        master_pml4[402].frame(),
                        master_pml4[403].frame()
                    );
                    log::info!("✓ INVARIANT OK: PML4[402]={:?} != PML4[403]={:?}", f402, f403);
                } else {
                    log::warn!("INVARIANT CHECK: Missing kernel stack entries - [402]={:?}, [403]={:?}",
                              pml4_402_frame, pml4_403_frame);
                }

                // FIX: DO NOT copy PML4[0] from master - each process needs its own PML4[0] for userspace
                // PML4[0] covers virtual addresses 0x0 - 0x7FFFFFFFFF (512GB)
                // This is where userspace programs are loaded (e.g., 0x50000000)
                // Sharing PML4[0] between processes causes "Page already mapped" errors
                // because processes see each other's userspace mappings
                //
                // The kernel identity mapping at 0x100000 is NOT needed in process page tables
                // because the kernel executes from upper-half addresses (PML4[256-511])
                // when running in process context
                //
                // TEMPORARY FIX: Copy lower-half kernel mappings from master
                // The kernel executes from multiple lower-half regions:
                // - PML4[0]: Identity mapping at 0x100000 [REMOVED - conflicts with userspace]
                // - PML4[2]: Direct physical memory mapping where kernel actually runs (0x100_xxxx_xxxx)
                // Once we move to high-half execution, we can remove this

                // PML4[0] is left EMPTY (all entries set to unused) - this is intentional
                // Each process gets its own independent userspace address range
                log::info!("PHASE2: PML4[0] left empty for process-specific userspace mappings (0x0 - 0x7FFFFFFFFF)");
                
                // NOTE: PML4[2] is already handled above at lines 398-483 with USER_ACCESSIBLE
                // set on all levels. Do NOT overwrite it here!
                // The earlier code sets USER_ACCESSIBLE on PML4[2] and all its PDPT/PD/PT entries
                // which is required for Ring 3 to execute INT 0x80 (CPU needs to read IDT).
                
                // CRITICAL: Also copy PML4[3] for kernel stack region
                // The kernel stack is at 0x180_xxxx_xxxx range  
                if !master_pml4[3].is_unused() {
                    // CRITICAL FIX: Keep PML4[3] EXACTLY as it is in master
                    // DO NOT modify flags - copy verbatim
                    let master_flags = master_pml4[3].flags();
                    level_4_table[3].set_addr(master_pml4[3].addr(), master_flags);
                    // log::info!("PHASE2-TEMP: Copied PML4[3] from master with original flags");
                }
                
                // Note: PML4[403] (IST stacks) is already copied in the upper-half loop above
                
                // PHASE 3: Identity mapping no longer needed since we're copying PML4[0] from master
                // which already contains the kernel low-half mappings
                // Once we complete the high-half transition, we'll remove the PML4[0] copy entirely
                log::info!("PHASE3: Skipping manual identity mapping - PML4[0] already copied from master");
                
                // Commented out - no longer needed since we copy PML4[0] from master
                /*
                unsafe {
                    // Map two regions:
                    // 1. Kernel code/data: 0x100000-0x300000 (2MB)
                    // 2. GDT/IDT/TSS/per-CPU: 0x100000e0000-0x100001000000 (2MB)
                    
                    // Region 1: Kernel code/data
                    let kernel_start = 0x100000u64;
                    let kernel_end = 0x300000u64;
                    let mut addr = kernel_start;
                    
                    while addr < kernel_end {
                        let page = Page::<Size4KiB>::containing_address(VirtAddr::new(addr));
                        let frame = PhysFrame::<Size4KiB>::containing_address(PhysAddr::new(addr));
                        
                        // Map with PRESENT | GLOBAL (no USER_ACCESSIBLE)
                        // Code pages should not have WRITABLE, data pages should
                        let flags = if addr < 0x200000 {
                            // Text section - read-only, executable
                            PageTableFlags::PRESENT | PageTableFlags::GLOBAL
                        } else {
                            // Data/BSS sections - read-write
                            PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::GLOBAL
                        };
                        
                        // Manually walk the page tables to install the mapping
                        // We'll use the existing page table hierarchy
                        let pml4_idx = (addr >> 39) & 0x1FF;
                        let pdpt_idx = (addr >> 30) & 0x1FF;
                        let pd_idx = (addr >> 21) & 0x1FF;
                        let pt_idx = (addr >> 12) & 0x1FF;
                        
                        // Get or create PDPT
                        let pdpt_frame = if level_4_table[pml4_idx as usize].is_unused() {
                            let frame = crate::memory::frame_allocator::allocate_frame()
                                .ok_or("Failed to allocate PDPT")?;
                            let pdpt_virt = phys_offset + frame.start_address().as_u64();
                            let pdpt = &mut *(pdpt_virt.as_mut_ptr() as *mut PageTable);
                            for i in 0..512 {
                                pdpt[i].set_unused();
                            }
                            level_4_table[pml4_idx as usize].set_frame(frame, 
                                PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE);
                            frame
                        } else {
                            level_4_table[pml4_idx as usize].frame().unwrap()
                        };
                        
                        let pdpt_virt = phys_offset + pdpt_frame.start_address().as_u64();
                        let pdpt = &mut *(pdpt_virt.as_mut_ptr() as *mut PageTable);
                        
                        // Get or create PD
                        let pd_frame = if pdpt[pdpt_idx as usize].is_unused() {
                            let frame = crate::memory::frame_allocator::allocate_frame()
                                .ok_or("Failed to allocate PD")?;
                            let pd_virt = phys_offset + frame.start_address().as_u64();
                            let pd = &mut *(pd_virt.as_mut_ptr() as *mut PageTable);
                            for i in 0..512 {
                                pd[i].set_unused();
                            }
                            pdpt[pdpt_idx as usize].set_frame(frame,
                                PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE);
                            frame
                        } else {
                            pdpt[pdpt_idx as usize].frame().unwrap()
                        };
                        
                        let pd_virt = phys_offset + pd_frame.start_address().as_u64();
                        let pd = &mut *(pd_virt.as_mut_ptr() as *mut PageTable);
                        
                        // Get or create PT
                        let pt_frame = if pd[pd_idx as usize].is_unused() {
                            let frame = crate::memory::frame_allocator::allocate_frame()
                                .ok_or("Failed to allocate PT")?;
                            let pt_virt = phys_offset + frame.start_address().as_u64();
                            let pt = &mut *(pt_virt.as_mut_ptr() as *mut PageTable);
                            for i in 0..512 {
                                pt[i].set_unused();
                            }
                            pd[pd_idx as usize].set_frame(frame,
                                PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE);
                            frame
                        } else {
                            pd[pd_idx as usize].frame().unwrap()
                        };
                        
                        let pt_virt = phys_offset + pt_frame.start_address().as_u64();
                        let pt = &mut *(pt_virt.as_mut_ptr() as *mut PageTable);
                        
                        // Map the page
                        pt[pt_idx as usize].set_frame(frame, flags);
                        
                        addr += 0x1000; // Next page
                    }
                    
                    // Region 2: GDT/IDT/TSS/per-CPU structures
                    // Based on KLAYOUT log output, these are at specific addresses:
                    // GDT: 0x100000f1bf8, IDT: 0x100000f1dc0, TSS: 0x100000f1b88, per-CPU: 0x100000f2e40
                    // Map the correct range: 0x100000f0000 - 0x100000f4000 (16 pages)
                    let control_start = 0x100000f0000u64;
                    let control_end = 0x100000f4000u64;
                    addr = control_start;
                    
                    while addr < control_end {
                        let _page = Page::<Size4KiB>::containing_address(VirtAddr::new(addr));
                        let frame = PhysFrame::<Size4KiB>::containing_address(PhysAddr::new(addr));
                        
                        // All control structures need read-write access AND user access for exception handling
                        // Without USER_ACCESSIBLE, CPU can't access these during exception from Ring 3
                        let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::GLOBAL | PageTableFlags::USER_ACCESSIBLE;
                        
                        // Manually walk the page tables to install the mapping
                        let pml4_idx = (addr >> 39) & 0x1FF;
                        let pdpt_idx = (addr >> 30) & 0x1FF;
                        let pd_idx = (addr >> 21) & 0x1FF;
                        let pt_idx = (addr >> 12) & 0x1FF;
                        
                        // Get or create PDPT
                        let pdpt_frame = if level_4_table[pml4_idx as usize].is_unused() {
                            let frame = crate::memory::frame_allocator::allocate_frame()
                                .ok_or("Failed to allocate PDPT")?;
                            let pdpt_virt = phys_offset + frame.start_address().as_u64();
                            let pdpt = &mut *(pdpt_virt.as_mut_ptr() as *mut PageTable);
                            for i in 0..512 {
                                pdpt[i].set_unused();
                            }
                            level_4_table[pml4_idx as usize].set_frame(frame, 
                                PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE);
                            frame
                        } else {
                            level_4_table[pml4_idx as usize].frame().unwrap()
                        };
                        
                        let pdpt_virt = phys_offset + pdpt_frame.start_address().as_u64();
                        let pdpt = &mut *(pdpt_virt.as_mut_ptr() as *mut PageTable);
                        
                        // Get or create PD
                        let pd_frame = if pdpt[pdpt_idx as usize].is_unused() {
                            let frame = crate::memory::frame_allocator::allocate_frame()
                                .ok_or("Failed to allocate PD")?;
                            let pd_virt = phys_offset + frame.start_address().as_u64();
                            let pd = &mut *(pd_virt.as_mut_ptr() as *mut PageTable);
                            for i in 0..512 {
                                pd[i].set_unused();
                            }
                            pdpt[pdpt_idx as usize].set_frame(frame,
                                PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE);
                            frame
                        } else {
                            pdpt[pdpt_idx as usize].frame().unwrap()
                        };
                        
                        let pd_virt = phys_offset + pd_frame.start_address().as_u64();
                        let pd = &mut *(pd_virt.as_mut_ptr() as *mut PageTable);
                        
                        // Get or create PT
                        let pt_frame = if pd[pd_idx as usize].is_unused() {
                            let frame = crate::memory::frame_allocator::allocate_frame()
                                .ok_or("Failed to allocate PT")?;
                            let pt_virt = phys_offset + frame.start_address().as_u64();
                            let pt = &mut *(pt_virt.as_mut_ptr() as *mut PageTable);
                            for i in 0..512 {
                                pt[i].set_unused();
                            }
                            pd[pd_idx as usize].set_frame(frame,
                                PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE);
                            frame
                        } else {
                            pd[pd_idx as usize].frame().unwrap()
                        };
                        
                        let pt_virt = phys_offset + pt_frame.start_address().as_u64();
                        let pt = &mut *(pt_virt.as_mut_ptr() as *mut PageTable);
                        
                        // Map the page
                        pt[pt_idx as usize].set_frame(frame, flags);
                        
                        addr += 0x1000; // Next page
                    }
                    
                    log::info!("PHASE3-TEMP: Mapped kernel regions: 0x100000-0x300000 and 0x100000f0000-0x100000f4000");
                }
                */
            } else {
            
            // Fallback to old behavior if no master PML4 (shouldn't happen after Phase 2)
            let mut low_kernel_entries = 0;
            for i in 0..256 {  // Include entry 0 for kernel code at 0x100000
                if !current_l4_table[i].is_unused() {
                    let addr = current_l4_table[i].addr();
                    let flags = current_l4_table[i].flags();
                    
                    // CRITICAL: Validate that the entry has a valid physical address
                    // An entry with PRESENT but addr=0x0 is invalid and would cause crashes
                    if flags.contains(PageTableFlags::PRESENT) && addr.as_u64() == 0 {
                        log::warn!("PML4[{}] has PRESENT flag but invalid address 0x0, skipping", i);
                        continue;
                    }
                    
                    // Copy ALL valid entries to ensure kernel can access everything it needs
                    if addr.as_u64() != 0 {
                        // CRITICAL: For PML4[0], we need special handling since it contains both
                        // kernel (0x100000-0x300000) and userspace (0x10000000) mappings
                        // CURSOR AGENT FIX: Set proper flags for ALL kernel mappings
                        let mut new_flags = flags;
                        new_flags.remove(PageTableFlags::USER_ACCESSIBLE);
                        new_flags.insert(PageTableFlags::GLOBAL);
                        
                        level_4_table[i].set_addr(addr, new_flags);
                        if i == 0 {
                            log::info!("PHASE1: Fixed PML4[0] flags for kernel code at 0x100000 (cleared USER, added GLOBAL)");
                        } else {
                            log::debug!("Fixed low-memory kernel PML4[{}] flags", i);
                        }
                        low_kernel_entries += 1;
                        log::debug!("Copied low-memory kernel PML4 entry {} (phys={:#x}, flags={:?})", i, addr.as_u64(), flags);
                    }
                }
            }
            
            log::debug!("Process page table created with {} kernel entries ({} low + {} high)", 
                kernel_entries_count + low_kernel_entries, low_kernel_entries, kernel_entries_count);
                
            // CRITICAL: Ensure kernel stacks are mapped (Phase 1)
            // The kernel stacks are at 0xffffc90000000000 range
            // This is PML4 entry 402 (0xffffc90000000000 >> 39 = 402)
            let kernel_stack_pml4_idx = 402;
            if !current_l4_table[kernel_stack_pml4_idx].is_unused() {
                // CURSOR AGENT FIX: Set proper flags for kernel stack mapping
                let mut stack_flags = current_l4_table[kernel_stack_pml4_idx].flags();
                stack_flags.remove(PageTableFlags::USER_ACCESSIBLE);
                stack_flags.insert(PageTableFlags::GLOBAL);
                level_4_table[kernel_stack_pml4_idx].set_addr(
                    current_l4_table[kernel_stack_pml4_idx].addr(), 
                    stack_flags
                );
                log::info!("PHASE1: Fixed kernel stack PML4[{}] flags (0xffffc90000000000)", kernel_stack_pml4_idx);
            } else {
                log::warn!("PHASE1: Kernel stack PML4[{}] not present in current table!", kernel_stack_pml4_idx);
            }
            
            // CRITICAL: Ensure IST double-fault stack is mapped (Phase 1)  
            // The IST stacks are at 0xffffc98000000000
            // This is PML4 entry 403 (0xffffc98000000000 >> 39 = 403)
            let ist_stack_pml4_idx = 403;
            if !current_l4_table[ist_stack_pml4_idx].is_unused() {
                // CURSOR AGENT FIX: Set proper flags for IST stack mapping
                let mut ist_flags = current_l4_table[ist_stack_pml4_idx].flags();
                ist_flags.remove(PageTableFlags::USER_ACCESSIBLE);
                ist_flags.insert(PageTableFlags::GLOBAL);
                level_4_table[ist_stack_pml4_idx].set_addr(
                    current_l4_table[ist_stack_pml4_idx].addr(),
                    ist_flags
                );
                log::info!("PHASE1: Fixed IST stack PML4[{}] flags (0xffffc98000000000)", ist_stack_pml4_idx);
            } else {
                log::warn!("PHASE1: IST stack PML4[{}] not present in current table!", ist_stack_pml4_idx);
            }
            } // End of else block for fallback behavior
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

    /// Walk all mapped user pages in this page table
    ///
    /// This walks the entire page table hierarchy (L4 -> L3 -> L2 -> L1) and calls
    /// the callback for each mapped page with (VirtAddr, PhysAddr, PageTableFlags).
    ///
    /// Only userspace pages (addresses < 0x8000_0000_0000_0000) are visited.
    /// Returns the count of pages visited.
    pub fn walk_mapped_pages<F>(&self, mut callback: F) -> Result<usize, &'static str>
    where
        F: FnMut(VirtAddr, PhysAddr, PageTableFlags),
    {
        let phys_offset = crate::memory::physical_memory_offset();
        let mut page_count = 0;

        unsafe {
            // Get the L4 table
            let l4_virt = phys_offset + self.level_4_frame.start_address().as_u64();
            let l4_table = &*(l4_virt.as_ptr() as *const PageTable);

            // Walk L4 entries 0-255 (userspace only, 256-511 is kernel)
            for l4_idx in 0..256u64 {
                let l4_entry = &l4_table[l4_idx as usize];
                if l4_entry.is_unused() || !l4_entry.flags().contains(PageTableFlags::PRESENT) {
                    continue;
                }

                // Get L3 table
                let l3_phys = l4_entry.addr();
                let l3_virt = phys_offset + l3_phys.as_u64();
                let l3_table = &*(l3_virt.as_ptr() as *const PageTable);

                for l3_idx in 0..512u64 {
                    let l3_entry = &l3_table[l3_idx as usize];
                    if l3_entry.is_unused() || !l3_entry.flags().contains(PageTableFlags::PRESENT) {
                        continue;
                    }

                    // Check for 1GB huge page
                    if l3_entry.flags().contains(PageTableFlags::HUGE_PAGE) {
                        // 1GB huge page - calculate virtual address
                        let virt_addr = VirtAddr::new((l4_idx << 39) | (l3_idx << 30));
                        let phys_addr = l3_entry.addr();
                        callback(virt_addr, phys_addr, l3_entry.flags());
                        page_count += 1;
                        continue;
                    }

                    // Get L2 table
                    let l2_phys = l3_entry.addr();
                    let l2_virt = phys_offset + l2_phys.as_u64();
                    let l2_table = &*(l2_virt.as_ptr() as *const PageTable);

                    for l2_idx in 0..512u64 {
                        let l2_entry = &l2_table[l2_idx as usize];
                        if l2_entry.is_unused() || !l2_entry.flags().contains(PageTableFlags::PRESENT) {
                            continue;
                        }

                        // Check for 2MB huge page
                        if l2_entry.flags().contains(PageTableFlags::HUGE_PAGE) {
                            // 2MB huge page - calculate virtual address
                            let virt_addr = VirtAddr::new((l4_idx << 39) | (l3_idx << 30) | (l2_idx << 21));
                            let phys_addr = l2_entry.addr();
                            callback(virt_addr, phys_addr, l2_entry.flags());
                            page_count += 1;
                            continue;
                        }

                        // Get L1 table
                        let l1_phys = l2_entry.addr();
                        let l1_virt = phys_offset + l1_phys.as_u64();
                        let l1_table = &*(l1_virt.as_ptr() as *const PageTable);

                        for l1_idx in 0..512u64 {
                            let l1_entry = &l1_table[l1_idx as usize];
                            if l1_entry.is_unused() || !l1_entry.flags().contains(PageTableFlags::PRESENT) {
                                continue;
                            }

                            // 4KB page - calculate virtual address
                            let virt_addr = VirtAddr::new(
                                (l4_idx << 39) | (l3_idx << 30) | (l2_idx << 21) | (l1_idx << 12)
                            );
                            let phys_addr = l1_entry.addr();
                            callback(virt_addr, phys_addr, l1_entry.flags());
                            page_count += 1;
                        }
                    }
                }
            }
        }

        Ok(page_count)
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
            // CRITICAL FIX: Use map_to_with_table_flags to ensure USER_ACCESSIBLE
            // is set on intermediate page tables (PML4, PDPT, PD) not just the final PT entry
            let table_flags = if flags.contains(PageTableFlags::USER_ACCESSIBLE) {
                // For user pages, intermediate tables need USER_ACCESSIBLE too
                PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE
            } else {
                // For kernel pages, intermediate tables don't need USER_ACCESSIBLE
                PageTableFlags::PRESENT | PageTableFlags::WRITABLE
            };
            
            match self
                .mapper
                .map_to_with_table_flags(page, frame, flags, table_flags, &mut GlobalFrameAllocator)
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
                    #[cfg(target_arch = "x86_64")]
                    use x86_64::structures::paging::mapper::MapToError;
                    #[cfg(not(target_arch = "x86_64"))]
                    use crate::memory::arch_stub::mapper::MapToError;
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
    #[allow(dead_code)]
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

    /// Update the flags of an already-mapped page
    ///
    /// This is used by mprotect to change page protections without remapping.
    /// The page must already be mapped; if not, this returns an error.
    pub fn update_page_flags(
        &mut self,
        page: Page<Size4KiB>,
        new_flags: PageTableFlags,
    ) -> Result<(), &'static str> {
        // First, check if the page is mapped and get the physical frame
        let frame = self
            .mapper
            .translate_page(page)
            .map_err(|_| "Page not mapped")?;

        // Unmap the page (get the frame back)
        let (unmapped_frame, _flush) = self
            .mapper
            .unmap(page)
            .map_err(|_| "Failed to unmap page for flag update")?;

        // Sanity check: the frame should match
        if unmapped_frame != frame {
            log::error!(
                "update_page_flags: frame mismatch! translate returned {:?}, unmap returned {:?}",
                frame,
                unmapped_frame
            );
            return Err("Frame mismatch during flag update");
        }

        // Remap with new flags
        // We need to determine appropriate table flags based on the new page flags
        let table_flags = if new_flags.contains(PageTableFlags::USER_ACCESSIBLE) {
            PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE
        } else {
            PageTableFlags::PRESENT | PageTableFlags::WRITABLE
        };

        unsafe {
            self.mapper
                .map_to_with_table_flags(
                    page,
                    frame,
                    new_flags,
                    table_flags,
                    &mut GlobalFrameAllocator,
                )
                .map_err(|_| "Failed to remap page with new flags")?
                .ignore(); // Don't flush - caller will handle TLB
        }

        Ok(())
    }

    /// Get frame and flags for a mapped page
    ///
    /// Returns the physical frame and page table flags for a 4KB page.
    /// Returns None if the page is not mapped or is a huge page.
    pub fn get_page_info(
        &self,
        page: Page<Size4KiB>,
    ) -> Option<(PhysFrame<Size4KiB>, PageTableFlags)> {
        let phys_offset = crate::memory::physical_memory_offset();
        let virt_addr = page.start_address();

        unsafe {
            let l4_virt = phys_offset + self.level_4_frame.start_address().as_u64();
            let l4_table = &*(l4_virt.as_ptr() as *const PageTable);

            let l4_idx = (virt_addr.as_u64() >> 39) & 0x1FF;
            let l4_entry = &l4_table[l4_idx as usize];
            if l4_entry.is_unused() || !l4_entry.flags().contains(PageTableFlags::PRESENT) {
                return None;
            }

            let l3_virt = phys_offset + l4_entry.addr().as_u64();
            let l3_table = &*(l3_virt.as_ptr() as *const PageTable);

            let l3_idx = (virt_addr.as_u64() >> 30) & 0x1FF;
            let l3_entry = &l3_table[l3_idx as usize];
            if l3_entry.is_unused() || !l3_entry.flags().contains(PageTableFlags::PRESENT) {
                return None;
            }

            // 1GB huge page - not supported for CoW
            if l3_entry.flags().contains(PageTableFlags::HUGE_PAGE) {
                return None;
            }

            let l2_virt = phys_offset + l3_entry.addr().as_u64();
            let l2_table = &*(l2_virt.as_ptr() as *const PageTable);

            let l2_idx = (virt_addr.as_u64() >> 21) & 0x1FF;
            let l2_entry = &l2_table[l2_idx as usize];
            if l2_entry.is_unused() || !l2_entry.flags().contains(PageTableFlags::PRESENT) {
                return None;
            }

            // 2MB huge page - not supported for CoW
            if l2_entry.flags().contains(PageTableFlags::HUGE_PAGE) {
                return None;
            }

            let l1_virt = phys_offset + l2_entry.addr().as_u64();
            let l1_table = &*(l1_virt.as_ptr() as *const PageTable);

            let l1_idx = (virt_addr.as_u64() >> 12) & 0x1FF;
            let l1_entry = &l1_table[l1_idx as usize];
            if l1_entry.is_unused() || !l1_entry.flags().contains(PageTableFlags::PRESENT) {
                return None;
            }

            Some((
                PhysFrame::containing_address(l1_entry.addr()),
                l1_entry.flags(),
            ))
        }
    }

    /// Translate a virtual address to physical address
    #[allow(dead_code)]
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
                    // TEMPORARILY DISABLED: Too verbose, causes kernel hang
                    // log::debug!("translate_page({:#x}) -> None (FAILED)", addr.as_u64());

                    // Let's manually check the page table entries to debug
                    // TEMPORARILY DISABLED: Too verbose
                    if false {
                    unsafe {
                        let phys_offset = crate::memory::physical_memory_offset();
                        let l4_table = {
                            let virt = phys_offset + self.level_4_frame.start_address().as_u64();
                            &*(virt.as_ptr() as *const PageTable)
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
                                as *const PageTable);

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
                                    as *const PageTable);

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
                                        as *const PageTable);

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
                    } // End of disabled debug block
                }
            }
        }

        result
    }

    /// Get a reference to the mapper
    #[allow(dead_code)]
    pub fn mapper(&mut self) -> &mut OffsetPageTable<'static> {
        &mut self.mapper
    }

    /// Allocate a stack in this process's page table
    #[allow(dead_code)]
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
    #[allow(dead_code)]
    pub fn clear_userspace_for_exec(&mut self) -> Result<(), &'static str> {
        log::debug!("clear_userspace_for_exec: Clearing common userspace regions");

        // Clear the standard userspace regions that programs typically use
        // This prevents "page already mapped" errors when loading ELF files

        // 1. Clear code/data region (USERSPACE_BASE - USERSPACE_BASE + 64KB)
        let code_start = VirtAddr::new(crate::memory::layout::USERSPACE_BASE);
        let code_end = VirtAddr::new(crate::memory::layout::USERSPACE_BASE + 0x10000);
        match self.unmap_user_pages(code_start, code_end) {
            Ok(()) => log::debug!("Cleared code region {:#x}-{:#x}", code_start, code_end),
            Err(e) => log::warn!("Failed to clear code region: {}", e),
        }

        // 2. Clear user stack region if it exists
        // Use centralized stack region boundaries from layout
        let stack_bottom = VirtAddr::new(crate::memory::layout::USER_STACK_REGION_START);
        let stack_top = VirtAddr::new(crate::memory::layout::USER_STACK_REGION_END);
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
#[allow(dead_code)]
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
#[allow(dead_code)]
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
    // Use the master kernel PML4 which has all kernel mappings including stacks.
    // The bootloader's KERNEL_PAGE_TABLE_FRAME (0x101000) doesn't have the
    // kernel stack region at 0xffffc90000000000 mapped, which would cause
    // a page fault when switching.
    if let Some(kernel_frame) = crate::memory::kernel_page_table::master_kernel_pml4() {
        let (current_frame, flags) = Cr3::read();
        if current_frame != kernel_frame {
            log::trace!(
                "Switching to master kernel PML4: {:?} -> {:?}",
                current_frame,
                kernel_frame
            );
            Cr3::write(kernel_frame, flags);
            // Ensure TLB consistency after page table switch
            super::tlb::flush_after_page_table_switch();
        }
    } else {
        log::error!("Master kernel PML4 not initialized!");
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

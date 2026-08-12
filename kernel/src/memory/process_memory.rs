//! Per-process memory management
//!
//! This module provides per-process page tables and address space isolation.

#[cfg(not(target_arch = "x86_64"))]
use crate::memory::arch_stub::{
    Cr3, FrameAllocator, Mapper, OffsetPageTable, Page, PageTable, PageTableFlags, PhysAddr,
    PhysFrame, Size4KiB, Translate, VirtAddr,
};
#[cfg(target_arch = "aarch64")]
use crate::memory::frame_allocator::allocate_frame;
#[cfg(target_arch = "aarch64")]
use crate::memory::frame_allocator::return_lease;
use crate::memory::frame_allocator::{
    acquire_leaf_mapping, allocate_frame_leased, deallocate_leaf_frame, FrameLease,
    LeafMappingClass, ReturnOutcome,
};
use crate::memory::layout::USER_STACK_REGION_END;
#[cfg(target_arch = "x86_64")]
use x86_64::{
    registers::control::Cr3,
    structures::paging::{
        mapper::TranslateResult, FrameAllocator, Mapper, OffsetPageTable, Page, PageTable,
        PageTableFlags, PhysFrame, Size4KiB, Translate,
    },
    PhysAddr, VirtAddr,
};

use alloc::vec::Vec;

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
///
/// On x86_64: Contains a full PML4 with kernel mappings copied from the master.
/// On ARM64: Contains only a TTBR0 L0 table for userspace; kernel uses TTBR1 automatically.
pub struct ProcessPageTable {
    /// Physical frame containing the L0/PML4 page table
    /// On x86_64: This is the PML4 frame loaded into CR3
    /// On ARM64: This is the L0 frame loaded into TTBR0_EL1
    level_4_frame: PhysFrame,
    /// The mapper for this page table
    mapper: OffsetPageTable<'static>,
    /// Allocator authority for the root. PR-1c consumes this after proving the
    /// address space is no longer live; PR-1b intentionally never returns it.
    #[cfg_attr(not(target_arch = "aarch64"), allow(dead_code))]
    root_lease: FrameLease,
    /// Allocation-derived custody for every intermediate table we created.
    tables: OwnedTableFrames,
    /// One allocation-derived custody record per user virtual page.
    leaves: OwnedLeafFrames,
}

#[derive(Clone, Copy)]
enum LeafMapping {
    Owned,
    External,
}

#[derive(Clone, Copy)]
struct LeafRecord {
    page: u64,
    mapping: LeafMapping,
}

struct OwnedLeafFrames {
    records: Vec<LeafRecord>,
    released: bool,
}

impl OwnedLeafFrames {
    fn new() -> Self {
        Self {
            records: Vec::new(),
            released: false,
        }
    }

    fn search(&self, page: u64) -> Result<usize, usize> {
        self.records
            .binary_search_by_key(&page, |record| record.page)
    }
}

struct OwnedTableFrames {
    leases: Vec<FrameLease>,
    disposition: Disposition,
}

impl OwnedTableFrames {
    fn new() -> Self {
        Self {
            leases: Vec::new(),
            disposition: Disposition::Undecided,
        }
    }

    fn record(&mut self, lease: FrameLease) {
        self.leases.push(lease);
        crate::trace_count!(crate::tracing::providers::teardown::PT_TABLE_FRAMES_RECORDED);
    }
}

struct TableRecorder<'a>(&'a mut OwnedTableFrames);

unsafe impl FrameAllocator<Size4KiB> for TableRecorder<'_> {
    fn allocate_frame(&mut self) -> Option<PhysFrame> {
        let lease = allocate_frame_leased()?;
        let frame = lease.frame();
        self.0.record(lease);
        Some(frame)
    }
}

#[derive(Clone, Copy)]
enum Disposition {
    Undecided,
    #[cfg(target_arch = "aarch64")]
    Retiring,
    #[cfg(target_arch = "aarch64")]
    Retired,
    #[cfg(target_arch = "x86_64")]
    RetiredByExecWalk,
    Abandoned(AbandonReason),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(target_arch = "aarch64")]
pub(crate) enum RetireProgress {
    Complete,
    Budgeted,
}

#[cfg(target_arch = "aarch64")]
pub(crate) const RETIRE_FRAME_BUDGET: u32 = 64;

/// Owns an aarch64 address space until exec publishes it in a Process row.
/// Failure drops release leaves first, then tables and the root. This needs no
/// hardware-liveness proof because the table has never been installed in TTBR0.
#[cfg(target_arch = "aarch64")]
pub(crate) struct UnpublishedPageTable {
    page_table: Option<alloc::boxed::Box<ProcessPageTable>>,
    pid: u64,
}

#[cfg(target_arch = "aarch64")]
impl UnpublishedPageTable {
    pub(crate) fn new(page_table: ProcessPageTable, pid: u64) -> Self {
        Self {
            page_table: Some(alloc::boxed::Box::new(page_table)),
            pid,
        }
    }

    pub(crate) fn as_mut(&mut self) -> &mut ProcessPageTable {
        self.page_table.as_deref_mut().expect("unpublished table")
    }

    pub(crate) fn publish(mut self) -> alloc::boxed::Box<ProcessPageTable> {
        self.page_table.take().expect("unpublished table")
    }
}

#[cfg(target_arch = "aarch64")]
impl core::ops::Deref for UnpublishedPageTable {
    type Target = ProcessPageTable;

    fn deref(&self) -> &Self::Target {
        self.page_table.as_deref().expect("unpublished table")
    }
}

#[cfg(target_arch = "aarch64")]
impl core::ops::DerefMut for UnpublishedPageTable {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.as_mut()
    }
}

#[cfg(target_arch = "aarch64")]
impl Drop for UnpublishedPageTable {
    fn drop(&mut self) {
        if let Some(page_table) = self.page_table.as_deref_mut() {
            page_table.release_mapped_leaves();
            let mut budget = u32::MAX;
            let _ = page_table.retire_bounded(self.pid, &mut budget);
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum AbandonReason {
    #[cfg_attr(not(target_arch = "aarch64"), allow(dead_code))]
    NoProofPipeline,
    #[cfg_attr(target_arch = "aarch64", allow(dead_code))]
    NoArchPipeline,
    AlreadyTerminated,
}

impl ProcessPageTable {
    /// Create a new page table for a process (ARM64 version)
    ///
    /// On ARM64, this is much simpler than x86_64 because:
    /// - TTBR1_EL1 handles all kernel mappings automatically
    /// Create a new page table for ARM64 process
    ///
    /// IMPORTANT: The kernel is mapped in the higher half via TTBR1.
    /// TTBR0 is reserved for userspace mappings only, so new process page tables
    /// must start empty and allow the mapper to create L0 entries on demand.
    #[cfg(target_arch = "aarch64")]
    pub fn new() -> Result<Self, &'static str> {
        log::debug!("ProcessPageTable::new() [ARM64] - Creating userspace page table");

        // Allocate a frame for the L0 page table (TTBR0)
        let root_lease = match allocate_frame_leased() {
            Some(lease) => {
                log::debug!(
                    "ARM64: Allocated L0 frame for TTBR0: {:#x}",
                    lease.frame().start_address().as_u64()
                );
                lease
            }
            None => {
                log::error!("ARM64: Frame allocator returned None - out of memory?");
                return Err("Failed to allocate frame for page table");
            }
        };
        let l0_frame = root_lease.frame();

        // Get physical memory offset
        let phys_offset = crate::memory::physical_memory_offset();

        // Initialize a fresh TTBR0 L0 table for userspace mappings.
        let l0_table = unsafe {
            let virt = phys_offset + l0_frame.start_address().as_u64();
            &mut *(virt.as_mut_ptr() as *mut PageTable)
        };

        // First, zero out all entries
        for i in 0..512 {
            l0_table[i].set_unused();
        }

        // NOTE: Do not copy boot TTBR0 entries (kernel/device mappings).
        // Userspace mappings may reside in high TTBR0 regions (e.g. L0[511]),
        // and we want the mapper to allocate those tables as needed.

        // Create mapper for the new page table
        let mapper = unsafe {
            let l0_table_ptr = {
                let virt = phys_offset + l0_frame.start_address().as_u64();
                &mut *(virt.as_mut_ptr() as *mut PageTable)
            };
            OffsetPageTable::new(l0_table_ptr, phys_offset)
        };

        let page_table = ProcessPageTable {
            level_4_frame: l0_frame,
            mapper,
            root_lease,
            tables: OwnedTableFrames::new(),
            leaves: OwnedLeafFrames::new(),
        };

        log::debug!("ARM64: ProcessPageTable created successfully");
        Ok(page_table)
    }

    /// Create a new page table for a process (x86_64 version)
    ///
    /// This creates a new level 4 page table with kernel mappings copied
    /// from the current page table.
    #[cfg(target_arch = "x86_64")]
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
        let root_lease = match allocate_frame_leased() {
            Some(lease) => {
                let frame_addr = lease.frame().start_address().as_u64();
                log::debug!("Successfully allocated frame: {:#x}", frame_addr);

                // Check for problematic frames
                if frame_addr == 0x611000 {
                    log::error!(
                        "WARNING: Allocated frame 0x611000 which is already in use by a process!"
                    );
                }

                lease
            }
            None => {
                log::error!("Frame allocator returned None - out of memory?");
                return Err("Failed to allocate frame for page table");
            }
        };
        let level_4_frame = root_lease.frame();
        let mut tables = OwnedTableFrames::new();

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
                    if i <= 260 || i >= 509 {
                        // First few and last few
                        log::debug!(
                            "  Kernel PML4[{}]: phys={:#x}, flags={:?}",
                            i,
                            addr.as_u64(),
                            flags
                        );
                    }

                    // CRITICAL: Validate that the entry has a valid physical address
                    // An entry with PRESENT but addr=0x0 is invalid and would cause crashes
                    if flags.contains(PageTableFlags::PRESENT) && addr.as_u64() == 0 {
                        log::warn!(
                            "PML4[{}] has PRESENT flag but invalid address 0x0, skipping",
                            i
                        );
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
            log::debug!(
                "Found {} valid upper-half kernel PML4 entries (256-511)",
                valid_upper_entries
            );
            log::debug!(
                "Copied {} upper-half kernel PML4 entries (256-511)",
                kernel_entries_count
            );

            // PHASE 2: Use master kernel PML4 if available
            if let Some(master_pml4_frame) = crate::memory::kernel_page_table::master_kernel_pml4()
            {
                log::info!("PHASE2: Using master kernel PML4 for process creation");

                // Copy upper-half entries from master instead of current
                let master_pml4_virt = phys_offset + master_pml4_frame.start_address().as_u64();
                let master_pml4 = &*(master_pml4_virt.as_ptr() as *const PageTable);

                // Log what we're about to copy for critical entries
                log::info!(
                    "PHASE2-DEBUG: Reading master PML4 from virtual address {:p}",
                    master_pml4
                );
                log::info!(
                    "PHASE2-DEBUG: Master PML4[402] = {:?}",
                    master_pml4[402].frame()
                );
                log::info!(
                    "PHASE2-DEBUG: Master PML4[403] = {:?}",
                    master_pml4[403].frame()
                );
                log::info!(
                    "PHASE2-DEBUG: &master_pml4[403] is at {:p}",
                    &master_pml4[403]
                );

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
                            log::trace!(
                                "Skipping PML4[{}] - has USER_ACCESSIBLE flag (userspace region)",
                                i
                            );
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
                let fresh_pdpt_lease =
                    allocate_frame_leased().ok_or("Failed to allocate PDPT for PML4[0]")?;
                let fresh_pdpt_frame = fresh_pdpt_lease.frame();
                tables.record(fresh_pdpt_lease);

                // Map and zero out the fresh PDPT
                let fresh_pdpt_virt = phys_offset + fresh_pdpt_frame.start_address().as_u64();
                let fresh_pdpt = &mut *(fresh_pdpt_virt.as_mut_ptr() as *mut PageTable);
                for i in 0..512 {
                    fresh_pdpt[i].set_unused();
                }

                // Install the fresh PDPT in PML4[0] with user-accessible flags
                // The intermediate page tables need USER_ACCESSIBLE for userspace to work
                let pml4_0_flags = PageTableFlags::PRESENT
                    | PageTableFlags::WRITABLE
                    | PageTableFlags::USER_ACCESSIBLE;
                level_4_table[0].set_addr(fresh_pdpt_frame.start_address(), pml4_0_flags);
                log::info!(
                    "PHASE2: Created fresh PDPT for PML4[0] at frame {:#x}",
                    fresh_pdpt_frame.start_address().as_u64()
                );

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
                                let master_frame: PhysFrame<Size4KiB> =
                                    master_pml4[i].frame().unwrap();
                                let copied_frame: PhysFrame<Size4KiB> =
                                    level_4_table[i].frame().unwrap();
                                log::info!(
                                    "PHASE2: PML4[402] (kernel stacks): master={:?}, copied={:?}",
                                    master_frame,
                                    copied_frame
                                );
                                if master_frame != copied_frame {
                                    log::error!("ERROR: Frame mismatch for PML4[402]!");
                                }
                            }
                            403 => {
                                let master_frame: PhysFrame<Size4KiB> =
                                    master_pml4[i].frame().unwrap();
                                let copied_frame: PhysFrame<Size4KiB> =
                                    level_4_table[i].frame().unwrap();
                                log::info!(
                                    "PHASE2: PML4[403] (IST stacks): master={:?}, copied={:?}",
                                    master_frame,
                                    copied_frame
                                );
                                if master_frame != copied_frame {
                                    log::error!("ERROR: Frame mismatch for PML4[403]!");
                                }
                            }
                            510 => {
                                if !master_pml4[i].is_unused() {
                                    let master_frame: PhysFrame<Size4KiB> =
                                        master_pml4[i].frame().unwrap();
                                    let copied_frame: PhysFrame<Size4KiB> =
                                        level_4_table[i].frame().unwrap();
                                    log::info!(
                                        "PHASE2: PML4[510]: master={:?}, copied={:?}",
                                        master_frame,
                                        copied_frame
                                    );
                                }
                            }
                            511 => {
                                let master_frame: PhysFrame<Size4KiB> =
                                    master_pml4[i].frame().unwrap();
                                let copied_frame: PhysFrame<Size4KiB> =
                                    level_4_table[i].frame().unwrap();
                                log::info!("PHASE2: PML4[511] (kernel high-half): master={:?}, copied={:?}",
                                         master_frame, copied_frame);
                            }
                            _ => {}
                        }
                    }
                }
                log::info!(
                    "PHASE2: Inherited {} upper-half kernel mappings (256-511) from master PML4",
                    upper_half_copied
                );

                // INVARIANT ASSERTION: Kernel stacks and IST stacks must be in different frames
                // This catches bugs where PML4[402] and PML4[403] alias to the same PDPT,
                // which causes stack corruption during exception handling
                let pml4_402_frame: Result<PhysFrame<Size4KiB>, _> = level_4_table[402].frame();
                let pml4_403_frame: Result<PhysFrame<Size4KiB>, _> = level_4_table[403].frame();

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
                    log::info!(
                        "✓ INVARIANT OK: PML4[402]={:?} != PML4[403]={:?}",
                        f402,
                        f403
                    );
                } else {
                    log::warn!(
                        "INVARIANT CHECK: Missing kernel stack entries - [402]={:?}, [403]={:?}",
                        pml4_402_frame,
                        pml4_403_frame
                    );
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
                log::info!(
                    "PHASE3: Skipping manual identity mapping - PML4[0] already copied from master"
                );

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
                for i in 0..256 {
                    // Include entry 0 for kernel code at 0x100000
                    if !current_l4_table[i].is_unused() {
                        let addr = current_l4_table[i].addr();
                        let flags = current_l4_table[i].flags();

                        // CRITICAL: Validate that the entry has a valid physical address
                        // An entry with PRESENT but addr=0x0 is invalid and would cause crashes
                        if flags.contains(PageTableFlags::PRESENT) && addr.as_u64() == 0 {
                            log::warn!(
                                "PML4[{}] has PRESENT flag but invalid address 0x0, skipping",
                                i
                            );
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
                            log::debug!(
                                "Copied low-memory kernel PML4 entry {} (phys={:#x}, flags={:?})",
                                i,
                                addr.as_u64(),
                                flags
                            );
                        }
                    }
                }

                log::debug!(
                    "Process page table created with {} kernel entries ({} low + {} high)",
                    kernel_entries_count + low_kernel_entries,
                    low_kernel_entries,
                    kernel_entries_count
                );

                // CRITICAL: Ensure kernel stacks are mapped (Phase 1)
                // The kernel stacks are at 0xffffc90000000000 range
                // This is PML4 entry 402 (0xffffc90000000000 >> 39 = 402)
                let kernel_stack_pml4_idx = 402;
                if !current_l4_table[kernel_stack_pml4_idx].is_unused() {
                    // CURSOR AGENT FIX: Set proper flags for kernel stack mapping
                    let mut stack_flags = current_l4_table[kernel_stack_pml4_idx].flags();
                    stack_flags.remove(PageTableFlags::USER_ACCESSIBLE);
                    stack_flags.insert(PageTableFlags::GLOBAL);
                    level_4_table[kernel_stack_pml4_idx]
                        .set_addr(current_l4_table[kernel_stack_pml4_idx].addr(), stack_flags);
                    log::info!(
                        "PHASE1: Fixed kernel stack PML4[{}] flags (0xffffc90000000000)",
                        kernel_stack_pml4_idx
                    );
                } else {
                    log::warn!(
                        "PHASE1: Kernel stack PML4[{}] not present in current table!",
                        kernel_stack_pml4_idx
                    );
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
                    level_4_table[ist_stack_pml4_idx]
                        .set_addr(current_l4_table[ist_stack_pml4_idx].addr(), ist_flags);
                    log::info!(
                        "PHASE1: Fixed IST stack PML4[{}] flags (0xffffc98000000000)",
                        ist_stack_pml4_idx
                    );
                } else {
                    log::warn!(
                        "PHASE1: IST stack PML4[{}] not present in current table!",
                        ist_stack_pml4_idx
                    );
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
            root_lease,
            tables,
            leaves: OwnedLeafFrames::new(),
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

            // Walk userspace L4 entries (x86_64: 0-255; ARM64 TTBR0 uses 0-511)
            #[cfg(target_arch = "x86_64")]
            let l4_range = 0..256u64;
            #[cfg(target_arch = "aarch64")]
            let l4_range = 0..512u64;
            for l4_idx in l4_range {
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
                        if l2_entry.is_unused()
                            || !l2_entry.flags().contains(PageTableFlags::PRESENT)
                        {
                            continue;
                        }

                        // Check for 2MB huge page
                        if l2_entry.flags().contains(PageTableFlags::HUGE_PAGE) {
                            // 2MB huge page - calculate virtual address
                            let virt_addr =
                                VirtAddr::new((l4_idx << 39) | (l3_idx << 30) | (l2_idx << 21));
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
                            if l1_entry.is_unused()
                                || !l1_entry.flags().contains(PageTableFlags::PRESENT)
                            {
                                continue;
                            }

                            // 4KB page - calculate virtual address
                            let virt_addr = VirtAddr::new(
                                (l4_idx << 39) | (l3_idx << 30) | (l2_idx << 21) | (l1_idx << 12),
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

            // First, ensure we're not trying to map kernel/non-canonical addresses as user pages.
            // Use the arch-specific canonical boundary instead of an x86_64 constant.
            let page_addr = page.start_address().as_u64();
            if page_addr >= USER_STACK_REGION_END && flags.contains(PageTableFlags::USER_ACCESSIBLE)
            {
                log::error!(
                    "Attempting to map kernel address {:#x} as user-accessible!",
                    page_addr
                );
                return Err("Cannot map kernel addresses as user-accessible");
            }
            let user_mapping = flags.contains(PageTableFlags::USER_ACCESSIBLE);

            // CRITICAL FIX: Check if page is already mapped before attempting to map
            // This handles the case where L3 tables are shared between processes
            if let Ok(existing_frame) = self.mapper.translate_page(page) {
                if existing_frame == frame {
                    if user_mapping {
                        let Ok(_) = self.leaves.search(page_addr) else {
                            crate::trace_count!(
                                crate::tracing::providers::teardown::LEAF_CUSTODY_REFUSED
                            );
                            return Err("Mapped user page has no leaf custody record");
                        };
                    }
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

            let leaf_insert = if user_mapping {
                match self.leaves.search(page_addr) {
                    Ok(_) => {
                        crate::trace_count!(
                            crate::tracing::providers::teardown::LEAF_CUSTODY_REFUSED
                        );
                        return Err("Leaf custody record exists without a mapped descriptor");
                    }
                    Err(index) => {
                        self.leaves
                            .records
                            .try_reserve(1)
                            .map_err(|_| "Failed to reserve leaf custody")?;
                        let mapping = match acquire_leaf_mapping(frame) {
                            Ok(LeafMappingClass::Owned) => LeafMapping::Owned,
                            Ok(LeafMappingClass::External) => LeafMapping::External,
                            Err(error) => return Err(error),
                        };
                        self.leaves.records.insert(
                            index,
                            LeafRecord {
                                page: page_addr,
                                mapping,
                            },
                        );
                        Some((index, mapping))
                    }
                }
            } else {
                None
            };

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

            let map_result = {
                let Self { mapper, tables, .. } = self;
                mapper.map_to_with_table_flags(
                    page,
                    frame,
                    flags,
                    table_flags,
                    &mut TableRecorder(tables),
                )
            };
            match map_result {
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

                    if leaf_insert.is_some() {
                        self.leaves.released = false;
                        crate::trace_count!(
                            crate::tracing::providers::teardown::LEAF_MAPPINGS_RECORDED
                        );
                    }

                    log::trace!("mapper.map_to succeeded, TLB flush deferred");
                    Ok(())
                }
                Err(e) => {
                    if let Some((index, mapping)) = leaf_insert {
                        self.leaves.records.remove(index);
                        if let LeafMapping::Owned = mapping {
                            let _ = crate::memory::frame_metadata::frame_decref(frame);
                        }
                    }
                    // Enhanced error logging to understand map_to failures
                    #[cfg(not(target_arch = "x86_64"))]
                    use crate::memory::arch_stub::mapper::MapToError;
                    #[cfg(target_arch = "x86_64")]
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
    #[allow(dead_code)]
    pub fn unmap_page(
        &mut self,
        page: Page<Size4KiB>,
    ) -> Result<PhysFrame<Size4KiB>, &'static str> {
        let page_addr = page.start_address().as_u64();
        let record_index = self.leaves.search(page_addr).map_err(|_| {
            crate::trace_count!(crate::tracing::providers::teardown::LEAF_CUSTODY_REFUSED);
            "Mapped user page has no releasable leaf custody"
        })?;
        let record = self.leaves.records[record_index];
        let (frame, flush) = self
            .mapper
            .unmap(page)
            .map_err(|_| "Failed to unmap page")?;
        // Don't flush immediately - same reasoning as map_page
        let _ = flush;
        self.leaves.records.remove(record_index);
        Self::release_leaf_record(record, frame);
        Ok(frame)
    }

    fn release_leaf_record(record: LeafRecord, frame: PhysFrame) {
        crate::trace_count!(crate::tracing::providers::teardown::LEAF_MAPPINGS_RELEASED);
        if let LeafMapping::Owned = record.mapping {
            if crate::memory::frame_metadata::frame_decref(frame)
                && deallocate_leaf_frame(frame) == ReturnOutcome::Returned
            {
                crate::trace_count!(crate::tracing::providers::teardown::LEAF_FRAMES_RETURNED);
            }
        }
    }

    /// Release mapped user leaves exactly once while the caller owns a dead or
    /// never-published address space. The record proves custody of the virtual
    /// mapping; its current descriptor supplies the frame identity so a CoW
    /// replacement does not change the record's ownership key.
    pub(crate) fn release_mapped_leaves(&mut self) {
        if self.leaves.released {
            return;
        }
        let records = &self.leaves.records;
        let _ = self.walk_mapped_pages(|virt_addr, phys_addr, flags| {
            if !flags.contains(PageTableFlags::USER_ACCESSIBLE) {
                return;
            }
            let page = virt_addr.as_u64() & !0xfff;
            let frame = PhysFrame::containing_address(phys_addr);
            match records.binary_search_by_key(&page, |record| record.page) {
                Ok(index) => Self::release_leaf_record(records[index], frame),
                Err(_) => {
                    crate::trace_count!(crate::tracing::providers::teardown::LEAF_CUSTODY_REFUSED);
                }
            }
        });
        self.leaves.records.clear();
        self.leaves.released = true;
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
            let Self { mapper, tables, .. } = self;
            mapper
                .map_to_with_table_flags(
                    page,
                    frame,
                    new_flags,
                    table_flags,
                    &mut TableRecorder(tables),
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
                                let virt =
                                    phys_offset + self.level_4_frame.start_address().as_u64();
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
                                let l3_table = &*(l3_virt.as_ptr() as *const PageTable);

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
                                    let l2_table = &*(l2_virt.as_ptr() as *const PageTable);

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
                                        let l1_table = &*(l1_virt.as_ptr() as *const PageTable);

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
            if self.mapper.translate_page(page).is_err() {
                continue;
            }
            match self.unmap_page(page) {
                Ok(frame) => {
                    // Don't flush immediately - the page table switch will handle it
                    log::trace!(
                        "Unmapped page {:#x} (was mapped to frame {:#x})",
                        page.start_address().as_u64(),
                        frame.start_address().as_u64()
                    );
                }
                Err(_) => return Err("Mapped user page could not release leaf custody"),
            }
        }

        Ok(())
    }

    /// Classify an address space that this PR cannot reclaim. This method is
    /// intentionally limited to disposition accounting and returns no frames.
    pub(crate) fn abandon(mut self, reason: AbandonReason) {
        self.tables.disposition = Disposition::Abandoned(reason);
        match reason {
            AbandonReason::NoProofPipeline => {
                crate::trace_count!(
                    crate::tracing::providers::teardown::PT_ROOT_ABANDONED_NO_PROOF
                );
            }
            AbandonReason::NoArchPipeline => {
                crate::trace_count!(crate::tracing::providers::teardown::PT_ROOT_ABANDONED_NO_ARCH);
            }
            AbandonReason::AlreadyTerminated => {
                crate::trace_count!(
                    crate::tracing::providers::teardown::PT_ROOT_ABANDONED_TERMINATED
                );
            }
        }
    }

    /// Return only allocator-issued table leases after the caller has proved
    /// this address space is no longer live. Work is resumable and the root is
    /// always attempted last, after every intermediate-table lease is consumed.
    #[cfg(target_arch = "aarch64")]
    pub(crate) fn retire_bounded(&mut self, pid: u64, budget: &mut u32) -> RetireProgress {
        match self.tables.disposition {
            Disposition::Retired => return RetireProgress::Complete,
            Disposition::Undecided => {
                crate::tracing::providers::teardown::record_pt_retire_started(
                    pid,
                    self.tables.leases.len() as u64,
                );
                self.tables.disposition = Disposition::Retiring;
            }
            Disposition::Retiring => {}
            Disposition::Abandoned(_) => {
                return RetireProgress::Complete;
            }
        }

        while *budget > 0 {
            *budget -= 1;
            let outcome = match self.tables.leases.pop() {
                Some(lease) => return_lease(lease),
                None => {
                    let outcome = return_lease(self.root_lease);
                    match outcome {
                        ReturnOutcome::Returned => {
                            crate::tracing::providers::teardown::record_pt_frame_returned(pid)
                        }
                        ReturnOutcome::LostContended => {
                            crate::tracing::providers::teardown::record_pt_frame_lost(pid)
                        }
                        ReturnOutcome::RefusedDoubleRelease
                        | ReturnOutcome::RefusedStale
                        | ReturnOutcome::RefusedNeverAllocated
                        | ReturnOutcome::RefusedUntracked
                        | ReturnOutcome::RefusedLiveLeaf => {}
                    }
                    self.tables.disposition = Disposition::Retired;
                    crate::tracing::providers::teardown::record_pt_root_retired(pid);
                    return RetireProgress::Complete;
                }
            };
            match outcome {
                ReturnOutcome::Returned => {
                    crate::tracing::providers::teardown::record_pt_frame_returned(pid)
                }
                ReturnOutcome::LostContended => {
                    crate::tracing::providers::teardown::record_pt_frame_lost(pid)
                }
                ReturnOutcome::RefusedDoubleRelease
                | ReturnOutcome::RefusedStale
                | ReturnOutcome::RefusedNeverAllocated
                | ReturnOutcome::RefusedUntracked
                | ReturnOutcome::RefusedLiveLeaf => {}
            }
        }

        RetireProgress::Budgeted
    }

    /// Clean up page table resources during exec()
    ///
    /// This walks the entire page table hierarchy and:
    /// 1. Decrements reference counts for all user pages (CoW support)
    /// 2. Deallocates frames that are no longer shared (refcount=0)
    /// 3. Deallocates the page table structure frames (L1/L2/L3 tables)
    /// 4. Deallocates the L4 frame itself
    ///
    /// Call this on the OLD page table after installing the new one during exec().
    #[cfg(target_arch = "x86_64")]
    pub(crate) fn cleanup_for_exec(mut self) {
        use crate::memory::frame_allocator::deallocate_frame;
        use crate::memory::frame_metadata::frame_decref;
        use alloc::vec::Vec;

        self.tables.disposition = Disposition::RetiredByExecWalk;
        crate::trace_count_add!(
            crate::tracing::providers::teardown::PT_EXEC_WALK_LEASES_UNRETURNED,
            self.tables.leases.len() as u64
        );

        let phys_offset = crate::memory::physical_memory_offset();
        let mut user_frames_freed = 0u64;
        let mut user_frames_still_shared = 0u64;
        let mut table_frames_freed = 0u64;

        // Collect page table structure frames to free after walking
        let mut l3_frames: Vec<PhysFrame> = Vec::new();
        let mut l2_frames: Vec<PhysFrame> = Vec::new();
        let mut l1_frames: Vec<PhysFrame> = Vec::new();

        unsafe {
            // Get the L4 table
            let l4_virt = phys_offset + self.level_4_frame.start_address().as_u64();
            let l4_table = &*(l4_virt.as_ptr() as *const PageTable);

            // Walk L4 entries 0-255 (userspace only, 256-511 is kernel - don't touch)
            for l4_idx in 0..256usize {
                let l4_entry = &l4_table[l4_idx];
                if l4_entry.is_unused() || !l4_entry.flags().contains(PageTableFlags::PRESENT) {
                    continue;
                }

                // Skip kernel entries in lower half (e.g. PML4[136] = kernel heap at 0x4444_4444_0000)
                // These are copied from the master page table without USER_ACCESSIBLE and are shared.
                if !l4_entry.flags().contains(PageTableFlags::USER_ACCESSIBLE) {
                    continue;
                }

                // Get L3 table and mark for cleanup
                let l3_phys = l4_entry.addr();
                let l3_frame = PhysFrame::containing_address(l3_phys);
                l3_frames.push(l3_frame);

                let l3_virt = phys_offset + l3_phys.as_u64();
                let l3_table = &*(l3_virt.as_ptr() as *const PageTable);

                for l3_idx in 0..512usize {
                    let l3_entry = &l3_table[l3_idx];
                    if l3_entry.is_unused() || !l3_entry.flags().contains(PageTableFlags::PRESENT) {
                        continue;
                    }

                    // 1GB huge page - handle CoW for the frame
                    if l3_entry.flags().contains(PageTableFlags::HUGE_PAGE) {
                        // Skip kernel identity-mapped frames (not USER_ACCESSIBLE)
                        if !l3_entry.flags().contains(PageTableFlags::USER_ACCESSIBLE) {
                            continue;
                        }
                        let frame = PhysFrame::containing_address(l3_entry.addr());
                        if frame_decref(frame) {
                            deallocate_frame(frame);
                            user_frames_freed += 1;
                        } else {
                            user_frames_still_shared += 1;
                        }
                        continue;
                    }

                    // Skip kernel-only subtrees (not USER_ACCESSIBLE)
                    // On x86-64, intermediate entries must have USER_ACCESSIBLE for
                    // any leaf pages under them to be accessible from userspace.
                    if !l3_entry.flags().contains(PageTableFlags::USER_ACCESSIBLE) {
                        continue;
                    }

                    // Get L2 table and mark for cleanup
                    let l2_phys = l3_entry.addr();
                    let l2_frame = PhysFrame::containing_address(l2_phys);
                    l2_frames.push(l2_frame);

                    let l2_virt = phys_offset + l2_phys.as_u64();
                    let l2_table = &*(l2_virt.as_ptr() as *const PageTable);

                    for l2_idx in 0..512usize {
                        let l2_entry = &l2_table[l2_idx];
                        if l2_entry.is_unused()
                            || !l2_entry.flags().contains(PageTableFlags::PRESENT)
                        {
                            continue;
                        }

                        // 2MB huge page - handle CoW for the frame
                        if l2_entry.flags().contains(PageTableFlags::HUGE_PAGE) {
                            // Skip kernel identity-mapped frames (not USER_ACCESSIBLE)
                            if !l2_entry.flags().contains(PageTableFlags::USER_ACCESSIBLE) {
                                continue;
                            }
                            let frame = PhysFrame::containing_address(l2_entry.addr());
                            if frame_decref(frame) {
                                deallocate_frame(frame);
                                user_frames_freed += 1;
                            } else {
                                user_frames_still_shared += 1;
                            }
                            continue;
                        }

                        // Skip kernel-only subtrees (not USER_ACCESSIBLE)
                        if !l2_entry.flags().contains(PageTableFlags::USER_ACCESSIBLE) {
                            continue;
                        }

                        // Get L1 table and mark for cleanup
                        let l1_phys = l2_entry.addr();
                        let l1_frame = PhysFrame::containing_address(l1_phys);
                        l1_frames.push(l1_frame);

                        let l1_virt = phys_offset + l1_phys.as_u64();
                        let l1_table = &*(l1_virt.as_ptr() as *const PageTable);

                        for l1_idx in 0..512usize {
                            let l1_entry = &l1_table[l1_idx];
                            if l1_entry.is_unused()
                                || !l1_entry.flags().contains(PageTableFlags::PRESENT)
                            {
                                continue;
                            }

                            // Skip kernel identity-mapped frames (not USER_ACCESSIBLE)
                            if !l1_entry.flags().contains(PageTableFlags::USER_ACCESSIBLE) {
                                continue;
                            }

                            // 4KB page - handle CoW for the frame
                            let frame = PhysFrame::containing_address(l1_entry.addr());
                            if frame_decref(frame) {
                                deallocate_frame(frame);
                                user_frames_freed += 1;
                            } else {
                                user_frames_still_shared += 1;
                            }
                        }
                    }
                }
            }

            // Free page table structure frames (L1 first, then L2, then L3)
            for frame in l1_frames {
                deallocate_frame(frame);
                table_frames_freed += 1;
            }
            for frame in l2_frames {
                deallocate_frame(frame);
                table_frames_freed += 1;
            }
            for frame in l3_frames {
                deallocate_frame(frame);
                table_frames_freed += 1;
            }

            // Free the L4 frame itself
            deallocate_frame(self.level_4_frame);
            table_frames_freed += 1;
        }

        log::info!(
            "cleanup_for_exec: freed {} user frames, {} still shared, {} table frames",
            user_frames_freed,
            user_frames_still_shared,
            table_frames_freed
        );
    }

    /// Release a superseded ARM64 address space from allocation-derived leaf
    /// and table custody. The caller supplies the shared retirement budget so
    /// old exec roots remain resumable with the ordinary exit pipeline.
    #[cfg(target_arch = "aarch64")]
    pub(crate) fn cleanup_for_exec(&mut self, pid: u64, budget: &mut u32) -> RetireProgress {
        self.release_mapped_leaves();
        self.retire_bounded(pid, budget)
    }
}

impl Drop for ProcessPageTable {
    fn drop(&mut self) {
        match self.tables.disposition {
            Disposition::Undecided => {
                crate::trace_count!(crate::tracing::providers::teardown::PT_ROOT_DROPPED_UNDECIDED);
            }
            #[cfg(target_arch = "aarch64")]
            Disposition::Retiring => {
                crate::trace_count!(
                    crate::tracing::providers::teardown::PT_ROOT_DROPPED_MID_RETIRE
                );
            }
            #[cfg(target_arch = "aarch64")]
            Disposition::Retired => {}
            #[cfg(target_arch = "x86_64")]
            Disposition::RetiredByExecWalk => {}
            Disposition::Abandoned(reason) => match reason {
                AbandonReason::NoProofPipeline
                | AbandonReason::NoArchPipeline
                | AbandonReason::AlreadyTerminated => {}
            },
        }
    }
}

#[cfg(feature = "boot_tests")]
fn disposition_gate_counters() -> [u64; 6] {
    use crate::tracing::providers::teardown;
    [
        teardown::PT_TABLE_FRAMES_RECORDED.aggregate(),
        teardown::PT_ROOT_ABANDONED_NO_PROOF.aggregate(),
        teardown::PT_ROOT_ABANDONED_NO_ARCH.aggregate(),
        teardown::PT_ROOT_ABANDONED_TERMINATED.aggregate(),
        teardown::PT_ROOT_DROPPED_UNDECIDED.aggregate(),
        teardown::PT_EXEC_WALK_LEASES_UNRETURNED.aggregate(),
    ]
}

#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
fn corrupt_executable_fixture() -> [u8; 180] {
    use crate::arch_impl::aarch64::elf::{
        flags, Elf64Header, Elf64ProgramHeader, SegmentType, ELFDATA2LSB, ELFCLASS64, ELF_MAGIC,
        EM_AARCH64,
    };

    let header = Elf64Header {
        magic: ELF_MAGIC,
        class: ELFCLASS64,
        data: ELFDATA2LSB,
        version: 1,
        osabi: 0,
        abiversion: 0,
        _pad: [0; 7],
        elf_type: 2,
        machine: EM_AARCH64,
        version2: 1,
        entry: 0x20_0000,
        phoff: 64,
        shoff: 0,
        flags: 0,
        ehsize: 64,
        phentsize: 56,
        phnum: 2,
        shentsize: 0,
        shnum: 0,
        shstrndx: 0,
    };
    let executable = Elf64ProgramHeader {
        p_type: SegmentType::Load as u32,
        p_flags: flags::PF_R | flags::PF_X,
        p_offset: 176,
        p_vaddr: 0x20_0000,
        p_paddr: 0,
        p_filesz: 4,
        p_memsz: 4096,
        p_align: 4096,
    };
    let corrupt = Elf64ProgramHeader {
        p_type: SegmentType::Load as u32,
        p_flags: flags::PF_R | flags::PF_X,
        p_offset: 180,
        p_vaddr: 0x40_0000,
        p_paddr: 0,
        p_filesz: 4,
        p_memsz: 4096,
        p_align: 4096,
    };

    let mut bytes = [0u8; 180];
    unsafe {
        core::ptr::copy_nonoverlapping(
            &header as *const Elf64Header as *const u8,
            bytes.as_mut_ptr(),
            core::mem::size_of::<Elf64Header>(),
        );
        core::ptr::copy_nonoverlapping(
            &executable as *const Elf64ProgramHeader as *const u8,
            bytes.as_mut_ptr().add(64),
            core::mem::size_of::<Elf64ProgramHeader>(),
        );
        core::ptr::copy_nonoverlapping(
            &corrupt as *const Elf64ProgramHeader as *const u8,
            bytes.as_mut_ptr().add(120),
            core::mem::size_of::<Elf64ProgramHeader>(),
        );
    }
    bytes[176..180].copy_from_slice(&[0x1f, 0x20, 0x03, 0xd5]);
    bytes
}

/// O2/G-H: drive the real classified-abandon and non-freeing Drop paths.
#[cfg(feature = "boot_tests")]
pub fn page_table_custody_disposition_gate_test() -> crate::test_framework::registry::TestResult {
    use crate::test_framework::registry::TestResult;

    let start = disposition_gate_counters();

    // O2/E: hold the real reuse-pool lock across real retirement. The ledger
    // transition commits ST_FREE, retirement reports loss rather than a
    // corruption refusal, and the fixture repairs the deliberately lost root.
    #[cfg(target_arch = "aarch64")]
    {
        use crate::tracing::providers::teardown;

        let mut contended = match ProcessPageTable::new() {
            Ok(page_table) => page_table,
            Err(_) => return TestResult::Fail("E: page-table construction failed"),
        };
        let root = contended.level_4_frame();
        let free_before = crate::memory::frame_allocator::free_list_len_for_gate();
        let returned_before = teardown::PT_TABLE_FRAMES_RETURNED.aggregate();
        let lost_before = teardown::PT_RETIRE_FRAMES_LOST.aggregate();
        let retired_before = teardown::PT_ROOTS_RETIRED.aggregate();
        let mut budget = RETIRE_FRAME_BUDGET;
        if crate::memory::frame_allocator::retire_with_free_list_contended(
            &mut contended,
            u64::MAX - 1,
            &mut budget,
        ) != RetireProgress::Complete
            || teardown::PT_TABLE_FRAMES_RETURNED.aggregate() != returned_before
            || teardown::PT_RETIRE_FRAMES_LOST.aggregate() != lost_before + 1
            || teardown::PT_ROOTS_RETIRED.aggregate() != retired_before + 1
            || crate::memory::frame_allocator::free_list_len_for_gate() != free_before
            || !crate::memory::frame_allocator::republish_frame_for_gate(root)
            || crate::memory::frame_allocator::free_list_len_for_gate() != free_before + 1
        {
            return TestResult::Fail("E: retirement contention was not isolated and repaired");
        }

        let used_before = {
            let stats = crate::memory::frame_allocator::memory_stats();
            stats.allocated_frames.saturating_sub(
                crate::memory::frame_allocator::free_list_len_for_gate(),
            )
        };
        let leaf_recorded_before = teardown::LEAF_MAPPINGS_RECORDED.aggregate();
        let leaf_released_before = teardown::LEAF_MAPPINGS_RELEASED.aggregate();
        let leaf_returned_before = teardown::LEAF_FRAMES_RETURNED.aggregate();
        let tables_returned_before = teardown::PT_TABLE_FRAMES_RETURNED.aggregate();
        let roots_retired_before = teardown::PT_ROOTS_RETIRED.aggregate();
        let live_refusals_before = teardown::FRAME_RETURN_REFUSED_LIVE_LEAF.aggregate();
        let custody_refusals_before = teardown::LEAF_CUSTODY_REFUSED.aggregate();
        let unregistered_before = teardown::LEAF_DECREF_UNREGISTERED.aggregate();

        let page_table = match ProcessPageTable::new() {
            Ok(page_table) => page_table,
            Err(_) => return TestResult::Fail("F4: unpublished page-table construction failed"),
        };
        let unpublished_pid = u64::MAX - 2;
        let mut unpublished = UnpublishedPageTable::new(page_table, unpublished_pid);
        let corrupt_elf = corrupt_executable_fixture();
        let failed_load = crate::arch_impl::aarch64::elf::load_elf_into_page_table(
            &corrupt_elf,
            unpublished.as_mut(),
        );
        match failed_load {
            Err("Segment data out of bounds") => {}
            _ => {
                return TestResult::Fail(
                    "F4: corrupt executable did not fail after its first mapping",
                )
            }
        }
        drop(unpublished);

        let used_after = {
            let stats = crate::memory::frame_allocator::memory_stats();
            stats.allocated_frames.saturating_sub(
                crate::memory::frame_allocator::free_list_len_for_gate(),
            )
        };
        crate::serial_println!(
            "[EXEC_FAILED_RELEASE_ORACLE:aarch64:used_before={}:used_after={}:leaf_recorded={}:leaf_released={}:leaf_returned={}:tables_returned={}:roots_retired={}:live_refused={}]",
            used_before,
            used_after,
            teardown::LEAF_MAPPINGS_RECORDED.aggregate() - leaf_recorded_before,
            teardown::LEAF_MAPPINGS_RELEASED.aggregate() - leaf_released_before,
            teardown::LEAF_FRAMES_RETURNED.aggregate() - leaf_returned_before,
            teardown::PT_TABLE_FRAMES_RETURNED.aggregate() - tables_returned_before,
            teardown::PT_ROOTS_RETIRED.aggregate() - roots_retired_before,
            teardown::FRAME_RETURN_REFUSED_LIVE_LEAF.aggregate() - live_refusals_before,
        );
        if used_after != used_before
            || teardown::LEAF_MAPPINGS_RECORDED.aggregate() != leaf_recorded_before + 1
            || teardown::LEAF_MAPPINGS_RELEASED.aggregate() != leaf_released_before + 1
            || teardown::LEAF_FRAMES_RETURNED.aggregate() != leaf_returned_before + 1
            || teardown::PT_TABLE_FRAMES_RETURNED.aggregate() != tables_returned_before + 4
            || teardown::PT_ROOTS_RETIRED.aggregate() != roots_retired_before + 1
            || teardown::FRAME_RETURN_REFUSED_LIVE_LEAF.aggregate() != live_refusals_before
            || teardown::LEAF_CUSTODY_REFUSED.aggregate() != custody_refusals_before
            || teardown::LEAF_DECREF_UNREGISTERED.aggregate() != unregistered_before
        {
            return TestResult::Fail("F4: failed exec did not release unpublished custody exactly");
        }
    }

    let terminated = match ProcessPageTable::new() {
        Ok(page_table) => page_table,
        Err(_) => return TestResult::Fail("G: page-table construction failed"),
    };
    let free_before_abandon = crate::memory::frame_allocator::free_list_len_for_gate();
    terminated.abandon(AbandonReason::AlreadyTerminated);
    let after_abandon = disposition_gate_counters();
    if after_abandon[3] != start[3] + 1
        || after_abandon[1] != start[1]
        || after_abandon[2] != start[2]
        || after_abandon[4] != start[4]
        || after_abandon[5] != start[5]
        || crate::memory::frame_allocator::free_list_len_for_gate() != free_before_abandon
    {
        return TestResult::Fail("G: terminated abandonment was not isolated and non-freeing");
    }

    let undecided = match ProcessPageTable::new() {
        Ok(page_table) => page_table,
        Err(_) => return TestResult::Fail("H: page-table construction failed"),
    };
    let free_before_drop = crate::memory::frame_allocator::free_list_len_for_gate();
    drop(undecided);
    let after_drop = disposition_gate_counters();
    if after_drop[3] != after_abandon[3]
        || after_drop[1] != after_abandon[1]
        || after_drop[2] != after_abandon[2]
        || after_drop[4] != after_abandon[4] + 1
        || after_drop[5] != after_abandon[5]
        || crate::memory::frame_allocator::free_list_len_for_gate() != free_before_drop
    {
        return TestResult::Fail("H: undecided Drop was not isolated and non-freeing");
    }

    // O3 x86 residual direction: without an architecture proof pipeline the
    // same custody object must be counted and must return no table frame.
    #[cfg(target_arch = "x86_64")]
    {
        let no_arch = match ProcessPageTable::new() {
            Ok(page_table) => page_table,
            Err(_) => return TestResult::Fail("O3: page-table construction failed"),
        };
        let returned_before =
            crate::tracing::providers::teardown::PT_TABLE_FRAMES_RETURNED.aggregate();
        no_arch.abandon(AbandonReason::NoArchPipeline);
        let after_no_arch = disposition_gate_counters();
        if after_no_arch[2] != after_drop[2] + 1
            || crate::tracing::providers::teardown::PT_TABLE_FRAMES_RETURNED.aggregate()
                != returned_before
        {
            return TestResult::Fail("O3: x86 residual abandonment returned a table frame");
        }
    }

    TestResult::Pass
}

#[cfg(all(feature = "boot_tests", target_arch = "x86_64"))]
pub fn run_x86_page_table_custody_gate() {
    crate::serial_println!("[TEST:process:page_table_custody_disposition_gate:START]");
    let result = page_table_custody_disposition_gate_test();
    if result.is_pass() {
        crate::serial_println!("[TEST:process:page_table_custody_disposition_gate:PASS]");
    } else {
        crate::serial_println!(
            "[TEST:process:page_table_custody_disposition_gate:FAIL:{:?}]",
            result
        );
    }
    let values = disposition_gate_counters();
    crate::serial_println!(
        "[PT_CUSTODY_COUNTERS:x86:recorded={}:no_proof={}:no_arch={}:terminated={}:undecided={}:exec_unreturned={}]",
        values[0], values[1], values[2], values[3], values[4], values[5]
    );
    assert!(result.is_pass(), "x86 page-table custody gate failed");
}

/// Switch to a process's page table (ARM64 version)
///
/// On ARM64, this only switches TTBR0_EL1 (userspace page table).
/// Kernel mappings in TTBR1_EL1 remain unchanged.
///
/// # Safety
/// This changes the active page table. The caller must ensure that:
/// - The new page table is valid
/// - This is called from a safe context (e.g., during interrupt return)
#[cfg(target_arch = "aarch64")]
#[allow(dead_code)]
pub unsafe fn switch_to_process_page_table(page_table: &ProcessPageTable) {
    let (current_frame, flags) = Cr3::read(); // Reads TTBR0_EL1
    let new_frame = page_table.level_4_frame();

    if current_frame != new_frame {
        log::trace!(
            "ARM64: Switching TTBR0: {:?} -> {:?}",
            current_frame,
            new_frame
        );
        Cr3::write(new_frame, flags); // Writes TTBR0_EL1 with barriers
                                      // ARM64 Cr3::write includes DSB ISH and ISB, so no separate TLB flush needed
        log::debug!("ARM64: TTBR0 switch completed");
    }
}

/// Switch to a process's page table (x86_64 version)
///
/// # Safety
/// This changes the active page table. The caller must ensure that:
/// - The new page table is valid
/// - The kernel mappings are present in the new page table
/// - This is called from a safe context (e.g., during interrupt return)
#[cfg(target_arch = "x86_64")]
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
        let stack_ptr: u64 = {
            let rsp: u64;
            core::arch::asm!("mov {}, rsp", out(reg) rsp);
            rsp
        };
        log::debug!("Current stack pointer: {:#x}", stack_ptr);

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

/// Switch back to the kernel page table (ARM64 version)
///
/// On ARM64, kernel mappings are always in TTBR1_EL1, so we don't need to
/// switch anything. This function is essentially a no-op, but we zero out
/// TTBR0 to ensure no stale userspace mappings remain active.
///
/// # Safety
/// Caller must ensure this is called from a safe context
#[cfg(target_arch = "aarch64")]
pub unsafe fn switch_to_kernel_page_table() {
    // On ARM64, TTBR1 always has kernel mappings, so no switch needed.
    // However, we could optionally zero TTBR0 to prevent any accidental
    // userspace access. For now, we just log and do nothing.
    log::trace!("ARM64: switch_to_kernel_page_table - TTBR1 always active");
}

/// Switch back to the kernel page table (x86_64 version)
///
/// # Safety
/// Caller must ensure this is called from a safe context
#[cfg(target_arch = "x86_64")]
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

    // Validate stack address range to prevent capacity overflow in Page::range_inclusive()
    if stack_bottom.as_u64() >= stack_top.as_u64() {
        log::error!(
            "Invalid stack address range: stack_bottom ({:#x}) >= stack_top ({:#x})",
            stack_bottom.as_u64(),
            stack_top.as_u64()
        );
        return Err("Invalid stack address range: stack_bottom >= stack_top");
    }

    // Calculate page range to copy
    let start_page = Page::<Size4KiB>::containing_address(stack_bottom);
    let end_page = Page::<Size4KiB>::containing_address(stack_top - 1u64);

    let mut mapped_pages = 0;

    #[cfg(target_arch = "x86_64")]
    {
        // Get access to the kernel page table
        let kernel_mapper = unsafe { crate::memory::paging::get_mapper() };

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
    }

    #[cfg(not(target_arch = "x86_64"))]
    {
        // ARM64: user addresses live in TTBR0 and must be mapped directly into
        // the process page table, not copied from the kernel (TTBR1) mappings.
        let flags =
            PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE;

        for page in Page::range_inclusive(start_page, end_page) {
            if let Some(existing_frame) = process_page_table.translate_page(page.start_address()) {
                let existing_frame = PhysFrame::<Size4KiB>::containing_address(existing_frame);
                log::trace!(
                    "User stack page {:#x} already mapped to frame {:#x}",
                    page.start_address().as_u64(),
                    existing_frame.start_address().as_u64()
                );
                mapped_pages += 1;
                continue;
            }

            let frame = match allocate_frame() {
                Some(frame) => frame,
                None => {
                    return Err("Out of memory for user stack");
                }
            };
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

    log::debug!(
        "✓ Successfully mapped {} user stack pages to process page table",
        mapped_pages
    );
    Ok(())
}

/// Map user stack to process page table using known physical addresses (ARM64).
///
/// This variant takes the physical address of the stack bottom and maps
/// the physical frames to userspace virtual addresses. This is needed on ARM64
/// where the kernel allocates stack frames via HHDM but they need to be
/// accessible at userspace addresses in TTBR0.
///
/// # Arguments
/// * `process_page_table` - The process's page table to map into
/// * `user_stack_bottom` - Userspace virtual address for stack bottom
/// * `user_stack_top` - Userspace virtual address for stack top (SP points here)
/// * `phys_bottom` - Physical address of the stack bottom
#[cfg(target_arch = "aarch64")]
pub fn map_user_stack_to_process_with_phys(
    process_page_table: &mut ProcessPageTable,
    user_stack_bottom: VirtAddr,
    user_stack_top: VirtAddr,
    phys_bottom: u64,
) -> Result<(), &'static str> {
    use crate::memory::arch_stub::{Page, PageTableFlags, PhysAddr, PhysFrame, Size4KiB};

    let stack_size = user_stack_top.as_u64() - user_stack_bottom.as_u64();
    let num_pages = stack_size / 4096;

    let flags =
        PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE;

    for i in 0..num_pages {
        let page_offset = i * 4096;
        let user_vaddr = VirtAddr::new(user_stack_bottom.as_u64() + page_offset);
        let phys_addr = PhysAddr::new(phys_bottom + page_offset);
        let page = Page::<Size4KiB>::containing_address(user_vaddr);
        let frame = PhysFrame::<Size4KiB>::containing_address(phys_addr);

        match process_page_table.map_page(page, frame, flags) {
            Ok(()) => {
                log::trace!(
                    "Mapped user stack page {:#x} -> frame {:#x}",
                    user_vaddr.as_u64(),
                    phys_addr.as_u64()
                );
            }
            Err(e) => {
                log::error!(
                    "Failed to map page {:#x} -> {:#x}: {}",
                    user_vaddr.as_u64(),
                    phys_addr.as_u64(),
                    e
                );
                return Err("Failed to map user stack page");
            }
        }
    }

    Ok(())
}

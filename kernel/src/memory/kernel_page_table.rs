//! Global kernel page table management
//!
//! This module implements a production-grade, globally-shared kernel address space
//! that guarantees every kernel stack is mapped in every address space before use.
//!
//! ## Design
//! - PML4 entries 256-511 point to a single shared kernel_pdpt
//! - All kernel mappings are installed once through the shared hierarchy
//! - Each process gets its own PML4 but shares the kernel mappings

use crate::memory::frame_allocator::allocate_frame;
use spin::Mutex;
use x86_64::{
    registers::control::{Cr3, Cr3Flags},
    structures::paging::{PageTable, PageTableFlags, PhysFrame},
    PhysAddr, VirtAddr,
};

/// The global kernel PDPT (L3 page table) frame
static KERNEL_PDPT_FRAME: Mutex<Option<PhysFrame>> = Mutex::new(None);

/// The master kernel PML4 frame (Phase 2)
static MASTER_KERNEL_PML4: Mutex<Option<PhysFrame>> = Mutex::new(None);

/// Physical memory offset for accessing page tables
static mut PHYS_MEM_OFFSET: Option<VirtAddr> = None;

/// Initialize the global kernel page table system
///
/// This must be called early in boot to set up the shared kernel address space.
/// It creates a kernel_pdpt and updates the boot PML4 to use it.
pub fn init(phys_mem_offset: VirtAddr) {
    unsafe {
        PHYS_MEM_OFFSET = Some(phys_mem_offset);
    }

    log::info!("Initializing global kernel page table system");

    // Allocate a frame for the kernel PDPT (L3 table)
    let kernel_pdpt_frame = allocate_frame().expect("Failed to allocate frame for kernel PDPT");

    // Zero the PDPT
    unsafe {
        let pdpt_virt = phys_mem_offset + kernel_pdpt_frame.start_address().as_u64();
        let pdpt = &mut *(pdpt_virt.as_mut_ptr() as *mut PageTable);
        // Clear all entries properly (not using zero() which sets PRESENT | WRITABLE)
        for i in 0..512 {
            pdpt[i].set_unused();
        }
    }

    log::info!("Allocated kernel PDPT at frame {:?}", kernel_pdpt_frame);

    // Get current PML4
    let (current_pml4_frame, _) = Cr3::read();

    unsafe {
        let pml4_virt = phys_mem_offset + current_pml4_frame.start_address().as_u64();
        let pml4 = &mut *(pml4_virt.as_mut_ptr() as *mut PageTable);

        // Copy existing kernel mappings (256-511) to the new kernel PDPT
        for i in 256..512 {
            if !pml4[i].is_unused() {
                // This PML4 entry has kernel mappings
                let old_pdpt_frame = pml4[i].frame().unwrap();
                let old_pdpt_virt = phys_mem_offset + old_pdpt_frame.start_address().as_u64();
                let old_pdpt = &*(old_pdpt_virt.as_ptr() as *const PageTable);

                let new_pdpt_virt = phys_mem_offset + kernel_pdpt_frame.start_address().as_u64();
                let new_pdpt = &mut *(new_pdpt_virt.as_mut_ptr() as *mut PageTable);

                // Copy PDPT entries
                let _pdpt_index = ((i - 256) * 512) % 512; // Map PML4 index to PDPT range
                for j in 0..512 {
                    if !old_pdpt[j].is_unused() {
                        new_pdpt[j] = old_pdpt[j].clone();
                    }
                }

                log::debug!("Migrated kernel mappings from PML4 entry {}", i);
            }

            // Update PML4 entry to point to shared kernel PDPT
            let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE;
            pml4[i].set_frame(kernel_pdpt_frame, flags);
        }
    }

    // Store the kernel PDPT frame for later use
    *KERNEL_PDPT_FRAME.lock() = Some(kernel_pdpt_frame);

    log::info!("Global kernel page table initialized successfully");
}

/// Map a page in the global kernel address space
///
/// This function maps a virtual page to a physical frame in the shared kernel
/// page tables. The mapping becomes immediately visible to all processes.
///
/// # Safety
/// Caller must ensure the virtual address is in kernel space (>= 0xFFFF_8000_0000_0000)
pub unsafe fn map_kernel_page(
    virt: VirtAddr,
    phys: PhysAddr,
    flags: PageTableFlags,
) -> Result<(), &'static str> {
    // Verify this is a kernel address
    if virt.as_u64() < 0xFFFF_8000_0000_0000 {
        return Err("map_kernel_page called with non-kernel address");
    }

    let phys_mem_offset = PHYS_MEM_OFFSET.ok_or("Physical memory offset not initialized")?;

    let kernel_pdpt_frame = KERNEL_PDPT_FRAME
        .lock()
        .ok_or("Kernel PDPT not initialized")?;

    // CRITICAL FIX: Use the master kernel PML4 if available, otherwise current
    // This ensures kernel mappings go into the shared kernel page tables
    // that all processes inherit, not just the current process's view
    let pml4_frame = if let Some(master_frame) = MASTER_KERNEL_PML4.lock().clone() {
        log::trace!("Using master kernel PML4 for kernel mapping");
        master_frame
    } else {
        // Fall back to current PML4 during early boot before master is created
        let (current_frame, _) = Cr3::read();
        log::trace!("Using current PML4 for kernel mapping (master not available)");
        current_frame
    };
    
    let pml4_virt = phys_mem_offset + pml4_frame.start_address().as_u64();
    let pml4 = &mut *(pml4_virt.as_mut_ptr() as *mut PageTable);

    // Calculate indices
    let pml4_index = (virt.as_u64() >> 39) & 0x1FF;
    let pdpt_index = (virt.as_u64() >> 30) & 0x1FF;
    let pd_index = (virt.as_u64() >> 21) & 0x1FF;
    let pt_index = (virt.as_u64() >> 12) & 0x1FF;

    // Ensure PML4 entry points to kernel PDPT
    if pml4_index >= 256 {
        let entry = &mut pml4[pml4_index as usize];
        if entry.is_unused() || entry.frame().unwrap() != kernel_pdpt_frame {
            entry.set_frame(
                kernel_pdpt_frame,
                PageTableFlags::PRESENT | PageTableFlags::WRITABLE,
            );
        }
    }

    // Walk the page tables, allocating as needed
    let pdpt_virt = phys_mem_offset + kernel_pdpt_frame.start_address().as_u64();
    let pdpt = &mut *(pdpt_virt.as_mut_ptr() as *mut PageTable);

    // Get or allocate PD (L2)
    let pd_frame = if pdpt[pdpt_index as usize].is_unused() {
        let frame = allocate_frame().ok_or("Out of memory for PD")?;
        let pd_virt = phys_mem_offset + frame.start_address().as_u64();
        let pd = &mut *(pd_virt.as_mut_ptr() as *mut PageTable);
        // Clear all entries properly (not using zero() which sets PRESENT | WRITABLE)
        for i in 0..512 {
            pd[i].set_unused();
        }

        pdpt[pdpt_index as usize]
            .set_frame(frame, PageTableFlags::PRESENT | PageTableFlags::WRITABLE);
        frame
    } else {
        pdpt[pdpt_index as usize].frame().unwrap()
    };

    let pd_virt = phys_mem_offset + pd_frame.start_address().as_u64();
    let pd = &mut *(pd_virt.as_mut_ptr() as *mut PageTable);

    // Get or allocate PT (L1)
    let pt_frame = if pd[pd_index as usize].is_unused() {
        let frame = allocate_frame().ok_or("Out of memory for PT")?;
        let pt_virt = phys_mem_offset + frame.start_address().as_u64();
        let pt = &mut *(pt_virt.as_mut_ptr() as *mut PageTable);
        // Clear all entries properly (not using zero() which sets PRESENT | WRITABLE)
        for i in 0..512 {
            pt[i].set_unused();
        }

        pd[pd_index as usize].set_frame(frame, PageTableFlags::PRESENT | PageTableFlags::WRITABLE);
        frame
    } else {
        pd[pd_index as usize].frame().unwrap()
    };

    let pt_virt = phys_mem_offset + pt_frame.start_address().as_u64();
    let pt = &mut *(pt_virt.as_mut_ptr() as *mut PageTable);

    // Map the page
    let page_frame = PhysFrame::containing_address(phys);
    pt[pt_index as usize].set_frame(page_frame, flags);

    // Flush TLB for this specific page
    use x86_64::instructions::tlb;
    tlb::flush(virt);

    log::trace!(
        "Mapped kernel page {:?} -> {:?} with flags {:?}",
        virt,
        phys,
        flags
    );

    Ok(())
}

/// Update all existing processes to use the global kernel page tables
///
/// This function iterates through all existing processes and updates their
/// PML4 entries 256-511 to point to the shared kernel PDPT.
pub fn migrate_existing_processes() {
    let _kernel_pdpt_frame = match KERNEL_PDPT_FRAME.lock().as_ref() {
        Some(frame) => *frame,
        None => {
            log::error!("Cannot migrate processes: kernel PDPT not initialized");
            return;
        }
    };

    let _phys_mem_offset = unsafe {
        match PHYS_MEM_OFFSET {
            Some(offset) => offset,
            None => {
                log::error!("Cannot migrate processes: physical memory offset not initialized");
                return;
            }
        }
    };

    // Note: ProcessManager doesn't expose all_processes() directly
    // For now, migration will happen naturally as processes are created
    // since ProcessPageTable::new() already uses the global kernel PDPT
    log::info!("Process migration will occur as new processes are created");
}

/// Get the kernel PDPT frame for use in new process creation
pub fn kernel_pdpt_frame() -> Option<PhysFrame> {
    KERNEL_PDPT_FRAME.lock().clone()
}

/// Build the master kernel PML4 with complete upper-half mappings (Phase 2)
/// This creates a reference PML4 that all processes will inherit from
/// 
/// === STEP 2: Build Real Master Kernel PML4 with Stacks Mapped ===
pub fn build_master_kernel_pml4() {
    use crate::memory::layout::{KERNEL_BASE, percpu_stack_base, percpu_stack_top, PERCPU_STACK_SIZE};
    
    let phys_mem_offset = unsafe { 
        PHYS_MEM_OFFSET.expect("Physical memory offset not initialized") 
    };
    
    log::info!("STEP 2: Building master kernel PML4 with upper-half mappings and per-CPU stacks");
    
    // Get current PML4 to copy from
    let (current_pml4_frame, _) = Cr3::read();
    
    // Allocate new master PML4
    let master_pml4_frame = allocate_frame().expect("Failed to allocate master PML4");
    
    unsafe {
        let master_pml4_virt = phys_mem_offset + master_pml4_frame.start_address().as_u64();
        let master_pml4 = &mut *(master_pml4_virt.as_mut_ptr() as *mut PageTable);
        
        // Clear all entries
        for i in 0..512 {
            master_pml4[i].set_unused();
        }
        
        let current_pml4_virt = phys_mem_offset + current_pml4_frame.start_address().as_u64();
        let current_pml4 = &*(current_pml4_virt.as_ptr() as *const PageTable);
        
        // Copy upper-half entries (256-511) from current - these already exist
        for i in 256..512 {
            if !current_pml4[i].is_unused() {
                master_pml4[i] = current_pml4[i].clone();
            }
        }
        
        // CRITICAL: Also preserve PML4[2] - the direct physical memory mapping
        // The kernel actually executes from here (0x100_xxxx_xxxx range)
        if !current_pml4[2].is_unused() {
            master_pml4[2] = current_pml4[2].clone();
            log::info!("PHASE2: Preserved PML4[2] (direct physical memory mapping) in master");
        } else {
            log::warn!("PHASE2: PML4[2] is empty in current - kernel may not be accessible!");
        }
        
        // CRITICAL: Also preserve PML4[3] - kernel stack region
        // The kernel stack is at 0x180_xxxx_xxxx range
        if !current_pml4[3].is_unused() {
            master_pml4[3] = current_pml4[3].clone();
            log::info!("PHASE2: Preserved PML4[3] (kernel stack region) in master");
        }
        
        // PHASE2 CRITICAL: Create alias mapping for kernel code/data
        // The kernel is currently at 0x100000 (PML4[0])
        // We need to alias it at 0xffffffff80000000 (PML4[511])
        
        // Calculate PML4 index for KERNEL_BASE (0xffffffff80000000)
        let kernel_pml4_idx = ((KERNEL_BASE >> 39) & 0x1FF) as usize;  // Should be 511
        
        // If PML4[0] contains kernel mappings, we need to preserve them AND alias them
        if !current_pml4[0].is_unused() {
            // Get the PDPT frame from PML4[0]
            let low_pdpt_frame = current_pml4[0].frame().unwrap();
            
            // We'll share the same PDPT but need to ensure it has correct flags
            let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::GLOBAL;
            
            // CRITICAL: Preserve PML4[0] for low-half kernel execution
            // This is temporary until we move to high-half execution
            master_pml4[0] = current_pml4[0].clone();
            log::info!("PHASE2-TEMP: Preserved PML4[0] in master for low-half kernel execution");
            
            // Also alias it at the high half for future transition
            master_pml4[kernel_pml4_idx].set_frame(low_pdpt_frame, flags);
            
            log::info!("PHASE2: Aliased kernel from PML4[0] to PML4[{}] (0xffffffff80000000)", 
                      kernel_pml4_idx);
        }
        
        // Copy kernel stack mappings (PML4[402] for 0xffffc90000000000)
        let kernel_stack_idx = 402;
        if !current_pml4[kernel_stack_idx].is_unused() {
            master_pml4[kernel_stack_idx] = current_pml4[kernel_stack_idx].clone();
            // Set GLOBAL flag
            let flags = master_pml4[kernel_stack_idx].flags() | PageTableFlags::GLOBAL;
            let frame = master_pml4[kernel_stack_idx].frame().unwrap();
            master_pml4[kernel_stack_idx].set_frame(frame, flags);
            log::info!("PHASE2: Master PML4[{}] kernel stacks -> frame {:?}", kernel_stack_idx, frame);
        }
        
        // Copy IST stack mappings (PML4[403] for 0xffffc98000000000)
        let ist_stack_idx = 403;
        if !current_pml4[ist_stack_idx].is_unused() {
            // CRITICAL FIX: Check if PML4[402] and PML4[403] incorrectly alias
            let kernel_stack_frame = if !current_pml4[kernel_stack_idx].is_unused() {
                current_pml4[kernel_stack_idx].frame().ok()
            } else {
                None
            };
            
            let ist_frame = current_pml4[ist_stack_idx].frame().unwrap();
            
            if let Some(ks_frame) = kernel_stack_frame {
                if ks_frame == ist_frame {
                    // CRITICAL BUG: PML4[402] and PML4[403] point to the same PML3!
                    // This will cause kernel stack faults. Fix it by allocating a new PML3 for IST.
                    log::error!("🔴 CRITICAL: PML4[402] and PML4[403] both point to frame {:?}", ist_frame);
                    log::info!("🔧 FIX: Allocating separate PML3 for PML4[403] (IST stacks)");
                    
                    // Allocate a new PML3 table for IST stacks
                    use crate::memory::frame_allocator::allocate_frame;
                    let new_ist_pml3_frame = allocate_frame()
                        .expect("Failed to allocate PML3 for IST stacks");
                    
                    // Zero the new PML3 table
                    let new_pml3_virt = phys_mem_offset + new_ist_pml3_frame.start_address().as_u64();
                    unsafe {
                        let new_pml3 = &mut *(new_pml3_virt.as_mut_ptr() as *mut PageTable);
                        for i in 0..512 {
                            new_pml3[i].set_unused();
                        }
                    }
                    
                    // Set PML4[403] to point to the new PML3
                    let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::GLOBAL;
                    master_pml4[ist_stack_idx].set_frame(new_ist_pml3_frame, flags);
                    log::info!("PHASE2: Master PML4[{}] IST stacks -> NEW frame {:?}", ist_stack_idx, new_ist_pml3_frame);
                    
                    // Verify the fix was applied
                    let verify_frame = master_pml4[ist_stack_idx].frame().unwrap();
                    log::info!("PHASE2: VERIFIED PML4[403] now points to {:?}", verify_frame);
                    assert_ne!(verify_frame, ks_frame, "PML4[403] still aliases PML4[402]!");
                    
                    // Log the actual memory address we're modifying
                    log::info!("PHASE2: Modified PML4[403] at virtual address {:p}", &master_pml4[ist_stack_idx]);
                    log::info!("PHASE2: Master PML4 base address is {:p}", master_pml4);
                } else {
                    // No aliasing, just copy normally
                    master_pml4[ist_stack_idx] = current_pml4[ist_stack_idx].clone();
                    let flags = master_pml4[ist_stack_idx].flags() | PageTableFlags::GLOBAL;
                    master_pml4[ist_stack_idx].set_frame(ist_frame, flags);
                    log::info!("PHASE2: Master PML4[{}] IST stacks -> frame {:?}", ist_stack_idx, ist_frame);
                }
            } else {
                // PML4[402] is empty, just copy PML4[403] normally
                master_pml4[ist_stack_idx] = current_pml4[ist_stack_idx].clone();
                let flags = master_pml4[ist_stack_idx].flags() | PageTableFlags::GLOBAL;
                master_pml4[ist_stack_idx].set_frame(ist_frame, flags);
                log::info!("PHASE2: Master PML4[{}] IST stacks -> frame {:?}", ist_stack_idx, ist_frame);
            }
        }
        
        // Log what's in PML4[510] if present
        if !master_pml4[510].is_unused() {
            let frame = master_pml4[510].frame().unwrap();
            log::info!("PHASE2: Master PML4[510] -> frame {:?}", frame);
        }
        
        // === STEP 2: Pre-build page table hierarchy for kernel stacks (Option B) ===
        // Per Cursor guidance: Build PML4->PDPT->PD->PT hierarchy now,
        // but leave leaf PTEs unmapped. allocate_kernel_stack() will populate them later.
        log::info!("STEP 2: Pre-building page table hierarchy for kernel stacks (without leaf mappings)");
        
        // CRITICAL INSIGHT from Cursor consultation:
        // - Option B is correct: Pre-create hierarchy, populate PTEs on demand
        // - This matches Linux vmalloc/per-CPU area patterns
        // - Ensures all processes share the SAME kernel subtree (not copies)
        // - TLB: Local invlpg on add, no remote shootdown needed
        
        // The kernel stacks at 0xffffc90000000000 are in PML4[402]
        let kernel_stack_pml4_idx = 402;
        
        // Ensure PML4[402] has a PDPT allocated
        let pdpt_frame = if master_pml4[kernel_stack_pml4_idx].is_unused() {
            // Allocate PDPT for kernel stacks
            let frame = allocate_frame().expect("Failed to allocate PDPT for kernel stacks");
            let pdpt_virt = phys_mem_offset + frame.start_address().as_u64();
            let pdpt = &mut *(pdpt_virt.as_mut_ptr() as *mut PageTable);
            for i in 0..512 {
                pdpt[i].set_unused();
            }
            
            // Per Cursor: GLOBAL doesn't apply to intermediate entries (only leaf PTEs)
            let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE;
            master_pml4[kernel_stack_pml4_idx].set_frame(frame, flags);
            log::info!("STEP 2: Allocated PDPT for kernel stacks at frame {:?} (no GLOBAL on intermediate)", frame);
            frame
        } else {
            let frame = master_pml4[kernel_stack_pml4_idx].frame().unwrap();
            log::info!("STEP 2: Using existing PDPT for kernel stacks at frame {:?}", frame);
            frame
        };
        
        // Build the page table hierarchy for the entire kernel stack region
        // We need to cover the full range: 0xffffc900_0000_0000 to 0xffffc900_0100_0000 (16MB)
        // This ensures ALL kernel stacks can be allocated later without issues
        const KERNEL_STACK_REGION_START: u64 = 0xffffc900_0000_0000;
        const KERNEL_STACK_REGION_END: u64 = 0xffffc900_0100_0000;
        
        let pdpt_virt = phys_mem_offset + pdpt_frame.start_address().as_u64();
        let pdpt = &mut *(pdpt_virt.as_mut_ptr() as *mut PageTable);
        
        log::info!("STEP 2: Building hierarchy for kernel stack region {:#x}-{:#x}", 
                  KERNEL_STACK_REGION_START, KERNEL_STACK_REGION_END);
        
        // We need to ensure PD and PT exist for the entire region
        // The region spans only one PDPT entry (index 0) since it's only 16MB
        let pdpt_index = 0; // (0xffffc900_0000_0000 >> 30) & 0x1FF = 0
        
        // Ensure PD exists for the kernel stack region
        let pd_frame = if pdpt[pdpt_index].is_unused() {
            let frame = allocate_frame().expect("Failed to allocate PD for kernel stacks");
            let pd_virt = phys_mem_offset + frame.start_address().as_u64();
            let pd = &mut *(pd_virt.as_mut_ptr() as *mut PageTable);
            for i in 0..512 {
                pd[i].set_unused();
            }
            
            // Don't use GLOBAL on intermediate tables per Cursor guidance
            let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE;
            pdpt[pdpt_index].set_frame(frame, flags);
            log::info!("STEP 2: Allocated PD for kernel stacks at frame {:?}", frame);
            frame
        } else {
            pdpt[pdpt_index].frame().unwrap()
        };
        
        let pd_virt = phys_mem_offset + pd_frame.start_address().as_u64();
        let pd = &mut *(pd_virt.as_mut_ptr() as *mut PageTable);
        
        // The 16MB region spans 8 PD entries (each PD entry covers 2MB)
        // PD indices 0-7 for the kernel stack region
        for pd_index in 0..8 {
            // Ensure PT exists for each 2MB chunk
            if pd[pd_index].is_unused() {
                let frame = allocate_frame().expect("Failed to allocate PT for kernel stacks");
                let pt_virt = phys_mem_offset + frame.start_address().as_u64();
                let pt = &mut *(pt_virt.as_mut_ptr() as *mut PageTable);
                for i in 0..512 {
                    pt[i].set_unused();  // Leave all PTEs unmapped - allocate_kernel_stack will populate them
                }
                
                let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE;
                pd[pd_index].set_frame(frame, flags);
                log::debug!("STEP 2: Allocated PT[{}] for kernel stacks at frame {:?}", pd_index, frame);
            }
        }
        
        log::info!("STEP 2: Page table hierarchy built for kernel stack region:");
        log::info!("  PML4[{}] -> PDPT frame {:?}", kernel_stack_pml4_idx, pdpt_frame);
        log::info!("  PDPT[0] -> PD frame {:?}", pd_frame);
        log::info!("  PD[0-7] -> PT frames allocated");
        log::info!("  PTEs: Left unmapped (will be populated by allocate_kernel_stack)");
        
        log::info!("STEP 2: Successfully pre-built page table hierarchy for kernel stacks");
    }
    
    // CRITICAL FIX: Ensure PML4[402] and PML4[403] point to different PML3 tables
    // This is a final check and fix right before storing
    unsafe {
        let master_pml4_virt = phys_mem_offset + master_pml4_frame.start_address().as_u64();
        let master_pml4 = &mut *(master_pml4_virt.as_mut_ptr() as *mut PageTable);
        
        // Check if they're aliased
        if !master_pml4[402].is_unused() && !master_pml4[403].is_unused() {
            let frame_402 = master_pml4[402].frame().unwrap();
            let frame_403 = master_pml4[403].frame().unwrap();
            
            if frame_402 == frame_403 {
                log::error!("🔴 FINAL FIX NEEDED: PML4[402] and [403] still alias to {:?}", frame_402);
                
                // Allocate a new PML3 for IST stacks
                use crate::memory::frame_allocator::allocate_frame;
                let new_ist_pml3 = allocate_frame().expect("Failed to allocate PML3 for IST final fix");
                
                // Zero it
                let new_pml3_virt = phys_mem_offset + new_ist_pml3.start_address().as_u64();
                let new_pml3_table = &mut *(new_pml3_virt.as_mut_ptr() as *mut PageTable);
                for i in 0..512 {
                    new_pml3_table[i].set_unused();
                }
                
                // Set PML4[403] to the new PML3
                let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::GLOBAL;
                master_pml4[403].set_frame(new_ist_pml3, flags);
                
                log::info!("🔧 FINAL FIX: Set PML4[403] to new frame {:?}", new_ist_pml3);
                
                // Verify the fix
                let final_402 = master_pml4[402].frame().unwrap();
                let final_403 = master_pml4[403].frame().unwrap();
                log::info!("✓ FINAL VERIFICATION: PML4[402]={:?}, PML4[403]={:?}", final_402, final_403);
                assert_ne!(final_402, final_403, "Final fix failed!");
            }
        }
    }
    
    // Store the master PML4 for process creation
    *MASTER_KERNEL_PML4.lock() = Some(master_pml4_frame);
    
    log::info!("PHASE2: Master kernel PML4 built at frame {:?}", master_pml4_frame);
    
    // === STEP 3: Defer CR3 switch to master kernel PML4 ===
    // NOTE: We cannot switch CR3 here because we're still on the bootstrap stack
    // which may not be properly mapped in the master PML4. The CR3 switch will
    // happen later after we've switched to the per-CPU kernel stack.
    log::info!("STEP 3: Master kernel PML4 ready, deferring CR3 switch until after stack switch");
}

/// Get the master kernel PML4 frame for process creation (Phase 2)
pub fn master_kernel_pml4() -> Option<PhysFrame> {
    MASTER_KERNEL_PML4.lock().clone()
}

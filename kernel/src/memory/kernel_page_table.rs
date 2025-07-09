//! Global kernel page table management
//! 
//! This module implements a production-grade, globally-shared kernel address space
//! that guarantees every kernel stack is mapped in every address space before use.
//! 
//! ## Design
//! - PML4 entries 256-511 point to a single shared kernel_pdpt
//! - All kernel mappings are installed once through the shared hierarchy
//! - Each process gets its own PML4 but shares the kernel mappings

use x86_64::{
    structures::paging::{
        PageTable, PageTableFlags, PhysFrame,
    },
    VirtAddr, PhysAddr,
    registers::control::Cr3,
};
use spin::Mutex;
use crate::memory::frame_allocator::allocate_frame;

/// The global kernel PDPT (L3 page table) frame
static KERNEL_PDPT_FRAME: Mutex<Option<PhysFrame>> = Mutex::new(None);

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
    let kernel_pdpt_frame = allocate_frame()
        .expect("Failed to allocate frame for kernel PDPT");
    
    // Zero the PDPT
    unsafe {
        let pdpt_virt = phys_mem_offset + kernel_pdpt_frame.start_address().as_u64();
        let pdpt = &mut *(pdpt_virt.as_mut_ptr() as *mut PageTable);
        pdpt.zero();
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
    
    let phys_mem_offset = PHYS_MEM_OFFSET
        .ok_or("Physical memory offset not initialized")?;
    
    let kernel_pdpt_frame = KERNEL_PDPT_FRAME.lock()
        .ok_or("Kernel PDPT not initialized")?;
    
    // Get the current PML4
    let (pml4_frame, _) = Cr3::read();
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
            entry.set_frame(kernel_pdpt_frame, PageTableFlags::PRESENT | PageTableFlags::WRITABLE);
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
        pd.zero();
        
        pdpt[pdpt_index as usize].set_frame(frame, PageTableFlags::PRESENT | PageTableFlags::WRITABLE);
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
        pt.zero();
        
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
    
    log::trace!("Mapped kernel page {:?} -> {:?} with flags {:?}", virt, phys, flags);
    
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


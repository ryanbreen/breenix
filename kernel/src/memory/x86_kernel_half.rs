#[cfg(not(target_arch = "x86_64"))]
use super::{PageTable, PageTableFlags, PhysFrame, Size4KiB, VirtAddr};
#[cfg(target_arch = "x86_64")]
use x86_64::{
    structures::paging::{PageTable, PageTableFlags, PhysFrame, Size4KiB},
    VirtAddr,
};

/// The offset must map the boot tables and every frame returned by the allocator.
/// Allocated frames must be disjoint from all the boot tables.
pub(super) unsafe fn migrate_kernel_half(
    pml4: &mut PageTable,
    phys_mem_offset: VirtAddr,
    mut allocate: impl FnMut() -> PhysFrame,
) -> PhysFrame {
    let kernel_pdpt_frame = allocate();

    let pdpt_virt = phys_mem_offset + kernel_pdpt_frame.start_address().as_u64();
    let pdpt = &mut *pdpt_virt.as_mut_ptr::<PageTable>();
    for i in 0..512 {
        pdpt[i].set_unused();
    }

    // Copy existing kernel mappings (256-511) to the new kernel PDPT
    for i in 256..512 {
        if !pml4[i].is_unused() {
            // This PML4 entry has kernel mappings
            let old_pdpt_frame: PhysFrame<Size4KiB> = pml4[i].frame().unwrap();
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

    kernel_pdpt_frame
}

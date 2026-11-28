use x86_64::{
    structures::paging::{PageTable, PageTableFlags, PhysFrame},
    VirtAddr,
};

/// Install a boot-owned PDPT for each of the 256 kernel PML4 slots.
///
/// The offset must map the boot tables and each frame returned by the allocator.
/// Allocated frames must be disjoint from each other and from the boot tables.
pub(super) unsafe fn migrate_kernel_half(
    pml4: &mut PageTable,
    phys_mem_offset: VirtAddr,
    mut allocate: impl FnMut() -> PhysFrame,
) {
    for root_entry in pml4.iter_mut().skip(256) {
        let frame = allocate();
        let new_pdpt = (phys_mem_offset + frame.start_address().as_u64()).as_mut_ptr::<PageTable>();
        let mut flags = root_entry.flags();

        if let Ok(old_frame) = root_entry.frame() {
            let old_pdpt =
                (phys_mem_offset + old_frame.start_address().as_u64()).as_ptr::<PageTable>();
            core::ptr::copy_nonoverlapping(old_pdpt, new_pdpt, 1);

            // Keep the root writable and executable for later mappings without
            // changing the effective permissions of any of the boot mappings.
            for entry in (*new_pdpt).iter_mut() {
                let mut child_flags = entry.flags();
                if child_flags.contains(PageTableFlags::PRESENT) {
                    if !flags.contains(PageTableFlags::WRITABLE) {
                        child_flags.remove(PageTableFlags::WRITABLE);
                    }
                    child_flags |= flags & PageTableFlags::NO_EXECUTE;
                    if child_flags != entry.flags() {
                        entry.set_flags(child_flags);
                    }
                }
            }
        } else {
            (*new_pdpt).zero();
            flags = PageTableFlags::empty();
        }

        flags.insert(PageTableFlags::PRESENT | PageTableFlags::WRITABLE);
        flags.remove(PageTableFlags::NO_EXECUTE);
        root_entry.set_frame(frame, flags);
    }
}

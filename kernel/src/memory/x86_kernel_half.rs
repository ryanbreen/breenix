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

/// Copy the master's shared entries into a new process root: the 256 kernel
/// slots, and each lower-half slot the master marks supervisor-only, plus the
/// legacy kernel slots 2 and 3. Slot 0 is left to the process.
pub(crate) fn inherit_master_slots(master: &PageTable, root: &mut PageTable) {
    for i in 256..512 {
        root[i] = master[i].clone();
    }

    // The bootloader can place kernel mappings in the lower half, and the
    // physical-memory mapping is dynamic, so any of these slots may hold the
    // direct map. Exec does not clear root slots of its new table, which the
    // `exec_root_keeps_a_direct_map_at_pml4_32` host fixture checks.
    for i in 1..256 {
        if i == 2 || i == 3 || !master[i].flags().contains(PageTableFlags::USER_ACCESSIBLE) {
            root[i] = master[i].clone();
        }
    }
}

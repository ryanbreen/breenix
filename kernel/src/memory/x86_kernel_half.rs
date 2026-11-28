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

    // The bootloader can place kernel mappings in the lower half.
    for i in 1..256 {
        if i == 2 || i == 3 || !master[i].flags().contains(PageTableFlags::USER_ACCESSIBLE) {
            root[i] = master[i].clone();
        }
    }
}

/// Clear the PML4 entries exec treats as old user ranges.
pub(crate) fn clear_exec_slots(root: &mut PageTable) {
    // Entry 32: Alternative user code location (0x100000000000)
    if !root[32].is_unused() {
        root[32].set_unused();
    }

    // Entry 170: User stack location (0x550000000000)
    if !root[170].is_unused() {
        root[170].set_unused();
    }
}

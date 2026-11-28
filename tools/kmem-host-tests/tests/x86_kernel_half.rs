use std::collections::BTreeSet;

use x86_64::{
    structures::paging::{OffsetPageTable, PageTable, PageTableFlags as Flags, PhysFrame, Translate},
    PhysAddr, VirtAddr,
};

#[path = "../../../kernel/src/memory/x86_kernel_half.rs"]
mod x86_kernel_half;

fn frame(index: usize) -> PhysFrame {
    PhysFrame::from_start_address(PhysAddr::new(index as u64 * 4096)).unwrap()
}

/// Effective (writable, no-execute) rights of a mapped VA: writable unless a
/// level clears R/W, no-execute if a level sets XD
/// (Intel SDM Vol. 3A, 4.6 Access Rights).
fn effective_rights(tables: &[PageTable], root: usize, va: VirtAddr) -> (bool, bool) {
    let indices = [
        usize::from(va.p4_index()),
        usize::from(va.p3_index()),
        usize::from(va.p2_index()),
        usize::from(va.p1_index()),
    ];
    let (mut writable, mut no_execute) = (true, false);
    let mut table = root;
    for index in indices {
        let entry = &tables[table][index];
        let flags = entry.flags();
        assert!(flags.contains(Flags::PRESENT), "walk reached a non-present entry");
        writable &= flags.contains(Flags::WRITABLE);
        no_execute |= flags.contains(Flags::NO_EXECUTE);
        if flags.contains(Flags::HUGE_PAGE) {
            break;
        }
        table = (entry.addr().as_u64() / 4096) as usize;
    }
    (writable, no_execute)
}

#[test]
fn migration_preserves_two_direct_map_slots_with_the_same_pdpt_index() {
    // Fixture from #3264: the two VAs have the same L3 and L2 indices.
    let va0 = VirtAddr::new(0xffff_8000_1000_0000);
    let va1 = VirtAddr::new(0xffff_8080_1000_0000);
    let phys0 = PhysAddr::new(0x1000_0000);
    let phys1 = PhysAddr::new(0x80_1000_0000);
    let flags = Flags::PRESENT | Flags::WRITABLE;

    // Frame 0 is the root, followed by the original PDPT and PD for
    // each slot, then the frames available to the boot allocator.
    let mut tables = Vec::new();
    tables.resize_with(5 + 256, PageTable::new);
    let offset = VirtAddr::from_ptr(tables.as_ptr());
    // Slot 256's root entry is read-only and no-execute, so those rights
    // live only at the root before migration.
    tables[0][256].set_frame(frame(1), Flags::PRESENT | Flags::NO_EXECUTE);
    tables[0][257].set_frame(frame(3), flags);
    tables[1][0].set_frame(frame(2), flags);
    tables[3][0].set_frame(frame(4), flags);
    tables[2][128].set_addr(phys0, flags | Flags::HUGE_PAGE);
    tables[4][128].set_addr(phys1, flags | Flags::HUGE_PAGE | Flags::NO_EXECUTE);

    let translate = |root: &mut PageTable| {
        // This is a software walk. The synthetic physical range is backed by tables.
        let mapper = unsafe { OffsetPageTable::new(root, offset) };
        (mapper.translate_addr(va0), mapper.translate_addr(va1))
    };
    assert_eq!(translate(&mut tables[0]), (Some(phys0), Some(phys1)));
    let rights_before = (
        effective_rights(&tables, 0, va0),
        effective_rights(&tables, 0, va1),
    );
    assert_eq!(rights_before, ((false, true), (true, true)));

    let mut free = (5..tables.len()).map(frame);
    unsafe {
        x86_kernel_half::migrate_kernel_half(&mut tables[0], offset, || free.next().unwrap());
    }

    assert_eq!(
        translate(&mut tables[0]),
        (Some(phys0), Some(phys1)),
        "the boot direct-map addresses must keep their physical identities"
    );
    assert_eq!(
        (
            effective_rights(&tables, 0, va0),
            effective_rights(&tables, 0, va1),
        ),
        rights_before,
        "the boot mappings must keep their effective access rights"
    );

    let pdpts: BTreeSet<_> = (256..512)
        .map(|slot| tables[0][slot].frame().unwrap().start_address())
        .collect();
    assert_eq!(pdpts.len(), 256, "each kernel slot needs its own PDPT");
    assert_eq!(free.next(), None, "allocate exactly 256 PDPT frames");
    assert!(tables[0][255].is_unused());
    for slot in 256..512 {
        let root_flags = tables[0][slot].flags();
        assert!(
            root_flags.contains(Flags::WRITABLE) && !root_flags.contains(Flags::NO_EXECUTE),
            "kernel slot {slot} must leave access rights to the levels below PML4"
        );
    }

    let new_slot = 511;
    let new_pdpt = (tables[0][new_slot].addr().as_u64() / 4096) as usize;
    assert!(tables[new_pdpt][0].is_unused());
    // Simulate a later mapping within a previously empty slot at the same
    // PDPT index. A populated direct-map slot must keep its own translation.
    tables[new_pdpt][0].set_addr(PhysAddr::new(1 << 30), flags | Flags::HUGE_PAGE);
    assert_eq!(translate(&mut tables[0]), (Some(phys0), Some(phys1)));
}

#[test]
fn exec_root_keeps_a_direct_map_at_pml4_32() {
    // The physical-memory mapping is dynamic, so the loader can place the
    // direct map in any free lower-half slot; this fixture uses slot 32
    // (VA 0x1000_0000_0000). The slot-170 entry stands for any other
    // supervisor-only lower-half mapping.
    let direct_map = VirtAddr::new(32 << 39);
    let low_ram = PhysAddr::new(0x20_0000);
    let other = VirtAddr::new(170 << 39);
    let flags = Flags::PRESENT | Flags::WRITABLE;

    // Frame 0 is the master, 1 the process root, 2 and 3 the master's PDPTs
    // for slots 32 and 170, and 4 a user PDPT the root must not inherit.
    let mut tables = Vec::new();
    tables.resize_with(5, PageTable::new);
    let offset = VirtAddr::from_ptr(tables.as_ptr());
    tables[0][32].set_frame(frame(2), flags);
    tables[0][170].set_frame(frame(3), flags);
    tables[0][7].set_frame(frame(4), flags | Flags::USER_ACCESSIBLE);
    tables[2][0].set_addr(PhysAddr::new(0), flags | Flags::HUGE_PAGE);
    tables[3][0].set_addr(PhysAddr::new(1 << 30), flags | Flags::HUGE_PAGE);

    let (master, rest) = tables.split_at_mut(1);
    x86_kernel_half::inherit_master_slots(&master[0], &mut rest[0]);
    // Exec's preparation of its new root.
    x86_kernel_half::clear_exec_slots(&mut rest[0]);

    let translate = |root: &mut PageTable, va: VirtAddr| {
        // This is a software walk. The synthetic physical range is backed by tables.
        let mapper = unsafe { OffsetPageTable::new(root, offset) };
        mapper.translate_addr(va)
    };
    assert_eq!(
        translate(&mut tables[1], direct_map + low_ram.as_u64()),
        Some(low_ram),
        "the process root must reach low physical RAM through the direct map"
    );
    assert_eq!(translate(&mut tables[1], other), Some(PhysAddr::new(1 << 30)));
    assert!(tables[1][7].is_unused(), "a user-accessible master slot is not inherited");
    assert!(tables[1][0].is_unused(), "slot 0 is left to the process");
}

use std::collections::BTreeSet;

use x86_64::{
    structures::paging::{OffsetPageTable, PageTable, PageTableFlags as Flags, PhysFrame, Translate},
    PhysAddr, VirtAddr,
};

// The fixture runs on either host architecture. Only the kernel build uses
// the ARM64 stub types when compiling the legacy entry point on ARM64.
use x86_64::structures::paging::{PageTableFlags, Size4KiB};
#[path = "../../../kernel/src/memory/x86_kernel_half.rs"]
mod x86_kernel_half;

fn frame(index: usize) -> PhysFrame {
    PhysFrame::from_start_address(PhysAddr::new(index as u64 * 4096)).unwrap()
}

#[test]
fn migration_preserves_two_direct_map_slots_with_the_same_pdpt_index() {
    // Fixture from #3264: the two VAs have the same L3 and L2 indices.
    let va0 = VirtAddr::new(0xffff_8000_1000_0000);
    let va1 = VirtAddr::new(0xffff_8080_1000_0000);
    let phys0 = PhysAddr::new(0x1000_0000);
    let phys1 = PhysAddr::new(0x80_1000_0000);
    let flags = Flags::PRESENT | Flags::WRITABLE;

    // Frame zero is the root, followed by the original PDPT and PD for
    // each slot, then the frames available to the boot allocator.
    let mut tables = Vec::new();
    tables.resize_with(5 + 256, PageTable::new);
    let offset = VirtAddr::from_ptr(tables.as_ptr());
    tables[0][256].set_frame(frame(1), flags);
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

    let mut free = (5..tables.len()).map(frame);
    unsafe {
        x86_kernel_half::migrate_kernel_half(&mut tables[0], offset, || free.next().unwrap());
    }

    assert_eq!(
        translate(&mut tables[0]),
        (Some(phys0), Some(phys1)),
        "the boot direct-map addresses must keep their physical identities"
    );

    let pdpts: BTreeSet<_> = (256..512)
        .map(|slot| tables[0][slot].frame().unwrap().start_address())
        .collect();
    assert_eq!(pdpts.len(), 256, "each kernel slot needs its own PDPT");
    assert_eq!(free.next(), None, "allocate exactly 256 PDPT frames");
    assert!(tables[0][255].is_unused());

    let new_slot = 511;
    let new_pdpt = (tables[0][new_slot].addr().as_u64() / 4096) as usize;
    assert!(tables[new_pdpt][0].is_unused());
    // Simulate a later mapping within a previously empty slot at the same
    // PDPT index. A populated direct-map slot must keep its own translation.
    tables[new_pdpt][0].set_addr(PhysAddr::new(1 << 30), flags | Flags::HUGE_PAGE);
    assert_eq!(translate(&mut tables[0]), (Some(phys0), Some(phys1)));
}

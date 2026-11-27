//! The translation a device is given for a kernel address
//! (`kernel/src/memory/kernel_walk.rs`): a missing level is an error, not the
//! virtual address handed back as physical, and physical addresses above 4 GiB
//! come through whole.

use crate::kernel_walk::{decode_par_el1, walk_x86_64, WalkError};
use std::collections::HashMap;

const P: u64 = 1; // present
const W: u64 = 1 << 1; // writable
const PS: u64 = 1 << 7; // page size (a 1 GiB or 2 MiB leaf)
const PAT_LARGE: u64 = 1 << 12; // PAT in a large-page entry, inside the address field
const NX: u64 = 1 << 63;

/// Page tables as a map from (table physical address, index) to entry.
struct Tables(HashMap<(u64, usize), u64>);

impl Tables {
    fn walk(&self, root: u64, virt: u64) -> Result<u64, WalkError> {
        walk_x86_64(root, virt, |table, index| self.0.get(&(table, index)).copied().unwrap_or(0))
    }
}

fn index(virt: u64, level: u32) -> usize {
    ((virt >> (12 + 9 * (level - 1))) & 0x1ff) as usize
}

const ROOT: u64 = 0x1000;
const PDPT: u64 = 0x2000;
const PD: u64 = 0x3000;
const PT: u64 = 0x4000;
/// A kernel heap address, in a 4 KiB page backed above 4 GiB.
const HEAP: u64 = 0x4444_4444_0123;
const HEAP_FRAME: u64 = 0x1_2345_6000;

fn heap_tables() -> Tables {
    let mut t = HashMap::new();
    t.insert((ROOT, index(HEAP, 4)), PDPT | P | W);
    t.insert((PDPT, index(HEAP, 3)), PD | P | W);
    t.insert((PD, index(HEAP, 2)), PT | P | W);
    t.insert((PT, index(HEAP, 1)), HEAP_FRAME | P | W | NX);
    Tables(t)
}

#[test]
fn a_walk_that_misses_a_level_is_an_error_naming_the_level() {
    let mut tables = heap_tables();
    assert_eq!(tables.walk(ROOT, HEAP), Ok(HEAP_FRAME | 0x123));
    for (level, table) in [(1, PT), (2, PD), (3, PDPT), (4, ROOT)] {
        tables.0.insert((table, index(HEAP, level)), 0);
        assert_eq!(tables.walk(ROOT, HEAP), Err(WalkError::NotPresent { level: level as u8 }));
    }
    // The old code returned `virt` here; the address is not mapped at all.
    assert!(Tables(HashMap::new()).walk(ROOT, HEAP).is_err());
}

#[test]
fn large_pages_translate_above_four_gib_without_their_pat_bit() {
    // A direct-map address 6 GiB above the map's base, in a 1 GiB page.
    let base = 0xffff_8000_0000_0000u64;
    let direct = base + 0x1_8000_0000 + 0x2345_6789;
    let mut t = HashMap::new();
    t.insert((ROOT, index(direct, 4)), PDPT | P | W);
    t.insert((PDPT, index(direct, 3)), 0x1_8000_0000 | PAT_LARGE | PS | P | W | NX);
    // A 2 MiB page at 5 GiB.
    let two_mib = base + 0x4000_0000 * 9 + 0x20_0000 * 3 + 0x1_2345;
    t.insert((PDPT, index(two_mib, 3)), PD | P | W);
    t.insert((PD, index(two_mib, 2)), 0x1_4000_0000 | PAT_LARGE | PS | P | W);
    let tables = Tables(t);
    assert_eq!(tables.walk(ROOT, direct), Ok(0x1_8000_0000 + 0x2345_6789));
    assert_eq!(tables.walk(ROOT, two_mib), Ok(0x1_4000_0000 + 0x1_2345));
}

#[test]
fn an_arm64_translation_fault_is_an_error_and_a_high_address_comes_through() {
    // PAR_EL1.F (bit 0) set: the MMU could not translate the address.
    assert_eq!(decode_par_el1(0x0000_0000_0000_0809, 0xffff_0000_4000_1234), Err(WalkError::Fault));
    // F clear: PA[51:12] in bits 51:12 and the attributes in bits 63:56; the
    // page offset comes from the address.
    let par = (0xffu64 << 56) | 0x0000_0001_2345_6000 | (0b11 << 7);
    assert_eq!(decode_par_el1(par, 0xffff_0001_2345_6abc), Ok(0x1_2345_6abc));
}

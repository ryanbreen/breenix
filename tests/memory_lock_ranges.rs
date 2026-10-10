//! Exercise the kernel's lock interval accounting independently of page tables.
extern crate alloc;
mod syscall {
    pub mod errno {
        pub const ENOMEM: i32 = 12;
    }
}
#[path = "../kernel/src/memory/locked.rs"]
mod locked;
use locked::MemoryLocks;

#[test]
fn overlapping_locks_charge_each_page_once() {
    let mut locks = MemoryLocks::default();
    locks.insert(4096, 12288).unwrap();
    assert_eq!(locks.additional(8192, 16384), 4096);
    locks.insert(8192, 16384).unwrap();
    locks.insert(4096, 8192).unwrap();
    assert_eq!(locks.bytes(), 12288);
    assert!(locks.overlaps(8192, 12288));
    assert!(!locks.overlaps(16384, 20480));
}

#[test]
fn unlock_middle_preserves_both_sides_and_empty_ranges() {
    let mut locks = MemoryLocks::default();
    locks.insert(4096, 20480).unwrap();
    locks.reserve_split().unwrap();
    locks.remove(8192, 16384);
    assert_eq!(locks.ranges, [(4096, 8192), (16384, 20480)]);
    locks.remove(18000, 18000);
    assert_eq!(locks.bytes(), 8192);
    locks.insert(8192, 16384).unwrap();
    assert_eq!(locks.ranges, [(4096, 20480)]);
}

#[test]
fn unmapping_and_clearing_release_charges_and_future_policy() {
    let mut locks = MemoryLocks::default();
    locks.insert(4096, 8192).unwrap();
    locks.insert(16384, 24576).unwrap();
    locks.reserve_split().unwrap();
    locks.remove(0, 20480);
    assert_eq!(locks.ranges, [(20480, 24576)]);
    locks.future = true;
    locks.onfault = true;
    locks.clear();
    assert_eq!(locks.bytes(), 0);
    assert!(!locks.future && !locks.onfault);
}

#[test]
fn stack_growth_uses_reserved_capacity_and_merges_without_allocating() {
    let mut locks = MemoryLocks::default();
    assert!(!locks.can_insert(4096, 8192));
    assert_eq!(locks.insert_reserved(4096, 8192), Err(12));
    locks.reserve_split().unwrap();
    let capacity = locks.ranges.capacity();
    locks.insert_reserved(12288, 16384).unwrap();
    locks.insert_reserved(8192, 12288).unwrap();
    locks.insert_reserved(4096, 8192).unwrap();
    assert_eq!(locks.ranges, [(4096, 16384)]);
    assert_eq!(locks.ranges.capacity(), capacity);
}

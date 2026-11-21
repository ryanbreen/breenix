//! Small writes keep the existing dirty-page index (#3040).
#![feature(allocator_api)]

extern crate alloc;

#[path = "../../../kernel/src/fs/breenfs/preempt.rs"]
pub mod preempt;

#[path = "../../../kernel/src/fs/errno.rs"]
pub mod errno;
pub mod fs {
    pub use crate::errno;
}
#[path = "../../../kernel/src/syscall/errno.rs"]
pub mod syscall_errno;
pub mod syscall {
    pub use crate::syscall_errno as errno;
}
#[path = "../../../kernel/src/fs/breenfs/dirty.rs"]
pub mod dirty;
pub mod format {
    pub use breenfs_host_tests::format::BLOCK_SIZE;
}

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    static COUNT: Cell<Option<usize>> = const { Cell::new(None) };
}
struct Heap;
unsafe impl GlobalAlloc for Heap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = COUNT.try_with(|count| {
            if let Some(n) = count.get() { count.set(Some(n + 1)); }
        });
        System.alloc(layout)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout)
    }
}
#[global_allocator]
static HEAP: Heap = Heap;

#[test]
fn a_small_append_does_not_rebuild_the_existing_dirty_index() {
    const PAGE: usize = format::BLOCK_SIZE;
    let mut pages = dirty::DirtyPages::default();
    pages.write(0, &vec![0x5a; 1024 * PAGE], |_, _| unreachable!()).unwrap();
    COUNT.with(|count| count.set(Some(0)));
    let result = pages.write((1024 * PAGE) as u64, &[0xa5; PAGE], |_, _| unreachable!());
    let allocations = COUNT.with(|count| count.replace(None).unwrap());
    result.unwrap();
    // One data page and a small number of index nodes fit this bound. Rebuilding
    // the 1024-page index allocates in proportion to its existing entries.
    assert!(allocations <= 16, "single-page append made {allocations} allocations");
    assert_eq!(pages.bytes(), (1025 * PAGE) as u64);
    for index in 0..1024 { assert_eq!(pages.get(index), Some(&[0x5a; PAGE][..])); }
    assert_eq!(pages.get(1024), Some(&[0xa5; PAGE][..]));
    assert_eq!(pages.reservation_runs((1025 * PAGE) as u64, PAGE), (1, 1026, 5));

    // Preparing a partial write can fail after one page was prepared. Neither
    // that page nor its successor may enter the dirty map on this error.
    let before = pages.bytes();
    let mut reads = 0;
    let failed = pages.write((1026 * PAGE + 1) as u64, &[0xcc; PAGE], |_, _| {
        reads += 1;
        if reads == 2 { Err(errno::Errno(syscall_errno::EIO)) } else { Ok(()) }
    });
    assert_eq!(failed, Err(errno::Errno(syscall_errno::EIO)));
    assert_eq!(pages.bytes(), before);
    assert_eq!(pages.get(1026), None);
    assert_eq!(pages.get(1027), None);
}

#[test]
fn contiguous_reservation_projection_does_not_walk_the_dirty_pages() {
    const PAGE: usize = format::BLOCK_SIZE;
    let mut pages = dirty::DirtyPages::default();
    pages.write(0, &vec![0x5a; 1024 * PAGE], |_, _| unreachable!()).unwrap();
    dirty::measurement::take_visits();
    assert_eq!(pages.reservation_runs((1024 * PAGE) as u64, PAGE), (1, 1025, 5));
    let visits = dirty::measurement::take_visits();
    assert!(visits <= 2, "sequential projection visited {visits} page indexes");

    // Compare projections with the page/run partition that write_runs uses,
    // including holes, overlaps, adjacent ranges and the 256-page boundary.
    for existing in [vec![], vec![(17, 300)], vec![(0, 4), (9, 260)]] {
        let mut pages = dirty::DirtyPages::default();
        for &(start, count) in &existing {
            pages.write(start * PAGE as u64, &vec![0x63; count * PAGE], |_, _| unreachable!()).unwrap();
        }
        for (offset, length) in [(0, 0), (0, 1), (16 * PAGE, PAGE), (17 * PAGE + 1, 3 * PAGE),
            (256 * PAGE, 257 * PAGE), (317 * PAGE, PAGE), (500 * PAGE + 1, PAGE)]
        {
            let mut indexes: std::collections::BTreeSet<_> = pages.indexes().collect();
            let old = indexes.len() as u64;
            if length != 0 {
                indexes.extend(offset as u64 / PAGE as u64..((offset + length) as u64).div_ceil(PAGE as u64));
            }
            let total = indexes.len() as u64;
            let mut previous = None;
            let mut in_run = 0;
            let mut runs = 0;
            for index in indexes {
                if previous != Some(index) || in_run == 256 { runs += 1; in_run = 0; }
                in_run += 1;
                previous = Some(index + 1);
            }
            assert_eq!(pages.reservation_runs(offset as u64, length), (total - old, total, runs),
                "existing={existing:?} offset={offset} length={length}");
        }
    }
}

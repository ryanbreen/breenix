extern crate alloc;

#[path = "../kernel/src/window/status_pages.rs"]
mod status_pages;

use breengel_abi::CompositorStatus;
use status_pages::{format_status, CompositeHistory, HistoryEntry, HISTORY_CAPACITY};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    static COUNTING: Cell<bool> = const { Cell::new(false) };
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}
struct AllocationCount;
fn count_allocation() {
    if COUNTING.try_with(Cell::get).unwrap_or(false) {
        ALLOCATIONS.with(|n| n.set(n.get() + 1));
    }
}
unsafe impl GlobalAlloc for AllocationCount {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count_allocation();
        unsafe { System.alloc(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        count_allocation();
        unsafe { System.realloc(ptr, layout, size) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}
#[global_allocator]
static ALLOCATOR: AllocationCount = AllocationCount;

#[test]
fn history_formats_into_one_output_allocation_without_per_row_temporaries() {
    // Receipt collection dominated diagnostic boot 20261106T044721Z-metal.
    // Count allocations in the production formatter used by those readers.
    for count in [0, 1, 480, HISTORY_CAPACITY + 17] {
        let history = CompositeHistory::new();
        let mut expected = String::new();
        for i in 0..count {
            let serial = u64::MAX - i as u64;
            history.push(HistoryEntry { composite_serial: serial, presented_ns: u64::MAX,
                window_id: u64::MAX, commit_id: u64::MAX, pixel_generation: u64::MAX });
            if i >= count.saturating_sub(HISTORY_CAPACITY) {
                expected.push_str(&format!("{serial} {0} {0} {0} {0}\n", u64::MAX));
            }
        }
        ALLOCATIONS.with(|n| n.set(0));
        COUNTING.with(|v| v.set(true));
        let actual = history.format();
        COUNTING.with(|v| v.set(false));
        let allocations = ALLOCATIONS.with(Cell::get);
        assert_eq!(actual, expected, "history wire bytes or retained ordering changed");
        assert_eq!(allocations, usize::from(count != 0),
            "formatting {count} rows allocated per-row temporaries or grew its output");
    }
}

#[test]
fn compositor_line_contains_w16_snapshot() {
    let status = CompositorStatus { claim_epoch: 1, epoch: 2, handled_serial: 3, presented: 4,
        scene_version: 5, composites: 6, next_deadline_ns: 7, owner: 8, flags: 1, reserved: [0; 2] };
    assert_eq!(format_status(&status), "claim_epoch=1 epoch=2 handled_serial=3 presented=4 scene_version=5 composites=6 next_deadline_ns=7 owner=8 flags=1\n");
}

#[test]
fn history_retains_newest_entries_in_order_across_wrap() {
    assert!(HISTORY_CAPACITY >= 4096);
    let history = CompositeHistory::new();
    for i in 0..HISTORY_CAPACITY + 17 {
        history.push(HistoryEntry { composite_serial: i as u64, presented_ns: 10000 + i as u64,
            window_id: 2, commit_id: 3, pixel_generation: 4 });
    }
    let text = history.format();
    let lines: Vec<_> = text.lines().collect();
    assert_eq!(lines.len(), HISTORY_CAPACITY);
    for (i, line) in lines.iter().enumerate() {
        let serial = i + 17;
        assert_eq!(*line, format!("{} {} 2 3 4", serial, 10000 + serial));
    }
}

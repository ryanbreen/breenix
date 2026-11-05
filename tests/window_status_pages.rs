extern crate alloc;

#[path = "../kernel/src/window/status_pages.rs"]
mod status_pages;

use breengel_abi::CompositorStatus;
use status_pages::{format_status, CompositeHistory, HistoryEntry, HISTORY_CAPACITY};

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

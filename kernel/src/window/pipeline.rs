//! W18: bounded, always-on frame-pipeline timestamps. Formatting is read-side only.
//! Columns are stage, object, related object, source input serial, monotonic ns.
//! Stages: input publication/read, window delivery/read, acquire begin/end,
//! commit begin/end, scene admission, render completion, PRESENT return,
//! physical/virtual presentation, and receipt publication (1 through 13), front publication (14), and driver
//! acceptance raster samples (15: native resource, packed counter/line, pixel).
//! Input trace flags (16): device, derived modifier bits plus SYNTHETIC, source serial.
//! Process exit (17..=22): thread id, PID at zombie publication (20), thread group.
//! Stages: entry, windows released, before PM, zombie, after PM, GPU handles released.
use alloc::{string::String, vec::Vec};
use core::fmt::Write;
const CAPACITY: usize = 32768;
type Row = [u64; 5];
struct Ring { rows: [Row; CAPACITY], count: u64 }
static RING: spin::Mutex<Ring> = spin::Mutex::new(Ring {
    rows: [[0; 5]; CAPACITY], count: 0,
});
pub fn record(stage: u64, object: u64, related: u64, source: u64) {
    at(stage, object, related, source, super::events::now_ns());
}
pub fn at(stage: u64, object: u64, related: u64, source: u64, ns: u64) {
    let mut ring = RING.lock();
    let slot = ring.count as usize % CAPACITY;
    ring.rows[slot] = [stage, object, related, source, ns];
    ring.count += 1;
}
fn snapshot() -> Vec<Row> {
    // Copy a bounded tail before formatting; no formatter holds the writer lock.
    let mut rows = Vec::with_capacity(CAPACITY);
    {
        let ring = RING.lock();
        let count = ring.count.min(rows.capacity() as u64) as usize;
        for i in 0..count {
            rows.push(ring.rows[(ring.count as usize - count + i) % CAPACITY]);
        }
    };
    rows
}
pub fn full_text() -> String {
    let mut out = String::new();
    for r in snapshot() {
        let _ = writeln!(out, "{} {} {} {} {}", r[0], r[1], r[2], r[3], r[4]);
    }
    out
}
pub fn text() -> String {
    let rows = snapshot();
    // Keep the last input window's path rather than unrelated animation draws.
    // Selection and formatting are read-side work, after the bounded ring copy.
    let target = rows.iter().rev().find(|r| r[0] == 3 && r[3] != 0).map(|r| r[1]);
    let frames: Vec<_> = rows.iter().filter(|r| r[0] == 9 && Some(r[1]) == target)
        .map(|r| r[3]).collect();
    let selected: Vec<_> = rows.iter().filter(|r| target.is_none() || r[0] <= 4
        || (Some(r[1]) == target && matches!(r[0], 5..=9 | 13 | 14))
        || (matches!(r[0], 10..=12) && frames.contains(&r[1]))).collect();
    let mut out = String::new();
    for r in &selected[selected.len().saturating_sub(256)..] {
        let _ = writeln!(out, "{} {} {} {} {}", r[0], r[1], r[2], r[3], r[4]);
    }
    let [requests, irqs, late] = crate::time::wakeup::stats();
    let _ = writeln!(out, "fine_wake_requests={} fine_wake_irqs={} fine_wake_late_max_ns={}", requests, irqs, late);
    out
}

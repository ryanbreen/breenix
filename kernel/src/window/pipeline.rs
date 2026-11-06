//! W18: bounded, always-on frame-pipeline timestamps. Formatting is read-side only.
//! Columns are stage, object, related object, source input serial, monotonic ns.
//! Stages: input publication/read, window delivery/read, acquire begin/end,
//! commit begin/end, scene admission, render completion, PRESENT return,
//! physical/virtual presentation, and receipt publication (1 through 13).
use alloc::{string::String, vec::Vec};
use core::fmt::Write;
const CAPACITY: usize = 4096;
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
pub fn text() -> String {
    // Copy a bounded tail before formatting; no formatter holds the writer lock.
    let mut rows = Vec::with_capacity(256);
    {
        let ring = RING.lock();
        let count = ring.count.min(rows.capacity() as u64) as usize;
        for i in 0..count {
            rows.push(ring.rows[(ring.count as usize - count + i) % CAPACITY]);
        }
    };
    let mut out = String::new();
    for r in &rows {
        let _ = writeln!(out, "{} {} {} {} {}", r[0], r[1], r[2], r[3], r[4]);
    }
    out
}

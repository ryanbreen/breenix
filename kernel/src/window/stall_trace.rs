//! Bounded, always-on wait evidence. Only the suite parent samples and exports.
use alloc::{format, string::String, vec::Vec};
use core::fmt::Write;

const CAPACITY: usize = 512;
struct Wait { tid: u64, begin: u64, duration: u64, label: String, sampled: bool }
struct Ring { rows: [[u8; 384]; CAPACITY], lengths: [usize; CAPACITY], count: u64, waits: Vec<Wait> }
static RING: spin::Mutex<Ring> = spin::Mutex::new(Ring {
    rows: [[0; 384]; CAPACITY], lengths: [0; CAPACITY], count: 0, waits: Vec::new(),
});
fn push(ring: &mut Ring, row: String) {
    let slot = ring.count as usize % CAPACITY;
    let len = row.len().min(384);
    ring.rows[slot][..len].copy_from_slice(&row.as_bytes()[..len]);
    ring.lengths[slot] = len;
    ring.count += 1;
}
/// Commands arrive through the root-only evidence device; they produce no serial output.
pub fn command(tid: u64, command: &str) {
    let mut words = command.trim().splitn(4, ' ');
    let op = words.next().unwrap_or("");
    let Some(begin) = words.next().and_then(|v| v.parse::<u64>().ok()) else { return };
    if op == "end" { sample(crate::signal::monotonic_nanos()); }
    let mut ring = RING.lock();
    if op == "end" {
        ring.waits.retain(|w| w.tid != tid || w.begin != begin);
    } else if op == "arm" {
        let Some(duration) = words.next().and_then(|v| v.parse::<u64>().ok()) else { return };
        if ring.waits.len() == 128 {
            push(&mut ring, String::from("stall-arm-overflow"));
            return;
        }
        ring.waits.push(Wait { tid, begin, duration, label: String::from(words.next().unwrap_or("wait")), sampled: false });
    } else if matches!(op, "click-handled" | "click-observed" | "click-request") {
        push(&mut ring, format!("{op} tid={tid} serial={begin} window={} ns={}", words.next().unwrap_or("0"), crate::signal::monotonic_nanos()));
    }
}
/// Sample after crossing half the original deadline, independently of the case thread.
pub fn sample(now: u64) {
    let due: Vec<_> = {
        let mut ring = RING.lock();
        ring.waits.iter_mut().filter_map(|w| {
            if w.sampled || now < w.begin.saturating_add(w.duration / 2) { return None; }
            w.sampled = true;
            Some(format!("wait={} begin_ns={} deadline_ns={} snapshot_ns={now} observer_tid={}",
                w.label, w.begin, w.begin.saturating_add(w.duration), w.tid))
        }).collect()
    };
    if due.is_empty() { return; }
    let rows = crate::task::scheduler::stall_snapshot();
    let mut ring = RING.lock();
    for wait in due {
        push(&mut ring, format!("stall-snapshot {wait}"));
        for row in &rows { push(&mut ring, format!("stall-thread {row}")); }
    }
}
/// Numeric click delivery marks; called in task context only.
pub fn record(label: &str, window: u64, serial: u64, ns: u64) {
    push(&mut RING.lock(), format!("{label} window={window} serial={serial} ns={ns}"));
}
pub fn text() -> String {
    let ring = RING.lock();
    let retained = ring.count.min(CAPACITY as u64) as usize;
    let mut out = format!("stall-buffer retained={retained} overwritten={}\n", ring.count.saturating_sub(CAPACITY as u64));
    for i in 0..retained {
        let slot = (ring.count as usize - retained + i) % CAPACITY;
        let _ = writeln!(out, "{}", String::from_utf8_lossy(&ring.rows[slot][..ring.lengths[slot]]));
    }
    out
}

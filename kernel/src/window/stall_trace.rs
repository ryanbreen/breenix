//! Bounded, always-on wait evidence. Only the suite parent samples and exports.
use alloc::{format, string::String, vec::Vec};
use core::fmt::Write;

const CAPACITY: usize = 512;
struct Wait { tid: u64, begin: u64, duration: u64, label: String, sampled: bool }
struct Budget {
    tid: u64, label: String, stamps: [u64; 5], diagnostic_begin: u64,
    diagnostic_ns: u64, diagnostics: u64, wait_ns: u64, wait_begin: u64, wait_label: String, last_ns: u64, phase: String, dirty: bool,
}
struct Ring { budgets: Vec<Budget>, rows: [[u8; 384]; CAPACITY], lengths: [usize; CAPACITY], count: u64, waits: Vec<Wait> }
static RING: spin::Mutex<Ring> = spin::Mutex::new(Ring {
    budgets: Vec::new(), rows: [[0; 384]; CAPACITY], lengths: [0; CAPACITY], count: 0, waits: Vec::new(),
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
    if op == "budget" {
        let phase = words.next().unwrap_or("unknown");
        let label: String = words.next().unwrap_or("unknown").chars().take(96).collect();
        if !ring.budgets.iter().any(|b| b.tid == tid && b.label == label) {
            if ring.budgets.len() == 16 { ring.budgets.remove(0); }
            ring.budgets.push(Budget { tid, label: label.clone(), stamps: [0; 5],
                diagnostic_begin: 0, diagnostic_ns: 0, diagnostics: 0, wait_ns: 0, wait_begin: 0, wait_label: String::new(), last_ns: begin,
                phase: String::new(), dirty: true });
        }
        if let Some(b) = ring.budgets.iter_mut().find(|b| b.tid == tid && b.label == label) {
            let index = match phase { "case-begin" => Some(0), "logic-begin" => Some(1),
                "logic-end" => Some(2), "report-begin" => Some(3), "report-end" => Some(4), _ => None };
            if let Some(i) = index { b.stamps[i] = begin; }
            if phase == "diagnostic-begin" { b.diagnostic_begin = begin; }
            if phase == "diagnostic-end" && b.diagnostic_begin != 0 {
                b.diagnostic_ns += begin.saturating_sub(b.diagnostic_begin);
                b.diagnostic_begin = 0;
                b.diagnostics += 1;
            }
            if phase == "parent-fork" { b.stamps[0] = begin; }
            b.last_ns = begin;
            b.phase = phase.chars().take(48).collect();
            b.dirty = true;
        }
    } else if op == "end" {
        if let Some(w) = ring.waits.iter().find(|w| w.tid == tid && w.begin == begin)
            .filter(|_| ring.budgets.iter().any(|b| b.tid == tid)) {
            let row = format!("wait-budget tid={tid} wait={} begin_ns={begin} end_ns={} deadline_ns={}",
                w.label, crate::signal::monotonic_nanos(), begin.saturating_add(w.duration));
            push(&mut ring, row);
        }
        if let Some(b) = ring.budgets.iter_mut().find(|b| b.tid == tid) {
            if b.wait_begin == begin {
                b.wait_ns += crate::signal::monotonic_nanos().saturating_sub(begin);
                b.wait_begin = 0;
                b.dirty = true;
            }
        }
        ring.waits.retain(|w| w.tid != tid || w.begin != begin);
    } else if op == "arm" {
        let Some(duration) = words.next().and_then(|v| v.parse::<u64>().ok()) else { return };
        if ring.waits.len() == 128 {
            push(&mut ring, String::from("stall-arm-overflow"));
            return;
        }
        push(&mut ring, format!("wait-budget tid={tid} wait={} begin_ns={begin} deadline_ns={} end_ns=0",
            words.clone().next().unwrap_or("wait"), begin.saturating_add(duration)));
        if let Some(b) = ring.budgets.iter_mut().find(|b| b.tid == tid) {
            b.wait_begin = begin;
            b.wait_label = words.clone().next().unwrap_or("wait").chars().take(96).collect();
            b.dirty = true;
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
    let mut ring = RING.lock();
    let retained = ring.count.min(CAPACITY as u64) as usize;
    let mut out = format!("stall-buffer retained={retained} overwritten={}\n", ring.count.saturating_sub(CAPACITY as u64));
    for i in 0..retained {
        let slot = (ring.count as usize - retained + i) % CAPACITY;
        let _ = writeln!(out, "{}", String::from_utf8_lossy(&ring.rows[slot][..ring.lengths[slot]]));
    }
    let now = crate::signal::monotonic_nanos();
    for b in &mut ring.budgets {
        if !b.dirty && b.stamps[4] != 0 { continue; }
        let _ = writeln!(out, "case-budget tid={} case={} case_begin_ns={} logic_begin_ns={} logic_end_ns={} report_begin_ns={} report_end_ns={} diagnostic_ns={} diagnostics={} diagnostic_begin_ns={} last_ns={} phase={} wait_ns={} wait_begin_ns={} last_wait={} snapshot_ns={now}",
            b.tid, b.label.replace(char::is_whitespace, "_"), b.stamps[0], b.stamps[1], b.stamps[2], b.stamps[3], b.stamps[4],
            b.diagnostic_ns, b.diagnostics, b.diagnostic_begin, b.last_ns, b.phase, b.wait_ns, b.wait_begin, b.wait_label.replace(char::is_whitespace, "_"));
        b.dirty = false;
    }
    out
}

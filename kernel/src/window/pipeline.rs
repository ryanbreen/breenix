//! W18: bounded, always-on frame-pipeline timestamps. Formatting is read-side only.
//! Columns are stage, object, related object, source input serial, monotonic ns.
//! Stages: input publication/read, window delivery/read, acquire begin/end,
//! commit begin/end, scene admission, render completion, PRESENT return,
//! physical/virtual presentation, and receipt publication (1 through 13), front publication (14), and driver
//! acceptance raster samples (15: native resource, packed counter/line, pixel).
//! Input trace flags (16): device, derived modifier bits plus SYNTHETIC, source serial.
//! Window destruction (17): window id, owner pid, reason (0 destroy, 1 exit, 2 exec).
//! Metadata-only publication (18) and READ_META observation (19): window id,
//! published commit id, processed watermark (possibly a derived input serial).
//! App frame boundaries (20..26), per-CPU queue lengths (27..34), current threads
//! (35..42), unavailable scheduler sample (43), sampling CPU (44); section 17.1.
//! Pending-render acceptance (45) and observed seal decision (46): window,
//! accepted commit, processed watermark. Stage 14 remains front publication.
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
/// Boundary observations only: no scheduler hot-path hooks and no formatted output.
pub(super) fn frame_stage(sample: &breengel_abi::FrameStage) {
    let ns = super::events::now_ns();
    let cpu_ns = crate::task::thread::current_thread_cpu_split_ns()
        .map_or(u64::MAX, |(user, system)| user.saturating_add(system));
    let cpu = crate::task::scheduler::current_cpu();
    let snapshot = crate::task::scheduler::try_liveness_snapshot(cpu);
    at(20 + u64::from(sample.stage), sample.id, sample.frame, cpu_ns, ns);
    at(44, sample.id, sample.frame, cpu as u64, ns);
    if let Some(snapshot) = snapshot {
        for (cpu, (&ready, &current)) in snapshot.per_cpu_ready_len.iter()
            .zip(snapshot.per_cpu_current.iter()).enumerate() {
            at(27 + cpu as u64, sample.id, sample.frame, ready, ns);
            at(35 + cpu as u64, sample.id, sample.frame, current, ns);
        }
    } else {
        at(43, sample.id, sample.frame, cpu as u64, ns);
    }
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

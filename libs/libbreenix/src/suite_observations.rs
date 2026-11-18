//! Fixed-slot observations retained across case/supervisor forks, formatted only on failure.
use super::{AtomicU64, Ordering, PAGE};
const BASE: usize = super::APP_OUTPUT_CLOCK + 1;
const FIELDS: usize = 7;
const OPS: &[&str] = &[
    "readdir-setup-done", "readdir-opendir-reached", "readdir-enumeration-start", "readdir-enumeration-end",
    "configure-parent", "configure-fixtures", "configure-verify", "configure-trace", "configure-arena",
    "window-wait", "window-registry", "window-composites", "window-registry-match", "window-timed-composite",
    "discard-outputs", "discard-children", "discard-devices", "discard-display", "discard-root-delete",
    "discard-remnants", "discard-reply",
];
pub(super) const USED_CELLS: usize = BASE + OPS.len() * FIELDS;
fn cell(page: usize, index: usize, field: usize) -> &'static AtomicU64 {
    unsafe { &*((page as *const AtomicU64).add(BASE + index * FIELDS + field)) }
}
fn now() -> u64 {
    libbreenix::time::now_monotonic().map_or(0, |t| t.as_nanos() as u64)
}
/// Aggregates repeated calls in one slot; an interrupted call retains started > completed.
/// Recording has no allocation, filesystem access, formatting or serial output.
pub struct Span { page: usize, index: Option<usize>, start: u64 }
impl Span {
    pub fn new(name: &str) -> Self {
        let page = PAGE.load(Ordering::Acquire);
        let index = OPS.iter().position(|&op| op == name);
        let start = if page != 0 && index.is_some() { now() } else { 0 };
        if let Some(index) = index.filter(|_| page != 0) {
            cell(page, index, 0).fetch_add(1, Ordering::Relaxed);
            let _ = cell(page, index, 4).compare_exchange(0, start, Ordering::Relaxed, Ordering::Relaxed);
            cell(page, index, 5).store(start, Ordering::Release);
        }
        Self { page, index, start }
    }
}
impl Drop for Span {
    fn drop(&mut self) {
        if let Some(index) = self.index.filter(|_| self.page != 0) {
            let end = now();
            let elapsed = if self.start == 0 || end == 0 { 0 } else { end.saturating_sub(self.start) };
            cell(self.page, index, 2).fetch_add(elapsed, Ordering::Relaxed);
            cell(self.page, index, 3).fetch_max(elapsed, Ordering::Relaxed);
            cell(self.page, index, 6).store(end, Ordering::Relaxed);
            cell(self.page, index, 1).fetch_add(1, Ordering::Release);
        }
    }
}
/// Keep the first guest timestamp, including across fragmented/replayed pipe reads.
pub fn stamp(name: &str, ns: u64) {
    let page = PAGE.load(Ordering::Acquire);
    if page == 0 || ns == 0 { return; }
    if let Some(index) = OPS.iter().position(|&op| op == name) {
        if cell(page, index, 4).compare_exchange(0, ns, Ordering::AcqRel, Ordering::Relaxed).is_ok() {
            cell(page, index, 5).store(ns, Ordering::Relaxed);
            cell(page, index, 6).store(ns, Ordering::Relaxed);
            cell(page, index, 0).store(1, Ordering::Relaxed);
            cell(page, index, 1).store(1, Ordering::Release);
        }
    }
}
/// The helper's result pipe carries diagnostic transport to its case-side reader.
pub fn libc_phases(bytes: &[u8]) {
    // Four short lines precede the unchanged result. Ignore an incomplete last line.
    for line in bytes.split_inclusive(|&b| b == b'\n').take(4) {
        if !line.ends_with(b"\n") { break; }
        let Ok(line) = core::str::from_utf8(line) else { continue };
        let mut words = line.split_whitespace();
        if words.next() != Some("OBS") { continue; }
        let (Some(name), Some(ns)) = (words.next(), words.next()) else { continue };
        if OPS[..4].contains(&name) && words.next().is_none() {
            if let Ok(ns) = ns.parse() { stamp(name, ns); }
        }
    }
}
pub(super) fn records(page: usize, suite: &str, case: &str, failed: bool) -> Vec<String> {
    if !failed { return Vec::new(); }
    let range = if suite == "libc" && case == "dirent-glob/readdir-many" { 0..4 }
        else if suite == "ui-testing" { 4..OPS.len() } else { return Vec::new(); };
    range.filter_map(|i| {
        let v: Vec<_> = (0..FIELDS).map(|f| cell(page, i, f).load(Ordering::Acquire)).collect();
        (v[0] != 0).then(|| format!("CASE_OBSERVATION {suite} {case} op={} started={} completed={} total_ns={} max_ns={} first_ns={} last_start_ns={} last_end_ns={}", OPS[i], v[0], v[1], v[2], v[3], v[4], v[5], v[6]))
    }).collect()
}

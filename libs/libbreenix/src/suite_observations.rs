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
// Filesystem rows identify the syscall and its possible kernel waits. Mount-wide
// counters cannot identify which of those waits belonged to one caller.
const UI_OPS: &[(&str, &str)] = &[
    ("arena-create", "may_wait=mount,admission,cache"),
    ("arena-resize", "may_wait=mount,admission,cache"),
    ("arena-header-write", "may_wait=mount,admission,cache"),
    ("arena-stat", "may_wait=mount,cache"),
    ("arena-open", "may_wait=mount,cache"),
    ("arena-header-read", "may_wait=mount,cache"),
    ("arena-map-headers", "may_wait=mount,cache"),
    ("arena-map-frame", "may_wait=mount,cache"),
    ("arena-map-tree", "may_wait=mount,cache"),
    ("arena-map-screen", "may_wait=mount,cache"),
    ("arena-close", "wait=descriptor-release"),
    ("arena-unmap", "wait=mapping-release"),
    ("arena-unlink", "may_wait=mount,admission,cache"),
    ("arena-rename", "may_wait=mount,admission,cache"),
    ("arena-copy-hash", "wait=cpu"),
    ("window-front-capture", "wait=front-and-copy"),
    ("scanout-readback", "wait=presentation"),
    ("tree-serialization", "wait=window-metadata"),
    ("input-ack", "wait=supervisor-input"),
    ("capture-stall-witness", "wait=capture-test-point"),
    ("capture-reader-result", "wait=reader-channel"),
    ("scenario-logic-outcome", "wait=logic-exit-and-pipe-eof"),
    ("scenario-controller-outcome", "wait=controller-exit-and-pipe-eof"),
    ("fixture-transcript", "wait=fixture-exit-and-pipe-eof"),
    ("fixture-read-artifacts", "may_wait=mount,cache"),
    ("artifact-directory", "may_wait=mount,admission,cache"),
    ("artifact-create", "may_wait=mount,admission,cache"),
    ("artifact-write", "may_wait=mount,admission,cache"),
    ("artifact-file-sync", "may_wait=mount,generation,cache"),
    ("artifact-rename", "may_wait=mount,admission,cache"),
    ("artifact-directory-sync", "may_wait=mount,generation,cache"),
    ("artifact-scan", "may_wait=mount,cache"),
    ("artifact-encode-png", "wait=cpu"),
    ("font-seed", "may_wait=mount,admission,cache"),
    ("font-log-write", "may_wait=mount,admission,cache"),
    ("font-final-launcher-launch", "wait=launch-and-window"),
    ("font-final-launcher-tree", "wait=input-and-window"),
    ("font-final-launcher-pixels", "wait=front-and-copy"),
    ("font-final-finish", "wait=session-teardown"),
    ("launcher-ack-unlink", "may_wait=mount,admission,cache"),
    ("launcher-catalog-write", "may_wait=mount,admission,cache"),
    ("launcher-ack-attach", "wait=window-registry-and-watch"),
    ("launcher-ack-read", "may_wait=mount,cache"),
    ("scenario-save-switch", "may_wait=mount,admission,cache"),
    ("scenario-artifact-checks", "may_wait=mount,cache"),
    ("ui-clock-open", "may_wait=mount,cache"),
    ("ui-clock-advance", "wait=epoch-acknowledgments"),
    ("capture-reader-job", "wait=arena-and-front"),
];
const FIXTURE_BASE: usize = OPS.len() + UI_OPS.len();
// A separate copy of the nested collector's operation rows. The exec'd
// fixture owns its own Scope and sends its failure records over a pipe.
const FIXTURE_OPS: usize = 33; // Shared capture, scenario and artifact rows above.
pub(super) const USED_CELLS: usize = BASE + (FIXTURE_BASE + FIXTURE_OPS) * FIELDS;

fn index(name: &str) -> Option<usize> {
    OPS.iter().position(|&op| op == name).or_else(|| {
        UI_OPS.iter().position(|&(op, _)| op == name).map(|i| OPS.len() + i)
    })
}
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
        let index = (page != 0).then(|| index(name)).flatten();
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
/// Time one named operation without retaining its arguments or result.
pub fn during<T>(name: &str, operation: impl FnOnce() -> T) -> T {
    let _span = Span::new(name);
    operation()
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
const KEYS: [&str; FIELDS] = [
    "started=", "completed=", "total_ns=", "max_ns=", "first_ns=", "last_start_ns=", "last_end_ns=",
];

/// Retain only bounded, known numeric rows from the selected nested case.
pub fn retain_fixture(transcript: &str, case: &str) {
    let page = PAGE.load(Ordering::Acquire);
    if page == 0 { return; }
    let prefix = format!("CASE_OBSERVATION runner-fixture {case} op=");
    for line in transcript.lines().filter_map(|line| line.strip_prefix(&prefix)).take(UI_OPS.len()) {
        let Some((op, fields)) = line.split_once(' ') else { continue };
        let Some(index) = UI_OPS[..FIXTURE_OPS].iter().position(|&(name, _)| name == op) else { continue };
        let mut values = [0u64; FIELDS];
        if !KEYS.iter().zip(&mut values).zip(fields.split_whitespace()).all(|((&key, value), token)| {
            token.strip_prefix(key).and_then(|digits| digits.parse().ok()).is_some_and(|n| {
                *value = n;
                true
            })
        }) || fields.split_whitespace().count() != FIELDS + 1 || values[0] == 0 {
            continue;
        }
        for (field, value) in values.into_iter().enumerate().skip(1) {
            cell(page, FIXTURE_BASE + index, field).store(value, Ordering::Relaxed);
        }
        cell(page, FIXTURE_BASE + index, 0).store(values[0], Ordering::Release);
    }
}

pub(super) fn records(page: usize, suite: &str, case: &str, failed: bool) -> Vec<String> {
    if !failed { return Vec::new(); }
    let range = match suite {
        "libc" if case == "dirent-glob/readdir-many" => 0..4,
        "ui-testing" => 4..FIXTURE_BASE + FIXTURE_OPS,
        "gui-apps" | "runner-fixture" => OPS.len()..FIXTURE_BASE,
        _ => return Vec::new(),
    };
    range.filter_map(|i| {
        let started = cell(page, i, 0).load(Ordering::Acquire);
        if started == 0 { return None; }
        let (name, wait, source) = if i < OPS.len() {
            (OPS[i], "", "")
        } else if i < FIXTURE_BASE {
            let (name, wait) = UI_OPS[i - OPS.len()];
            (name, wait, "")
        } else {
            let (name, wait) = UI_OPS[i - FIXTURE_BASE];
            (name, wait, " source=runner-fixture")
        };
        let mut line = format!("CASE_OBSERVATION {suite} {case} op={name} started={started}");
        for (f, key) in KEYS.iter().enumerate().skip(1) {
            use std::fmt::Write;
            let _ = write!(line, " {key}{}", cell(page, i, f).load(Ordering::Acquire));
        }
        if !wait.is_empty() {
            line.push(' ');
            line.push_str(wait);
        }
        line.push_str(source);
        Some(line)
    }).collect()
}

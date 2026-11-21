//! E24 coordinator 567: bounded, fork-shared marker receipts, serial only on failure.
use super::{AtomicU64, Ordering, PAGE};
use std::io::Read;
const BASE: usize = super::observations::USED_CELLS;
const SLOTS: usize = 12; // Four three-marker rounds in ftruncate-large.
const FIELDS: usize = 8; // generation, FREE, PENDING, validity, for each side.
pub(super) const USED_CELLS: usize = BASE + SLOTS * FIELDS;
fn cell(page: usize, index: usize, field: usize) -> &'static AtomicU64 {
    unsafe { &*((page as *const AtomicU64).add(BASE + index * FIELDS + field)) }
}
/// Counts of 0 are valid; validity is published separately.
pub fn sample(index: usize, after: bool, generation: u64, free: u64, pending: u64) {
    let page = PAGE.load(Ordering::Acquire);
    if page == 0 || index >= SLOTS { return; }
    let base = usize::from(after) * 4;
    cell(page, index, base).store(generation, Ordering::Relaxed);
    cell(page, index, base + 1).store(free, Ordering::Relaxed);
    cell(page, index, base + 2).store(pending, Ordering::Relaxed);
    cell(page, index, base + 3).store(1, Ordering::Release);
}
fn parse(text: &str) -> Option<(u64, u64, u64)> {
    let mut rows = text.lines().filter(|line| line.starts_with("root breenfs ") || line.starts_with("/ breenfs "));
    let row = rows.next()?;
    if rows.next().is_some() { return None; }
    let field = |key: &str| {
        let mut fields = row.split_whitespace().filter_map(|word| word.strip_prefix(key));
        let value = fields.next()?.parse::<u64>().ok()?;
        fields.next().is_none().then_some(value)
    };
    Some((field("durable_generation=")?, field("free_blocks=")?, field("pending_blocks=")?))
}
/// Read durable raw counts and retain the same failure-only receipt.
/// The BreenFS reclaim oracles use these counts for unlink and truncate.
pub fn sample_root(index: usize, after: bool) -> Option<(u64, u64, u64)> {
    let page = PAGE.load(Ordering::Acquire);
    if page == 0 || index >= SLOTS { return None; }
    let read = || {
        let mut text = String::new();
        std::fs::File::open("/proc/breenix/filesystems").ok()?
            .take(32 * 1024).read_to_string(&mut text).ok()?;
        parse(&text)
    };
    let counts = read();
    if let Some((generation, free, pending)) = counts {
        sample(index, after, generation, free, pending);
    } else {
        cell(page, index, usize::from(after) * 4 + 3).store(2, Ordering::Release);
    }
    counts
}
pub(super) fn records(page: usize, suite: &str, case: &str, failed: bool) -> Vec<String> {
    if !failed || suite != "files-io" || !matches!(case,
        "descriptors/unlink-reclaim" | "metadata/ftruncate-large"
            | "metadata/ftruncate-large-open" | "metadata/statfs-allocation") {
        return Vec::new();
    }
    (0..SLOTS).filter_map(|index| {
        let before = cell(page, index, 3).load(Ordering::Acquire);
        let after = cell(page, index, 7).load(Ordering::Acquire);
        if before == 0 && after == 0 { return None; }
        let number = |side: usize, field: usize, valid| match valid {
            1 => cell(page, index, side * 4 + field).load(Ordering::Relaxed).to_string(),
            2 => String::from("unavailable"),
            _ => String::from("unreached"),
        };
        Some(format!("CASE_MARKER {suite} {case} marker={index} before_free={} before_pending={} after_free={} after_pending={} before_generation={} after_generation={}",
            number(0, 1, before), number(0, 2, before), number(1, 1, after), number(1, 2, after),
            number(0, 0, before), number(1, 0, after)))
    }).collect()
}

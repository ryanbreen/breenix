//! Allocation-free, bounded ownership capture. Formatting follows capture.

#[derive(Clone, Copy)]
pub struct Row {
    pub kind: &'static str,
    pub pid: u64,
    pub root: u64,
    pub va: u64,
    pub end: u64,
    pub pa: u64,
    pub flags: u64,
    pub level: u8,
    pub present: u16,
    pub nonzero: u16,
    pub name: [u8; 32],
}
impl Row {
    pub const EMPTY: Self = Self { kind: "", pid: 0, root: 0, va: 0, end: 0,
        pa: 0, flags: 0, level: 0, present: 0, nonzero: 0, name: [0; 32] };
}

pub struct Snapshot<const N: usize> {
    rows: [Row; N],
    len: usize,
    pub overflow: bool,
}
impl<const N: usize> Snapshot<N> {
    pub const fn new() -> Self { Self { rows: [Row::EMPTY; N], len: 0, overflow: false } }
    pub fn clear(&mut self) { self.len = 0; self.overflow = false; }
    pub fn rows(&self) -> &[Row] { &self.rows[..self.len] }
    pub fn push(&mut self, row: Row) -> bool {
        if self.len == N { self.overflow = true; return false; }
        self.rows[self.len] = row;
        self.len += 1;
        true
    }
}

pub struct Entry {
    pub present: bool,
    pub nonzero: bool,
    pub child: Option<u64>,
    pub leaf: Option<(u64, u64)>,
}
impl Entry {
    pub const fn empty() -> Self { Self { present: false, nonzero: false, child: None, leaf: None } }
    pub const fn table(pa: u64) -> Self { Self { present: true, nonzero: true, child: Some(pa), leaf: None } }
    pub const fn leaf(pa: u64, flags: u64) -> Self { Self { present: true, nonzero: true, child: None, leaf: Some((pa, flags)) } }
}

/// Only owned child tables may be exposed; inherited kernel branches are skipped.
/// Callers exclude mutations and wait for any fork copy before walking.
pub trait Source {
    fn entry(&self, table: u64, index: usize, level: u8) -> Entry;
}

/// Four levels, 512 descriptors per table, stopping when the row capacity fills.
/// A table row records both present and nonzero counts, including empty tables.
/// A leaf row's end expresses its page/block size. Physical frames stay explicit
/// so shared leaves can be deduplicated by consumers.
pub fn walk<const N: usize>(source: &impl Source, root: u64, pid: u64, out: &mut Snapshot<N>) {
    visit(source, root, 4, 0, pid, root, out);
}
fn visit<const N: usize>(source: &impl Source, table: u64, level: u8, va: u64,
    pid: u64, root: u64, out: &mut Snapshot<N>) {
    let span = 1u64 << (12 + 9 * u32::from(level));
    let mut row = Row { kind: "table", pid, root, va, end: va + span, pa: table, level, ..Row::EMPTY };
    for index in 0..512 {
        let entry = source.entry(table, index, level);
        row.present += u16::from(entry.present);
        row.nonzero += u16::from(entry.nonzero);
    }
    if !out.push(row) { return; }
    let step = span / 512;
    for index in 0..512 {
        let entry = source.entry(table, index, level);
        let start = va + index as u64 * step;
        if let Some(child) = entry.child {
            if level > 1 { visit(source, child, level - 1, start, pid, root, out); }
        } else if let Some((pa, flags)) = entry.leaf {
            out.push(Row { kind: "leaf", pid, root, va: start, end: start + step,
                pa, flags, level, ..Row::EMPTY });
        }
        if out.overflow { return; }
    }
}

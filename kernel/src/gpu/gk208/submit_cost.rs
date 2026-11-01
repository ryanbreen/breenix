//! Exclusive native submit accounting. Timestamps are supplied by the caller so
//! the partition, nesting and aggregation can be checked without a device boot.
use alloc::vec::Vec;
use core::cell::Cell;

pub const PHASES: [&str; 13] = ["entry", "copy_in", "validation", "admission_poll",
    "setup", "translate", "frame_prepare", "fence_bookkeeping", "native_submit",
    "doorbell", "copy_out", "cleanup", "other"];
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum Phase { Entry, Copy, Validate, Admission, Setup, Translate, FramePrepare,
    Fence, Native, Doorbell, Output, Cleanup, Other }

pub struct Split {
    start: u64,
    last: Cell<u64>,
    current: Cell<Phase>,
    elapsed: Cell<[u64; PHASES.len()]>,
}
impl Split {
    pub fn new(start: u64) -> Self {
        Self { start, last: Cell::new(start), current: Cell::new(Phase::Entry),
            elapsed: Cell::new([0; PHASES.len()]) }
    }
    pub fn switch(&self, next: Phase, now: u64) -> Phase {
        let previous = self.current.replace(next);
        let mut elapsed = self.elapsed.get();
        elapsed[previous as usize] += now.saturating_sub(self.last.replace(now));
        self.elapsed.set(elapsed);
        previous
    }
    pub fn finish(&self, now: u64) -> (u64, [u64; PHASES.len()]) {
        let mut elapsed = self.elapsed.get();
        elapsed[self.current.get() as usize] += now.saturating_sub(self.last.get());
        (now.saturating_sub(self.start), elapsed)
    }
}

pub type Row = [u64; 3 + PHASES.len()];
/// Per-owner totals retain ordinary no-span submits without growing per draw.
/// An overflow/allocation refusal is explicit and must increment dropped.
pub struct Totals { rows: Vec<Row> }
impl Default for Totals { fn default() -> Self { Self::new() } }
impl Totals {
    pub const fn new() -> Self { Self { rows: Vec::new() } }
    pub fn record(&mut self, owner: u64, total: u64, phases: &[u64; PHASES.len()]) -> bool {
        if owner == 0 || phases.iter().sum::<u64>() != total { return false; }
        let index = match self.rows.iter().position(|row| row[0] == owner) {
            Some(index) => index,
            None => {
                if self.rows.len() == 1024 || self.rows.try_reserve(1).is_err() { return false; }
                let mut row = [0; 3 + PHASES.len()]; row[0] = owner;
                self.rows.push(row); self.rows.len() - 1
            }
        };
        let row = &mut self.rows[index]; row[1] += 1; row[2] += total;
        for (sum, elapsed) in row[3..].iter_mut().zip(phases) { *sum += elapsed; }
        true
    }
    pub fn len(&self) -> usize { self.rows.len() }
    pub fn is_empty(&self) -> bool { self.rows.is_empty() }
    pub fn drain(&mut self) -> Vec<Row> { core::mem::take(&mut self.rows) }
}

//! Exclusive native submit accounting. Timestamps are supplied by the caller so
//! the partition, nesting and aggregation can be checked without a device boot.
use alloc::vec::Vec;
use core::cell::Cell;

pub const PHASES: [&str; 34] = ["entry", "copy_in", "validation", "admission_poll",
    "setup", "translate", "frame_prepare", "fence_bookkeeping", "native_submit",
    "doorbell", "copy_out", "cleanup", "other",
    "publication", "command_upload", "descriptor_upload", "bar_flush", "gp_put",
    "translate_encode", "translate_compact", "translate_batch",
    "frame_alloc", "frame_copy", "frame_maintain", "frame_build",
    "frame_regions", "frame_map", "frame_relocate",
    "admission_core_poll", "admission_inspect", "admission_pause", "admission_sleep",
    "translate_validate", "translate_emit"];
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum Phase { Entry, Copy, Validate, Admission, Setup, Translate, FramePrepare,
    Fence, Native, Doorbell, Output, Cleanup, Other,
    Publication, CommandUpload, DescriptorUpload, BarFlush, GpPut,
    TranslateEncode, TranslateCompact, TranslateBatch,
    FrameAlloc, FrameCopy, FrameMaintain, FrameBuild, FrameRegions, FrameMap, FrameRelocate,
    AdmissionPoll, AdmissionInspect, AdmissionPause, AdmissionSleep,
    TranslateValidate, TranslateEmit }

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

/// CPU-written payload bytes, not PCIe wire traffic or GPU-rendered pixels.
pub const BYTE_NAMES: [&str; 6] = ["command", "descriptor", "receipt", "frame", "frame_zero", "gp_put"];
#[derive(Clone, Copy)]
#[repr(usize)]
pub enum Bytes { Command, Descriptor, Receipt, Frame, FrameZero, GpPut }
/// Four hardware receipt slots, then unpublished oldest, scanout, reading, busy.
pub const WAIT_NAMES: [&str; 8] = ["slot0", "slot1", "slot2", "slot3", "unpublished", "scanout", "reading", "busy"];
/// [episodes, inclusive_ns, first_serial, last_serial, first_native_sequence, last_native_sequence].
pub type Wait = [u64; 6];
/// Per-syscall facts need no allocation or global lock while timing the path.
pub struct Facts {
    bytes: Cell<[u64; BYTE_NAMES.len()]>,
    waits: Cell<[Wait; WAIT_NAMES.len()]>,
    waiting: Cell<Option<(usize, u64, u64, u64)>>,
}
impl Default for Facts { fn default() -> Self { Self::new() } }
impl Facts {
    pub fn new() -> Self { Self { bytes: Cell::new([0; BYTE_NAMES.len()]),
        waits: Cell::new([[0; 6]; WAIT_NAMES.len()]), waiting: Cell::new(None) } }
    pub fn bytes(&self, kind: Bytes, count: u64) {
        let mut bytes = self.bytes.get(); bytes[kind as usize] += count; self.bytes.set(bytes);
    }
    /// Update only when the blocking identity changes; repeat polls are one episode.
    /// These latencies include polling/publication and are NOT added to phase times.
    pub fn wait_on(&self, key: Option<(usize, u64, u64)>, now: u64) {
        let old = self.waiting.get();
        if old.map(|(b,s,n,_)|(b,s,n)) == key { return; }
        if let Some((bucket, serial, sequence, start)) = old {
            let mut waits = self.waits.get(); let row = &mut waits[bucket];
            if row[0] == 0 { row[2] = serial; row[4] = sequence; }
            row[0] += 1; row[1] += now.saturating_sub(start);
            row[3] = serial; row[5] = sequence; self.waits.set(waits);
        }
        self.waiting.set(key.map(|(b,s,n)|(b,s,n,now)));
    }
    pub fn finish(&self, now: u64) -> ([u64; BYTE_NAMES.len()], [Wait; WAIT_NAMES.len()]) {
        self.wait_on(None, now); (self.bytes.get(), self.waits.get())
    }
}

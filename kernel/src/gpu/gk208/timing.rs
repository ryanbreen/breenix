//! Bounded per-draw cost records for the coordinator's corpus diagnostic.
use alloc::{string::String, vec::Vec};
#[cfg(target_os = "none")]
use alloc::format;
use core::fmt::Write;

pub fn collecting(probe: bool, requested: bool) -> bool { probe || requested }

// 161810 produced 23,835 ordinary rows. 32,768 retains that case with
// 37% headroom; split/blocked records have independent 8,192-row bounds.
pub const MAX_ROWS: usize = 32768;
const MAX_READBACKS: usize = 8192;

#[derive(Clone, Copy, Default)]
pub struct ReadbackSplit { pub wait_ns: u64, pub copy_ns: u64 }
impl ReadbackSplit {
    pub fn wait(&mut self, start: u64, end: u64) { self.wait_ns += end.saturating_sub(start); }
    pub fn copy(&mut self, start: u64, end: u64) { self.copy_ns += end.saturating_sub(start); }
}

pub struct Recorder {
    owner: u64,
    draw: u64,
    rows: Vec<[u64; 5]>,
    dropped: u64,
    readbacks: Vec<[u64; 8]>,
    blocked: Vec<[u64; 7]>,
    vblank: [[u64; 2]; 2],
    vblank_used: usize,
}
impl Recorder {
    pub const fn new() -> Self { Self { owner: 0, draw: 0, rows: Vec::new(), dropped: 0, readbacks: Vec::new(), blocked: Vec::new(), vblank: [[0; 2]; 2], vblank_used: 0 } }
    /// The clear-rate client brackets its timed work with two read-only samples.
    pub fn sample_vblank(&mut self, at: u64, count: u16) {
        if self.vblank_used < self.vblank.len() {
            self.vblank[self.vblank_used] = [at, u64::from(count)];
            self.vblank_used += 1;
        } else { self.dropped += 1; }
    }
    pub fn begin_draw(&mut self, owner: u64) -> u64 {
        if owner != self.owner { self.owner = owner; self.draw = 0; }
        self.draw += 1;
        self.draw
    }
    pub fn finish(&mut self, owner: u64, draw: u64, kind: u64, start: u64, end: u64) {
        if owner == 0 || owner != self.owner { return; }
        if self.rows.len() == MAX_ROWS || self.rows.try_reserve(1).is_err() { self.dropped += 1; return; }
        self.rows.push([owner, draw, kind, start, end.saturating_sub(start)]);
    }
    /// [owner, draw, start/id, total_ns, wait_ns, copy_ns, polls, spin_batches].
    pub fn readback(&mut self, row: [u64; 8]) {
        if self.readbacks.len() == MAX_READBACKS || self.readbacks.try_reserve(1).is_err() {
            self.dropped += 1;
        } else { self.readbacks.push(row); }
    }
    /// [readback_id, stage, reason, start_ns, elapsed_ns, tick_before, tick_after].
    pub fn blocked(&mut self, row: [u64; 7]) {
        if self.blocked.len() == MAX_READBACKS || self.blocked.try_reserve(1).is_err() {
            self.dropped += 1;
        } else { self.blocked.push(row); }
    }
    pub fn drain(&mut self, batch: u64) -> Option<String> {
        if self.rows.is_empty() && self.readbacks.is_empty() && self.blocked.is_empty() && self.dropped == 0 && self.vblank_used == 0 { return None; }
        let capacity = self.rows.len().checked_mul(128)?
            .checked_add(self.readbacks.len().checked_mul(192)?)?
            .checked_add(self.blocked.len().checked_mul(168)?)?.checked_add(512)?;
        let mut text = String::new();
        text.try_reserve_exact(capacity).ok()?;
        let _ = write!(text, "GT710_TIMING_V1 {{\"batch\":{},\"dropped\":{},\"rows\":[", batch, self.dropped);
        for (n, row) in self.rows.drain(..).enumerate() {
            if n != 0 { text.push(','); }
            let _ = write!(text, "[{},{},{},{},{}]", row[0], row[1], row[2], row[3], row[4]);
        }
        self.dropped = 0;
        text.push(']');
        if self.vblank_used != 0 {
            text.push_str(",\"vblank_samples\":[");
            for (n, row) in self.vblank[..self.vblank_used].iter().enumerate() {
                if n != 0 { text.push(','); }
                let _ = write!(text, "[{},{}]", row[0], row[1]);
            }
            text.push(']');
            self.vblank_used = 0;
        }
        if !self.readbacks.is_empty() || !self.blocked.is_empty() {
            text.push_str(",\"readbacks\":[");
            for (n, row) in self.readbacks.drain(..).enumerate() {
                if n != 0 { text.push(','); }
                let _ = write!(text, "[{},{},{},{},{},{},{},{}]", row[0],row[1],row[2],row[3],row[4],row[5],row[6],row[7]);
            }
            text.push_str("],\"blocked\":[");
            for (n, row) in self.blocked.drain(..).enumerate() {
                if n != 0 { text.push(','); }
                let _ = write!(text, "[{},{},{},{},{},{},{}]", row[0],row[1],row[2],row[3],row[4],row[5],row[6]);
            }
            text.push(']');
        }
        text.push('}');
        Some(text)
    }
}

#[cfg(target_os = "none")]
mod runtime {
    use super::*;
    static REQUESTED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
    pub fn enable() { REQUESTED.store(true, core::sync::atomic::Ordering::Release); }
    pub fn enabled() -> bool { collecting(cfg!(feature = "gt710_core_probe"), REQUESTED.load(core::sync::atomic::Ordering::Acquire)) }
    static DATA: spin::Mutex<Recorder> = spin::Mutex::new(Recorder::new());
    static BATCH: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(1);
    fn with<R>(f: impl FnOnce(&mut Recorder) -> R) -> R {
        crate::per_cpu::preempt_disable();
        let result = f(&mut DATA.lock());
        crate::per_cpu::preempt_enable();
        result
    }
    pub fn begin_draw(owner: u64) { if enabled() { with(|r| { r.begin_draw(owner); }); } }
    pub struct Guard { owner: u64, kind: u64, start: u64 }
    impl Guard {
        pub fn new(owner: u64, kind: u64) -> Self {
            Self { owner, kind, start: crate::signal::monotonic_nanos() }
        }
    }
    impl Drop for Guard {
        fn drop(&mut self) {
            let end = crate::signal::monotonic_nanos();
            with(|r| r.finish(self.owner, r.draw, self.kind, self.start, end));
        }
    }
    pub fn phase(kind: u64) -> Option<Guard> { enabled().then(|| Guard::new(crate::gpu::caller().unwrap_or(0), kind)) }
    pub fn vblank_sample(at: u64, count: u16) { with(|r| r.sample_vblank(at, count)); }
    /// Diagnostic only: no wait policy or completion authority is changed.
    pub struct Readback {
        owner: u64, draw: u64, start: u64,
        split: core::cell::Cell<ReadbackSplit>,
        polls: core::cell::Cell<u64>, spins: core::cell::Cell<u64>,
    }
    impl Readback {
        pub fn begin(owner: u64) -> Option<Self> {
            enabled().then(|| Self { owner, draw: with(|r| r.draw),
                start: crate::signal::monotonic_nanos(), split: core::cell::Cell::new(ReadbackSplit::default()),
                polls: core::cell::Cell::new(0), spins: core::cell::Cell::new(0) })
        }
        pub fn phase(&self, copy: bool) -> ReadbackPhase<'_> {
            ReadbackPhase { trace: self, start: crate::signal::monotonic_nanos(), copy }
        }
        pub fn poll(&self) { self.polls.set(self.polls.get()+1); }
        pub fn spin(&self) { self.spins.set(self.spins.get()+1); }
        pub fn blocked(&self, stage: u64, reason: u64, start: u64, tick: u64) {
            let end = crate::signal::monotonic_nanos();
            let after = crate::time::get_ticks();
            with(|r| r.blocked([self.start,stage,reason,start,end.saturating_sub(start),tick,after]));
        }
    }
    impl Drop for Readback {
        fn drop(&mut self) {
            let total = crate::signal::monotonic_nanos().saturating_sub(self.start);
            let split = self.split.get();
            with(|r| r.readback([self.owner,self.draw,self.start,total,split.wait_ns,split.copy_ns,self.polls.get(),self.spins.get()]));
        }
    }
    pub struct ReadbackPhase<'a> { trace: &'a Readback, start: u64, copy: bool }
    impl Drop for ReadbackPhase<'_> {
        fn drop(&mut self) {
            let end = crate::signal::monotonic_nanos();
            let mut split = self.trace.split.get();
            if self.copy { split.copy(self.start,end); } else { split.wait(self.start,end); }
            self.trace.split.set(split);
        }
    }
    pub fn export() -> Option<(String, String)> {
        let batch = BATCH.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        let text = with(|r| r.drain(batch))?;
        let anchor = format!("GT710_TIMING_BULK batch={} bytes={} sha256={}", batch, text.len(), super::super::hash(text.as_bytes()));
        Some((text, anchor))
    }
}
#[cfg(target_os = "none")]
pub use runtime::{enable, enabled, begin_draw, phase, export, vblank_sample, Guard, Readback};

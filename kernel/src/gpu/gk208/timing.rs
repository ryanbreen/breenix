//! Bounded per-draw cost records for the coordinator's corpus diagnostic.
use alloc::{format, string::String, vec::Vec};
use core::fmt::Write;

pub struct Recorder {
    owner: u64,
    draw: u64,
    rows: Vec<[u64; 5]>,
    dropped: u64,
}
impl Recorder {
    pub const fn new() -> Self { Self { owner: 0, draw: 0, rows: Vec::new(), dropped: 0 } }
    pub fn begin_draw(&mut self, owner: u64) -> u64 {
        if owner != self.owner { self.owner = owner; self.draw = 0; }
        self.draw += 1;
        self.draw
    }
    pub fn finish(&mut self, owner: u64, draw: u64, kind: u64, start: u64, end: u64) {
        if owner == 0 || owner != self.owner { return; }
        if self.rows.len() == 16384 || self.rows.try_reserve(1).is_err() { self.dropped += 1; return; }
        self.rows.push([owner, draw, kind, start, end.saturating_sub(start)]);
    }
    pub fn drain(&mut self, batch: u64) -> Option<String> {
        if self.rows.is_empty() && self.dropped == 0 { return None; }
        let mut text = format!("GT710_TIMING_V1 {{\"batch\":{},\"dropped\":{},\"rows\":[", batch, self.dropped);
        for (n, row) in self.rows.drain(..).enumerate() {
            if n != 0 { text.push(','); }
            let _ = write!(text, "[{},{},{},{},{}]", row[0], row[1], row[2], row[3], row[4]);
        }
        self.dropped = 0;
        text.push_str("]}");
        Some(text)
    }
}

#[cfg(target_os = "none")]
mod runtime {
    use super::*;
    static DATA: spin::Mutex<Recorder> = spin::Mutex::new(Recorder::new());
    static BATCH: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(1);
    fn with<R>(f: impl FnOnce(&mut Recorder) -> R) -> R {
        crate::per_cpu::preempt_disable();
        let result = f(&mut DATA.lock());
        crate::per_cpu::preempt_enable();
        result
    }
    pub fn begin_draw(owner: u64) { with(|r| { r.begin_draw(owner); }); }
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
    pub fn phase(kind: u64) -> Guard { Guard::new(crate::gpu::caller().unwrap_or(0), kind) }
    pub fn export() -> Option<(String, String)> {
        let batch = BATCH.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        let text = with(|r| r.drain(batch))?;
        let anchor = format!("GT710_TIMING_BULK batch={} bytes={} sha256={}", batch, text.len(), super::super::hash(text.as_bytes()));
        Some((text, anchor))
    }
}
#[cfg(target_os = "none")]
pub use runtime::{begin_draw, phase, export, Guard};

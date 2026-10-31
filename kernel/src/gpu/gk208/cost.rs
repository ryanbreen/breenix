//! Fixed native phase costs, armed by an explicit case-metrics observation.
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

pub const NAMES: [&str; 24] = [
    "submit_n", "submit_ns", "wait_n", "wait_ns", "read_n", "read_ns",
    "present_n", "present_ns", "upload_n", "upload_ns", "pack_n", "pack_ns",
    "span_prep_n", "span_prep_ns", "span_map_n", "span_map_ns",
    "span_upload_n", "span_upload_ns", "translate_n", "translate_ns",
    "frame_prep_n", "frame_prep_ns", "evo_n", "evo_ns",
];
pub const WORDS: usize = NAMES.len();
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum Slot { Submit, Wait, Read, Present, Upload, Pack, SpanPrepare, SpanMap,
    SpanUpload, Translate, FramePrepare, Evo }
impl Slot {
    const ALL: [Self; 12] = [Self::Submit, Self::Wait, Self::Read, Self::Present,
        Self::Upload, Self::Pack, Self::SpanPrepare, Self::SpanMap, Self::SpanUpload,
        Self::Translate, Self::FramePrepare, Self::Evo];
}
pub fn slot_for_op(op: u64) -> Option<Slot> {
    let op = u32::try_from(op).ok()?;
    Some(match op {
        gpu_abi::op::SUBMIT => Slot::Submit,
        gpu_abi::op::WAIT => Slot::Wait,
        gpu_abi::op::READBACK => Slot::Read,
        gpu_abi::op::PRESENT => Slot::Present,
        gpu_abi::op::UPLOAD => Slot::Upload,
        _ => return None,
    })
}
pub struct Recorder { enabled: AtomicBool, values: [AtomicU64; WORDS] }
impl Recorder {
    pub const fn new() -> Self {
        Self { enabled: AtomicBool::new(false), values: [const { AtomicU64::new(0) }; WORDS] }
    }
    pub fn enabled(&self) -> bool { self.enabled.load(Ordering::Acquire) }
    pub fn record(&self, slot: Slot, start: u64, end: u64) {
        if !self.enabled() { return; }
        let index = slot as usize * 2;
        self.values[index + 1].fetch_add(end.saturating_sub(start), Ordering::Relaxed);
        self.values[index].fetch_add(1, Ordering::Release);
    }
    /// Return the current totals and arm collection for subsequent operations.
    pub fn snapshot(&self) -> [u64; WORDS] {
        let mut words = [0; WORDS];
        for slot in Slot::ALL {
            let index = slot as usize * 2;
            words[index] = self.values[index].load(Ordering::Acquire);
            words[index + 1] = self.values[index + 1].load(Ordering::Acquire);
        }
        self.enabled.store(true, Ordering::Release);
        words
    }
}
#[cfg(target_os = "none")]
mod runtime {
    use super::*;
    static DATA: Recorder = Recorder::new();
    pub struct Guard { slot: Slot, start: u64 }
    impl Drop for Guard {
        fn drop(&mut self) { DATA.record(self.slot, self.start, crate::signal::monotonic_nanos()); }
    }
    pub fn phase(slot: Slot) -> Option<Guard> {
        DATA.enabled().then(|| Guard { slot, start: crate::signal::monotonic_nanos() })
    }
    pub fn for_op(op: u64) -> Option<Guard> { phase(slot_for_op(op)?) }
    pub fn snapshot() -> [u64; WORDS] { DATA.snapshot() }
}
#[cfg(target_os = "none")]
pub use runtime::{for_op, phase, snapshot};

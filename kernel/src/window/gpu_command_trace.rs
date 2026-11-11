//! Bounded numeric GPU submissions, retained independently of accounting windows.
use alloc::{string::String, vec::Vec};
use core::fmt::Write;
use core::sync::atomic::{AtomicU64, Ordering::{Acquire, Release, Relaxed, AcqRel}};

// Use one CAS attempt per update and count contention instead of waiting.
struct Slot {
    version: AtomicU64,
    words: [AtomicU64; 9],
}
impl Slot {
    const fn new() -> Self {
        Self { version: AtomicU64::new(0), words: [const { AtomicU64::new(0) }; 9] }
    }
    fn update(&self, f: impl FnOnce([u64; 9]) -> Option<[u64; 9]>) -> bool {
        let version = self.version.load(Acquire);
        if version & 1 != 0 || self.version.compare_exchange(version, version.wrapping_add(1), AcqRel, Relaxed).is_err() {
            return false;
        }
        let old = core::array::from_fn(|i| self.words[i].load(Relaxed));
        let next = f(old);
        if let Some(words) = next {
            for (dst, value) in self.words.iter().zip(words) { dst.store(value, Relaxed); }
        }
        self.version.store(version.wrapping_add(2), Release);
        next.is_some()
    }
    fn read(&self) -> Option<[u64; 9]> {
        let version = self.version.load(Acquire);
        if version & 1 != 0 { return None; }
        let words = core::array::from_fn(|i| self.words[i].load(Relaxed));
        core::sync::atomic::fence(Acquire);
        (self.version.load(Relaxed) == version).then_some(words)
    }
}

pub struct Commands<const N: usize> {
    slots: [Slot; N],
    next: AtomicU64,
    lost: AtomicU64,
}
impl<const N: usize> Commands<N> {
    pub const fn new() -> Self {
        Self { slots: [const { Slot::new() }; N], next: AtomicU64::new(0), lost: AtomicU64::new(0) }
    }
    pub fn begin(&self, ns: u64, pid: u64, tid: u64, transport: u64, kind: u64, cmd_bytes: u64, payload_bytes: u64) -> u64 {
        let seq = self.next.fetch_add(1, Relaxed) + 1;
        if !self.slots[(seq as usize - 1) % N].update(|old|
            (old[0] < seq).then_some([seq, ns, 0, pid, tid, transport, kind, cmd_bytes, payload_bytes])) {
            self.lost.fetch_add(1, Relaxed);
        }
        seq
    }
    pub fn finish(&self, seq: u64, ns: u64) {
        if !self.slots[(seq as usize - 1) % N].update(|mut old| {
            if old[0] != seq { return None; }
            old[2] = ns;
            Some(old)
        }) {
            // Eviction is already accounted as overwritten, not a failed write.
            if self.slots[(seq as usize - 1) % N].read().is_none_or(|old| old[0] == seq) {
                self.lost.fetch_add(1, Relaxed);
            }
        }
    }
    pub fn text(&self) -> String {
        let count = self.next.load(Acquire);
        let mut rows = Vec::with_capacity(N);
        for slot in &self.slots {
            if let Some(row) = slot.read() {
                if row[0] != 0 && row[0] <= count && row[0] > count.saturating_sub(N as u64) { rows.push(row); }
            }
        }
        rows.sort_unstable_by_key(|row| row[0]);
        let mut out = String::new();
        let _ = writeln!(out, "gpu-command-ring retained={} overwritten={} lost={}", rows.len(), count.saturating_sub(N as u64), self.lost.load(Relaxed));
        for [seq, begin, end, pid, tid, transport, kind, cmd_bytes, payload_bytes] in rows {
            let _ = writeln!(out, "gpu-command seq={seq} begin_ns={begin} return_ns={end} pid={pid} tid={tid} transport={transport} type={kind} cmd_bytes={cmd_bytes} payload_bytes={payload_bytes}");
        }
        out
    }
}

// Kernel bindings. No formatting, allocation, scheduler or process-manager lock
// on submission. transport 1 = MMIO 2D, 2 = PCI (including VirGL).
static COMMANDS: Commands<2048> = Commands::new();
pub struct Stamp { seq: u64 }
pub fn begin(transport: u64, kind: u32, cmd_bytes: u32, payload_bytes: u32) -> Stamp {
    let (pid, tid) = crate::task::thread::current_accounting_identity().unwrap_or((0, 0));
    Stamp { seq: COMMANDS.begin(crate::signal::monotonic_nanos(), pid, tid, transport,
                                kind as u64, cmd_bytes as u64, payload_bytes as u64) }
}
impl Drop for Stamp {
    fn drop(&mut self) { COMMANDS.finish(self.seq, crate::signal::monotonic_nanos()); }
}
pub fn text() -> String { COMMANDS.text() }

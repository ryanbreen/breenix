//! Active VirGL request and doorbell boundaries, independent of accounting windows.
use alloc::{string::String, vec::Vec};
use core::fmt::Write;
use core::sync::atomic::{AtomicU64, Ordering::{Acquire, Relaxed}};
use super::gpu_command_trace::Slot;

/// Commands and notifications share a bounded sequence space. Queue sequence numbers
/// join a notification to the requests published before it, including batched kicks.
pub struct Commands<const N: usize> {
    slots: [Slot<12>; N],
    next: AtomicU64,
    lost: AtomicU64,
}
impl<const N: usize> Commands<N> {
    pub const fn new() -> Self {
        Self { slots: [const { Slot::new() }; N], next: AtomicU64::new(0), lost: AtomicU64::new(0) }
    }
    fn begin(&self, mut row: [u64; 12]) -> u64 {
        let seq = self.next.fetch_add(1, Relaxed) + 1;
        row[0] = seq;
        if !self.slots[(seq as usize - 1) % N].update(|old| (old[0] < seq).then_some(row)) {
            self.lost.fetch_add(1, Relaxed);
        }
        seq
    }
    pub fn submit(&self, ns: u64, pid: u64, tid: u64, ctx: u32, kind: u32,
                  cmd_bytes: u64, payload_bytes: u64, queue_seq: u64, fence: u64) -> u64 {
        self.begin([0, ns, 0, pid, tid, ctx as u64, kind as u64, cmd_bytes, payload_bytes, queue_seq, fence, 1])
    }
    pub fn doorbell(&self, ns: u64, pid: u64, tid: u64, through: u64) -> u64 {
        self.begin([0, ns, 0, pid, tid, 0, 0, 0, 0, through, 0, 2])
    }
    pub fn finish(&self, seq: u64, ns: u64) {
        if seq == 0 { return; }
        let slot = &self.slots[(seq as usize - 1) % N];
        if !slot.update(|mut old| {
            if old[0] != seq { return None; }
            old[2] = ns;
            Some(old)
        }) && slot.read().is_none_or(|old| old[0] == seq) {
            self.lost.fetch_add(1, Relaxed);
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
        let _ = writeln!(out, "gpu-core-command-ring retained={} overwritten={} lost={}", rows.len(), count.saturating_sub(N as u64), self.lost.load(Relaxed));
        for [seq, begin, end, pid, tid, context, kind, cmd_bytes, payload_bytes, queue_seq, fence, event] in rows {
            if event == 1 {
                let _ = writeln!(out, "gpu-core-command seq={seq} submit_ns={begin} fence_return_ns={end} pid={pid} tid={tid} context={context} type={kind} cmd_bytes={cmd_bytes} payload_bytes={payload_bytes} queue_seq={queue_seq} fence={fence}");
            } else {
                let _ = writeln!(out, "gpu-core-doorbell seq={seq} begin_ns={begin} return_ns={end} pid={pid} tid={tid} through={queue_seq}");
            }
        }
        out
    }
}

// Kernel bindings. Request identity is captured by the constructor, before kgpu
// can send the request. Submission updates use numeric atomic slots.
static COMMANDS: Commands<2048> = Commands::new();
pub fn submit(origin: (u64, u64), ctx: u32, kind: u32, cmd_bytes: usize,
              payload_bytes: usize, queue_seq: u64, fence: u64) -> u64 {
    COMMANDS.submit(crate::signal::monotonic_nanos(), origin.0, origin.1, ctx, kind,
                    cmd_bytes as u64, payload_bytes as u64, queue_seq, fence)
}
pub fn doorbell(through: u64) -> u64 {
    let (pid, tid) = crate::task::thread::current_accounting_identity().unwrap_or((0, 0));
    COMMANDS.doorbell(crate::signal::monotonic_nanos(), pid, tid, through)
}
pub fn finish(seq: u64) { COMMANDS.finish(seq, crate::signal::monotonic_nanos()); }
pub fn text() -> String { COMMANDS.text() }

//! Active VirGL request and doorbell boundaries, independent of accounting windows.
use alloc::{format, string::String, vec::Vec};
use core::fmt::Write;
use core::sync::atomic::{AtomicU64, Ordering::{Acquire, Relaxed, AcqRel}};
use super::gpu_command_trace::Slot;

/// Commands and notifications share a bounded sequence space. Queue sequence numbers
/// join a notification to the requests published before it, including batched kicks.
pub struct Commands<const N: usize> {
    slots: [Slot<12>; N],
    next: AtomicU64,
    lost: AtomicU64,
    // Lazily initialized by the parent at CASE START; submit/reap use its numeric slots.
    case_slots: spin::Once<Vec<Slot<12>>>,
    // case ID, parent TID, sequence baseline, start ns, end ns, end sequence, lost baseline.
    window: Slot<7>,
    case_lost: AtomicU64,
    exported: AtomicU64,
}
impl<const N: usize> Commands<N> {
    pub const fn new() -> Self {
        Self { slots: [const { Slot::new() }; N], next: AtomicU64::new(0), lost: AtomicU64::new(0),
            case_slots: spin::Once::new(), window: Slot::new(), case_lost: AtomicU64::new(0), exported: AtomicU64::new(0) }
    }
    /// The existing parent budget stamps bracket CASE START and CASE END. A
    /// watchdog freezes before its pre-kill dump, while the child is still alive.
    pub fn case_boundary(&self, tid: u64, command: &str) {
        let mut words = command.split_whitespace();
        if words.next() != Some("budget") { return; }
        let Some(ns) = words.next().and_then(|s| s.parse::<u64>().ok()).filter(|n| *n != 0) else { return; };
        let phase = words.next().unwrap_or("");
        let label = words.next().unwrap_or("");
        let id = CASES.iter().position(|case| *case == label).map(|i| i as u64 + 1);
        if phase == "parent-fork" {
            let Some(id) = id else { return; };
            // Keep the first bounded window, rather than overwriting its origin
            // during diagnostics. Saturation is exported as overflow, not success.
            self.case_slots.call_once(|| (0..(N * 64).min(32_768)).map(|_| Slot::new()).collect());
            if !self.window.update(|_| Some([id, tid, self.next.load(Acquire), ns, 0, 0, self.case_lost.load(Relaxed)])) {
                self.case_lost.fetch_add(1, Relaxed);
            }
        } else if matches!(phase, "parent-outcome" | "watchdog-before-kill") {
            self.window.update(|mut w| {
                if w[0] == 0 || w[1] != tid || w[4] != 0 { return None; }
                if phase == "parent-outcome" && id != Some(w[0]) { return None; }
                w[4] = ns;
                w[5] = self.next.load(Acquire);
                Some(w)
            });
        }
    }
    fn preserve(&self, row: [u64; 12]) {
        let Some(slots) = self.case_slots.get() else { return; };
        let Some(w) = self.window.read() else { self.case_lost.fetch_add(1, Relaxed); return; };
        if w[0] == 0 || w[4] != 0 || row[0] <= w[2] { return; }
        let index = row[0] - w[2] - 1;
        let Some(slot) = slots.get(index as usize) else { return; };
        if !slot.update(|old| (old[0] < row[0]).then_some(row)) {
            self.case_lost.fetch_add(1, Relaxed);
        }
    }
    fn begin(&self, mut row: [u64; 12]) -> u64 {
        let seq = self.next.fetch_add(1, Relaxed) + 1;
        row[0] = seq;
        if !self.slots[(seq as usize - 1) % N].update(|old| (old[0] < seq).then_some(row)) {
            self.lost.fetch_add(1, Relaxed);
        }
        self.preserve(row);
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
        let Some(slots) = self.case_slots.get() else { return; };
        let Some(w) = self.window.read() else { self.case_lost.fetch_add(1, Relaxed); return; };
        // Freeze membership and completion boundaries. A return after CASE END
        // stays pending in that case, even when the rolling ring later sees it.
        if w[0] == 0 || seq <= w[2] || (w[4] != 0 && ns > w[4]) { return; }
        let Some(slot) = slots.get((seq - w[2] - 1) as usize) else { return; };
        if !slot.update(|mut old| {
            if old[0] != seq { return None; }
            old[2] = ns;
            Some(old)
        }) {
            self.case_lost.fetch_add(1, Relaxed);
        }
    }
    pub fn text(&self) -> String {
        self.text_with_case(true)
    }
    /// Suite exports defer the archive until freeze and emit it once per case.
    /// The rolling ring and the retained snapshot remain available separately.
    pub fn export_text(&self) -> String {
        let archive = self.window.read().is_some_and(|w|
            w[0] != 0 && w[4] != 0 && self.exported.swap(w[3], AcqRel) != w[3]);
        if archive { self.text() } else { self.text_with_case(false) }
    }
    fn text_with_case(&self, archive: bool) -> String {
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
        Self::write_rows(&mut out, rows, "");
        if let (Some(slots), Some(w)) = (self.case_slots.get(), self.window.read()) {
            if archive && w[0] != 0 {
                let end = if w[4] == 0 { count } else { w[5] };
                let length = end.saturating_sub(w[2]);
                let mut rows = Vec::with_capacity(length.min(slots.len() as u64) as usize);
                for slot in slots.iter().take(length.min(slots.len() as u64) as usize) {
                    if let Some(mut row) = slot.read() {
                        if row[0] > w[2] && row[0] <= end {
                            if w[4] != 0 && row[2] > w[4] { row[2] = 0; }
                            rows.push(row);
                        }
                    }
                }
                let case = CASES[w[0] as usize - 1];
                let _ = writeln!(out, "gpu-core-case-window case={case} owner_tid={} begin_ns={} end_ns={} capacity={} retained={} overflow={} lost={} missing={} first_seq={} last_seq={end}",
                    w[1], w[3], w[4], slots.len(), rows.len(), length.saturating_sub(slots.len() as u64),
                    self.case_lost.load(Relaxed).saturating_sub(w[6]), length.min(slots.len() as u64).saturating_sub(rows.len() as u64), w[2] + 1);
                let suffix = format!(" case={case} window_begin_ns={}", w[3]);
                Self::write_rows(&mut out, rows, &suffix);
            }
        }
        out
    }
    fn write_rows(out: &mut String, rows: Vec<[u64; 12]>, suffix: &str) {
        for [seq, begin, end, pid, tid, context, kind, cmd_bytes, payload_bytes, queue_seq, fence, event] in rows {
            if event == 1 {
                let _ = writeln!(out, "gpu-core-command seq={seq} submit_ns={begin} fence_return_ns={end} pid={pid} tid={tid} context={context} type={kind} cmd_bytes={cmd_bytes} payload_bytes={payload_bytes} queue_seq={queue_seq} fence={fence}{suffix}");
            } else {
                let _ = writeln!(out, "gpu-core-doorbell seq={seq} begin_ns={begin} return_ns={end} pid={pid} tid={tid} through={queue_seq}{suffix}");
            }
        }
    }
}

const CASES: &[&str] = &[
    "recovery/clients-notified", "recovery/restart-adopts", "recovery/interaction-after-restart",
    "recovery/stack-restored", "recovery/supervisor-restarts", "stacking/click-raises",
    "stacking/all-minimized", "focus/pair-click", "focus/keys-to-focused",
];

// Kernel bindings. Request identity is captured by the constructor, before kgpu
// can send the request. Submission updates use numeric atomic slots.
static COMMANDS: Commands<2048> = Commands::new();
pub fn case_boundary(tid: u64, command: &str) { COMMANDS.case_boundary(tid, command); }
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
pub fn text() -> String { COMMANDS.export_text() }

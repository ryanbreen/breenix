//! W17: bounded acceptance/completion samples, emitted on console restoration.
//! Collection is independent of detailed-timing switches and allocates no memory.
use alloc::vec::Vec;
type Row = [u64; 15];
// A rate interval has 120 frames plus its final view and restoration. Keep 128
// receipts in static storage. Snapshot copies them to a Vec after the interval,
// keeping the ring off the 16 KiB kernel stack and avoiding per-frame allocation.
const FRAME_CAPACITY: usize = 128;
struct Frames {
    rows: [Row; FRAME_CAPACITY],
    count: u64,
    missed: bool,
    previous_latch: u64,
}
impl Frames {
    const fn new() -> Self {
        Self { rows: [[0; 15]; FRAME_CAPACITY], count: 0, missed: false, previous_latch: 0 }
    }
    fn record(&mut self, row: Row, period_ns: u64) {
        let gap = row[7].saturating_sub(self.previous_latch);
        // A receipt later than the first refresh after acceptance, or a two-edge
        // cadence gap, needs its surrounding frames. Longer gaps can be deliberate
        // idle work; the acceptance-to-latch condition still detects their late flip.
        self.missed |= period_ns != 0 && (row[7].saturating_sub(row[1]) > period_ns
            || (self.count != 0 && gap > period_ns + period_ns / 2
                && gap < period_ns.saturating_mul(2) + period_ns / 2));
        self.rows[self.count as usize % FRAME_CAPACITY] = row;
        self.count += 1;
        self.previous_latch = row[7];
    }
    fn take(&mut self) -> Vec<Row> {
        let mut rows = Vec::new();
        if self.missed {
            for i in self.count.saturating_sub(FRAME_CAPACITY as u64)..self.count {
                rows.push(self.rows[i as usize % FRAME_CAPACITY]);
            }
        }
        self.count = 0;
        self.missed = false;
        self.previous_latch = 0;
        rows
    }
}
static FRAMES: spin::Mutex<Frames> = spin::Mutex::new(Frames::new());
struct Trace {
    count: u64,
    early: u64,
    same_count: u64,
    min_after_ns: u64,
    first: [Row; 4],
    tail: [Row; 4],
}
impl Trace {
    const fn new() -> Self {
        Self { count: 0, early: 0, same_count: 0, min_after_ns: u64::MAX,
            first: [[0; 15]; 4], tail: [[0; 15]; 4] }
    }
    fn record(&mut self, row: Row) {
        if self.count < 4 { self.first[self.count as usize] = row; }
        self.tail[(self.count % 4) as usize] = row;
        self.count += 1;
        self.early += u64::from(row[7] < row[1]);
        self.same_count += u64::from(row[2] >> 16 == row[5] >> 16);
        self.min_after_ns = self.min_after_ns.min(row[7].saturating_sub(row[1]));
    }
}
static TRACE: spin::Mutex<Trace> = spin::Mutex::new(Trace::new());

pub(super) fn record(row: Row, period_ns: u64) {
    TRACE.lock().record(row);
    FRAMES.lock().record(row, period_ns);
}

/// Detached receipt samples and wakeup counters from one restoration boundary.
pub(super) struct Snapshot {
    wakeup: [u64; 3],
    trace: Option<Trace>,
    frames: Vec<Row>,
}

/// Detach while the native session still serializes presentation. Serial output
/// belongs after the session guard is dropped.
pub(super) fn snapshot() -> Snapshot {
    let wakeup = crate::time::wakeup::stats();
    let trace = {
        let mut live = TRACE.lock();
        if live.count == 0 { None }
        else { Some(core::mem::replace(&mut *live, Trace::new())) }
    };
    let frames = FRAMES.lock().take();
    Snapshot { wakeup, trace, frames }
}

#[cfg(not(feature = "gt710_clock_probe"))]
pub(super) fn dump() { export(snapshot()); }

pub(super) fn export(snapshot: Snapshot) {
    let [requests, irqs, late] = snapshot.wakeup;
    crate::serial_println!("TIMER_WAKE requests={} irqs={} late_max_ns={}", requests, irqs, late);
    let Some(trace) = snapshot.trace else { return; };
    crate::serial_println!("GT710_PRESENT_TIMELINE count={} early={} completion_same_count={} min_after_ns={}",
        trace.count, trace.early, trace.same_count, trace.min_after_ns);
    // Accumulate receipt totals and keep the first and most recent samples
    // without putting serial output or formatting on the per-frame path.
    for i in 0..trace.count.min(4) { emit(i + 1, trace.first[i as usize]); }
    for i in trace.count.saturating_sub(4).max(4)..trace.count {
        emit(i + 1, trace.tail[(i % 4) as usize]);
    }
    if !snapshot.frames.is_empty() {
        let retained = snapshot.frames.len() as u64;
        let dropped = trace.count.saturating_sub(retained);
        crate::serial_println!("GT710_PRESENT_MISS count={} retained={} dropped={}", trace.count, retained, dropped);
        for (i, row) in snapshot.frames.into_iter().enumerate() {
            crate::serial_println!("GT710_PRESENT_FRAME seq={} resource={} accept_to_latch_ns={}",
                dropped + i as u64 + 1, row[0], row[7].saturating_sub(row[1]));
            emit(dropped + i as u64 + 1, row);
        }
    }
}
fn emit(sequence: u64, r: Row) {
    crate::serial_println!(concat!("GT710_PRESENT_EDGE seq={} resource={} accept_ns={} accept_vert={} accept_horz={} ",
        "complete_ns={} complete_vert={} complete_horz={} presented_ns={} observed_ns={} observed_vert={} observed_horz={} ",
        "counter_line={} blank_end={} blank_start={} latch_phase={}"),
        sequence, r[0], r[1], r[2], r[3], r[4], r[5], r[6], r[7], r[8], r[9], r[10], r[11], r[12], r[13], r[14]);
}

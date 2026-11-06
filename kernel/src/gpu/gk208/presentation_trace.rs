//! W17: bounded acceptance/completion samples, emitted on console restoration.
//! Collection is independent of detailed-timing switches and allocates no memory.
type Row = [u64; 15];
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

pub(super) fn record(row: Row) { TRACE.lock().record(row); }

pub(super) fn dump() {
    let trace = {
        let mut live = TRACE.lock();
        if live.count == 0 { return; }
        core::mem::replace(&mut *live, Trace::new())
    };
    crate::serial_println!("GT710_PRESENT_TIMELINE count={} early={} completion_same_count={} min_after_ns={}",
        trace.count, trace.early, trace.same_count, trace.min_after_ns);
    // Accumulate receipt totals and keep the first and most recent samples
    // without putting serial output or formatting on the per-frame path.
    for i in 0..trace.count.min(4) { emit(i + 1, trace.first[i as usize]); }
    for i in trace.count.saturating_sub(4).max(4)..trace.count {
        emit(i + 1, trace.tail[(i % 4) as usize]);
    }
}
fn emit(sequence: u64, r: Row) {
    crate::serial_println!(concat!("GT710_PRESENT_EDGE seq={} resource={} accept_ns={} accept_vert={} accept_horz={} ",
        "complete_ns={} complete_vert={} complete_horz={} presented_ns={} observed_ns={} observed_vert={} observed_horz={} ",
        "counter_line={} blank_end={} blank_start={} latch_phase={}"),
        sequence, r[0], r[1], r[2], r[3], r[4], r[5], r[6], r[7], r[8], r[9], r[10], r[11], r[12], r[13], r[14]);
}

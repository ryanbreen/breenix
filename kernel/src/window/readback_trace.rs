//! Always-on, bounded observation evidence; formatting happens only on proc reads.
use alloc::{string::String, vec::Vec};
use core::fmt::Write;

const CAPACITY: usize = 2048;
#[derive(Clone, Copy)]
pub struct Record {
    pub branch: &'static str,
    // begin, end, observer group/thread, claimant/epoch, baseline/current serial,
    // busy, last completed serial/flags, acknowledged presentation.
    pub fields: [u64; 12],
}
const EMPTY: Record = Record { branch: "empty", fields: [0; 12] };
struct Ring { rows: [Record; CAPACITY], count: u64 }
static RING: spin::Mutex<Ring> = spin::Mutex::new(Ring { rows: [EMPTY; CAPACITY], count: 0 });
pub fn record(branch: &'static str, fields: [u64; 12]) {
    let mut ring = RING.lock();
    let slot = ring.count as usize % CAPACITY;
    ring.rows[slot] = Record { branch, fields };
    ring.count += 1;
}
pub fn text() -> String {
    let mut rows = Vec::with_capacity(CAPACITY);
    let count;
    {
        let ring = RING.lock();
        count = ring.count;
        let len = count.min(CAPACITY as u64) as usize;
        for i in 0..len {
            rows.push(ring.rows[(count as usize - len + i) % CAPACITY]);
        }
    }
    let mut out = String::new();
    let _ = writeln!(out, "retained={} overwritten={}", rows.len(), count.saturating_sub(CAPACITY as u64));
    for (i, row) in rows.iter().enumerate() {
        let seq = count - rows.len() as u64 + i as u64;
        let [begin, end, observer, tid, claimant, epoch, first, serial, busy, completed, flags, ack] = row.fields;
        if row.branch == "retained-writer-fallback" {
            let _ = writeln!(out, "seq={seq} begin_ns={begin} end_ns={end} branch={} observer={observer} target={tid}", row.branch);
            let _ = writeln!(out, "seq={seq} begin_ns={begin} writer_ctx={claimant} writer_serial={epoch} fence={first} answer={serial} errno={ack}");
            continue;
        }
        let _ = writeln!(out, "seq={seq} begin_ns={begin} end_ns={end} branch={} observer={observer} tid={tid}", row.branch);
        let _ = writeln!(out, "seq={seq} begin_ns={begin} claimant={claimant} epoch={epoch} first={first} serial={serial} busy={busy}");
        let _ = writeln!(out, "seq={seq} begin_ns={begin} completed={completed} flags={flags} ack={ack}");
    }
    out
}

//! Bounded, non-waiting per-CPU admission for serial records.
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

const BYTES: usize = 32768;
const RECORDS: usize = 2048;
const CPUS: usize = 8;
const ACTIVE: usize = 1;
const PRODUCER: usize = 2;
const GENERATION: usize = 1 << 16;
const PRODUCERS: usize = GENERATION - PRODUCER;

#[derive(Clone, Copy)]
pub struct Meta<W> {
    pub writer: W,
    pub tee: bool,
    pub fresh: bool,
    len: usize,
    start: usize,
}

pub enum Admission {
    Start,
    Queued,
    Busy,
}
pub enum Next<W> {
    Record(Meta<W>, usize),
    Overflow(u64),
    Empty,
}

pub fn overflow_line(mut lost: u64, out: &mut [u8; 96]) -> &[u8] {
    let prefix = b"[serial] deferred record overflow: ";
    out[..prefix.len()].copy_from_slice(prefix);
    let mut digits = [0u8; 20];
    let mut first = digits.len();
    loop {
        first -= 1;
        digits[first] = b'0' + (lost % 10) as u8;
        lost /= 10;
        if lost == 0 {
            break;
        }
    }
    let end = prefix.len() + digits.len() - first;
    out[prefix.len()..end].copy_from_slice(&digits[first..]);
    out[end] = b'\n';
    &out[..end + 1]
}

/// One local producer (interrupts masked) and the unit's single drainer.
/// Publication and reclamation use separate release/acquire cursors. Neither
/// participant holds exclusion across the other's copy or across UART access.
struct Queue<W: Copy> {
    producing: AtomicBool,
    bytes: [UnsafeCell<u8>; BYTES],
    records: [UnsafeCell<Option<Meta<W>>>; RECORDS],
    write: AtomicUsize,
    read: AtomicUsize,
    byte_write: AtomicUsize,
    byte_read: AtomicUsize,
}

impl<W: Copy> Queue<W> {
    const fn new() -> Self {
        Self {
            producing: AtomicBool::new(false),
            bytes: [const { UnsafeCell::new(0) }; BYTES],
            records: [const { UnsafeCell::new(None) }; RECORDS],
            write: AtomicUsize::new(0),
            read: AtomicUsize::new(0),
            byte_write: AtomicUsize::new(0),
            byte_read: AtomicUsize::new(0),
        }
    }

    fn push(&self, writer: W, bytes: &[u8], tee: bool, fresh: bool) -> bool {
        // Local IRQ masking prevents this in kernel callers. Keep the safe API
        // memory-safe even if a caller supplies an already occupied CPU index.
        if self.producing.swap(true, Ordering::Acquire) { return false; }
        let write = self.write.load(Ordering::Relaxed);
        let start = self.byte_write.load(Ordering::Relaxed);
        let used = start.wrapping_sub(self.byte_read.load(Ordering::Acquire));
        let fits = write.wrapping_sub(self.read.load(Ordering::Acquire)) < RECORDS
            && bytes.len() <= BYTES - used;
        if fits {
            for (offset, &byte) in bytes.iter().enumerate() {
                // SAFETY: producer exclusion and byte_read's acquire load
                // reserve these cells until the release publication below.
                unsafe { *self.bytes[start.wrapping_add(offset) % BYTES].get() = byte; }
            }
            // SAFETY: read has released this descriptor; the consumer cannot
            // inspect it before write is published with release ordering.
            unsafe { *self.records[write % RECORDS].get() = Some(Meta {
                writer, tee, fresh, len: bytes.len(), start,
            }); }
            self.byte_write.store(start.wrapping_add(bytes.len()), Ordering::Relaxed);
            self.write.store(write.wrapping_add(1), Ordering::Release);
        }
        self.producing.store(false, Ordering::Release);
        fits
    }

    /// Called only under the drainer's exclusion. Keep partial records in the
    /// same lane until complete, so another CPU's line cannot split them.
    fn pop(&self, out: &mut [u8]) -> Option<(Meta<W>, usize, bool)> {
        let read = self.read.load(Ordering::Relaxed);
        if read == self.write.load(Ordering::Acquire) { return None; }
        // SAFETY: write published the initialized descriptor and payload; the
        // producer cannot reuse this descriptor until read advances.
        let mut meta = unsafe { (*self.records[read % RECORDS].get())? };
        let sent = meta;
        let n = meta.len.min(out.len());
        for (offset, slot) in out.iter_mut().take(n).enumerate() {
            // SAFETY: byte_read has not released these payload cells yet.
            *slot = unsafe { *self.bytes[meta.start.wrapping_add(offset) % BYTES].get() };
        }
        meta.start = meta.start.wrapping_add(n);
        meta.len -= n;
        meta.fresh = false;
        self.byte_read.store(meta.start, Ordering::Release);
        let complete = meta.len == 0;
        if complete {
            self.read.store(read.wrapping_add(1), Ordering::Release);
        } else {
            // SAFETY: the descriptor belongs to the drainer until read advances.
            unsafe { *self.records[read % RECORDS].get() = Some(meta); }
        }
        Some((sent, n, complete))
    }
}

pub struct Deferred<W: Copy> {
    // Active bit, in-flight producer count and admission generation. The
    // generation prevents an enqueue between the empty scan and its CAS from
    // being stranded by an apparent empty -> producer -> empty ABA cycle.
    state: AtomicUsize,
    draining: AtomicBool,
    cursor: AtomicUsize,
    lost: AtomicU64,
    queues: [Queue<W>; CPUS],
}

// Each queue serializes its producer; draining protects the consumer cursor. The
// release/acquire cursors transfer exclusive access to descriptor/payload cells.
unsafe impl<W: Copy + Send> Sync for Deferred<W> {}

impl<W: Copy> Deferred<W> {
    pub const fn new() -> Self {
        Self {
            state: AtomicUsize::new(0),
            draining: AtomicBool::new(false),
            cursor: AtomicUsize::new(0),
            lost: AtomicU64::new(0),
            queues: [const { Queue::new() }; CPUS],
        }
    }

    pub fn active(&self) -> bool {
        self.state.load(Ordering::Acquire) & ACTIVE != 0
    }

    /// claim-lint:ok: #2424 W25 requires non-waiting kernel admission; concurrent_cpu_admission_keeps_every_line_when_storage_has_room exercises four producers.
    /// Called with local interrupts masked. Kernel producers never wait for a
    /// producer or the drainer. Userspace applies backpressure between units.
    pub fn submit(
        &self, cpu: usize, writer: W, bytes: &[u8], tee: bool, fresh: bool, may_defer: bool,
    ) -> Admission {
        if !may_defer && self.active() { return Admission::Busy; }
        self.state.fetch_add(GENERATION + PRODUCER, Ordering::AcqRel);
        let start = self.state.fetch_or(ACTIVE, Ordering::AcqRel) & ACTIVE == 0;
        let result = if start {
            Admission::Start
        } else if !may_defer {
            Admission::Busy
        } else {
            if !self.queues.get(cpu).is_some_and(|queue| queue.push(writer, bytes, tee, fresh)) {
                self.lost.fetch_add(1, Ordering::Relaxed);
            }
            Admission::Queued
        };
        self.state.fetch_sub(PRODUCER, Ordering::Release);
        result
    }

    /// The transmitter retries with interrupts restored while an admission is
    /// publishing. Empty closes admission only if the complete scan is current.
    pub fn next(&self, out: &mut [u8]) -> Option<Next<W>> {
        if self.draining.swap(true, Ordering::Acquire) { return None; }
        let observed = self.state.load(Ordering::Acquire);
        let first = self.cursor.load(Ordering::Relaxed);
        for offset in 0..CPUS {
            let cpu = (first + offset) % CPUS;
            if let Some((meta, n, complete)) = self.queues[cpu].pop(out) {
                self.cursor.store(if complete { (cpu + 1) % CPUS } else { cpu }, Ordering::Relaxed);
                self.draining.store(false, Ordering::Release);
                return Some(Next::Record(meta, n));
            }
        }
        let lost = self.lost.swap(0, Ordering::AcqRel);
        let result = if lost != 0 {
            Some(Next::Overflow(lost))
        } else if observed & PRODUCERS == 0 && self.state.compare_exchange(
            observed, observed & !ACTIVE, Ordering::AcqRel, Ordering::Relaxed,
        ).is_ok() {
            Some(Next::Empty)
        } else {
            None
        };
        self.draining.store(false, Ordering::Release);
        result
    }
}

impl<W: Copy> Default for Deferred<W> {
    fn default() -> Self { Self::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concurrent_cpu_admission_keeps_every_line_when_storage_has_room() {
        static RING: Deferred<u8> = Deferred::new();
        assert!(matches!(RING.submit(0, 0, b"unit\n", false, false, false), Admission::Start));
        std::thread::scope(|scope| {
            for cpu in 0..4 {
                scope.spawn(move || {
                    for sequence in 0..512 {
                        let line = std::format!("cpu={cpu} sequence={sequence}\n");
                        assert!(matches!(RING.submit(cpu, cpu as u8, line.as_bytes(), true, true, true), Admission::Queued));
                    }
                });
            }
        });
        let mut seen = std::collections::BTreeSet::new();
        let mut out = [0; 256];
        loop {
            match RING.next(&mut out) {
                Some(Next::Record(_, len)) => {
                    assert!(seen.insert(std::str::from_utf8(&out[..len]).unwrap().to_owned()));
                }
                Some(Next::Overflow(lost)) => panic!("lost {lost} lines with room available"),
                Some(Next::Empty) => break,
                None => std::thread::yield_now(),
            }
        }
        for cpu in 0..4 {
            for sequence in 0..512 {
                assert!(seen.remove(&std::format!("cpu={cpu} sequence={sequence}\n")));
            }
        }
        assert!(seen.is_empty());
    }
}

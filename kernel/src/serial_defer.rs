//! Bounded, non-waiting admission for serial records. No UART access under the ring lock.
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

const BYTES: usize = 32768;
const RECORDS: usize = 64;

#[derive(Clone, Copy)]
pub struct Meta<W> {
    pub writer: W,
    pub tee: bool,
    pub fresh: bool,
    len: usize,
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

struct Queue<W: Copy> {
    bytes: [u8; BYTES],
    head: usize,
    len: usize,
    records: [Option<Meta<W>>; RECORDS],
    first: usize,
    count: usize,
}

/// Callers mask interrupts only around these bounded copies. Contending producers
/// count overflow and return immediately; the record owner drains after its unit.
pub struct Deferred<W: Copy> {
    active: AtomicBool,
    locked: AtomicBool,
    lost: AtomicU64,
    queue: UnsafeCell<Queue<W>>,
}

// The queue accessors acquire locked with an acquire CAS before accessing queue.
unsafe impl<W: Copy + Send> Sync for Deferred<W> {}

impl<W: Copy> Deferred<W> {
    pub const fn new() -> Self {
        Self {
            active: AtomicBool::new(false),
            locked: AtomicBool::new(false),
            lost: AtomicU64::new(0),
            queue: UnsafeCell::new(Queue {
                bytes: [0; BYTES],
                head: 0,
                len: 0,
                records: [None; RECORDS],
                first: 0,
                count: 0,
            }),
        }
    }

    pub fn active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    /// True admits the caller as transmitter. False has queued the whole record,
    /// or counted its loss. Neither result waits for another producer.
    pub fn submit(
        &self,
        writer: W,
        bytes: &[u8],
        tee: bool,
        fresh: bool,
        may_defer: bool,
    ) -> Admission
    where
        W: PartialEq,
    {
        if self
            .locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            if may_defer {
                self.lost.fetch_add(1, Ordering::Relaxed);
                return Admission::Queued;
            }
            return Admission::Busy;
        }
        if self.active() && !may_defer {
            self.locked.store(false, Ordering::Release);
            return Admission::Busy;
        }
        let start = !self.active.swap(true, Ordering::AcqRel);
        if !start {
            // SAFETY: the successful acquire CAS above owns locked until the release store below.
            let q = unsafe { &mut *self.queue.get() };
            let last = (q.first + q.count.saturating_sub(1)) % RECORDS;
            let join = !tee
                && !fresh
                && q.count != 0
                && q.len != 0
                && bytes.len() <= BYTES - q.len
                && q.bytes[(q.head + q.len - 1) % BYTES] != b'\n'
                && q.records[last].is_some_and(|meta| {
                    !meta.tee
                        && !meta.fresh
                        && meta.writer == writer
                        && meta.len + bytes.len() <= 8256
                });
            if join {
                for &byte in bytes {
                    q.bytes[(q.head + q.len) % BYTES] = byte;
                    q.len += 1;
                }
                if let Some(meta) = q.records[last].as_mut() {
                    meta.len += bytes.len();
                }
            } else if q.count == RECORDS || bytes.len() > BYTES - q.len || bytes.len() > 8256 {
                self.lost.fetch_add(1, Ordering::Relaxed);
            } else {
                for &byte in bytes {
                    q.bytes[(q.head + q.len) % BYTES] = byte;
                    q.len += 1;
                }
                q.records[(q.first + q.count) % RECORDS] = Some(Meta {
                    writer,
                    tee,
                    fresh,
                    len: bytes.len(),
                });
                q.count += 1;
            }
        }
        self.locked.store(false, Ordering::Release);
        if start {
            Admission::Start
        } else {
            Admission::Queued
        }
    }

    /// The transmitter tries again on contention, with interrupts restored.
    /// Empty clears active under the same exclusion used for admission.
    pub fn next(&self, out: &mut [u8]) -> Option<Next<W>> {
        if self
            .locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            return None;
        }
        // SAFETY: the successful acquire CAS above owns locked until the release store below.
        let q = unsafe { &mut *self.queue.get() };
        let result = if let Some(mut meta) = q.records[q.first].take() {
            let n = meta.len.min(out.len());
            let sent = meta;
            for slot in out.iter_mut().take(n) {
                *slot = q.bytes[q.head];
                q.head = (q.head + 1) % BYTES;
                q.len -= 1;
            }
            if n == meta.len {
                q.first = (q.first + 1) % RECORDS;
                q.count -= 1;
            } else {
                meta.len -= n;
                meta.fresh = false;
                q.records[q.first] = Some(meta);
            }
            Next::Record(sent, n)
        } else {
            let lost = self.lost.swap(0, Ordering::AcqRel);
            if lost != 0 {
                Next::Overflow(lost)
            } else {
                self.active.store(false, Ordering::Release);
                Next::Empty
            }
        };
        self.locked.store(false, Ordering::Release);
        Some(result)
    }
}

impl<W: Copy> Default for Deferred<W> {
    fn default() -> Self {
        Self::new()
    }
}

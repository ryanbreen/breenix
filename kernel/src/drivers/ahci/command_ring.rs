//! Failure-only AHCI command history. Recording writes only to the history.
//!
//! A producer reserves one sequence and writes tagged atomic fields without
//! waiting for another producer. A reader accepts a record only when its fields
//! carry that sequence. An interrupted producer, or one lapped by another CPU,
//! leaves an explicit incomplete record rather than mixed command identities.

use core::fmt::{self, Write};
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

const CAPACITY: usize = 32;
const FIELDS: usize = 10;

#[derive(Clone, Copy)]
#[repr(u8)]
pub(super) enum EventKind {
    Issue = 1,
    IsrEntry,
    ReceiptDeferred,
    ReceiptRecorded,
    RetireNormal,
    RetireDrain,
    RetireSingle,
    WaitPark,
    WaitWake,
    WaitRecheck,
    Timeout,
    Issued,
}

impl EventKind {
    fn name(value: u8) -> &'static str {
        match value {
            1 => "issue",
            2 => "isr-entry",
            3 => "receipt-deferred",
            4 => "receipt-recorded",
            5 => "retire-normal",
            6 => "retire-drain",
            7 => "retire-single-active",
            8 => "wait-park",
            9 => "wait-wake",
            10 => "wait-recheck",
            11 => "timeout",
            12 => "issued",
            _ => "unknown",
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct Event {
    pub kind: EventKind,
    pub slot: u8,
    pub token: u32,
    pub ci: u32,
    pub port_is: u32,
    pub tfd: u32,
    pub hba_is: u32,
    pub active: u32,
    pub done: u32,
    pub waiter: u64,
}

pub(super) struct CommandRing {
    next: AtomicU64,
    dumped: AtomicBool,
    fields: [[AtomicU64; FIELDS]; CAPACITY],
}

impl CommandRing {
    pub const fn new() -> Self {
        Self {
            next: AtomicU64::new(0),
            dumped: AtomicBool::new(false),
            fields: [const { [const { AtomicU64::new(0) }; FIELDS] }; CAPACITY],
        }
    }

    /// Fixed work: one sequence reservation and ten tagged field stores.
    /// The tag is the sequence's low word; the full sequence orders the dump.
    /// No ownership bit, contention loop, or shared completion state is touched.
    #[inline]
    pub fn record(&self, event: Event) {
        let ticket = self.next.fetch_add(1, Ordering::Relaxed);
        let tag = ((ticket.wrapping_add(1) as u32) as u64) << 32;
        let row = &self.fields[ticket as usize & (CAPACITY - 1)];
        row[0].store(tag | event.kind as u64 | ((event.slot as u64) << 8), Ordering::Relaxed);
        row[1].store(tag | event.token as u64, Ordering::Relaxed);
        row[2].store(tag | event.ci as u64, Ordering::Relaxed);
        row[3].store(tag | event.port_is as u64, Ordering::Relaxed);
        row[4].store(tag | event.tfd as u64, Ordering::Relaxed);
        row[5].store(tag | event.hba_is as u64, Ordering::Relaxed);
        row[6].store(tag | event.active as u64, Ordering::Relaxed);
        row[7].store(tag | event.done as u64, Ordering::Relaxed);
        row[8].store(tag | event.waiter as u32 as u64, Ordering::Relaxed);
        row[9].store(tag | (event.waiter >> 32), Ordering::Relaxed);
    }

    /// Called only at timeout. A port emits at most one history per boot.
    /// Dumping reports missing tags without waiting for an interrupted writer.
    pub fn dump_once(&self, port: usize, sink: &mut impl Write) -> fmt::Result {
        if self.dumped.swap(true, Ordering::Relaxed) {
            return Ok(());
        }
        let end = self.next.load(Ordering::Relaxed);
        writeln!(sink, "[ahci-ring] port={} newest={} capacity={}", port, end, CAPACITY)?;
        for ticket in end.saturating_sub(CAPACITY as u64)..end {
            let seq = ticket.wrapping_add(1);
            let row = &self.fields[ticket as usize & (CAPACITY - 1)];
            let words = core::array::from_fn::<_, FIELDS, _>(|i| row[i].load(Ordering::Relaxed));
            if words.iter().any(|word| (word >> 32) as u32 != seq as u32) {
                writeln!(sink, "[ahci-ring] seq={} incomplete", seq)?;
                continue;
            }
            let values = words.map(|word| word as u32);
            let waiter = values[8] as u64 | ((values[9] as u64) << 32);
            writeln!(sink,
                "[ahci-ring] seq={} event={} slot={} token={} CI={:#x} IS={:#x} TFD={:#x} HBA_IS={:#x} active={:#x} done={} waiter={:#x}",
                seq, EventKind::name(values[0] as u8), values[0] >> 8,
                values[1], values[2], values[3], values[4], values[5], values[6], values[7], waiter)?;
        }
        Ok(())
    }
}

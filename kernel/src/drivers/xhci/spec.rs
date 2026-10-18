//! Clean-room encodings from Intel xHCI 1.2, chapters 5-7, and USB 2.0 ch. 9.
//! No implementation from another USB stack was consulted.
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

pub const TRBS: usize = 256;
pub const EVENT_TRBS: usize = 16;

#[repr(C, align(16))]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Trb {
    pub parameter: u64,
    pub status: u32,
    pub control: u32,
}
impl Trb {
    pub const fn new(kind: u32, parameter: u64, status: u32, flags: u32) -> Self {
        Self {
            parameter,
            status,
            control: (kind << 10) | flags,
        }
    }
    pub fn kind(self) -> u32 {
        (self.control >> 10) & 0x3f
    }
    pub fn code(self) -> u8 {
        (self.status >> 24) as u8
    }
    pub fn slot(self) -> u8 {
        (self.control >> 24) as u8
    }
}

/// Software producer, with a Link TRB at the end (4.9.2, 6.4.4.1).
/// The caller serializes submissions and retires each TD before reusing its space.
pub struct Producer {
    pub index: usize,
    pub cycle: bool,
}
impl Producer {
    pub const fn new() -> Self {
        Self {
            index: 0,
            cycle: true,
        }
    }
    pub fn advance(&mut self) -> bool {
        self.index += 1;
        if self.index == TRBS - 1 {
            self.index = 0;
            self.cycle = !self.cycle;
            true
        } else {
            false
        }
    }
}

pub struct Consumer {
    pub index: usize,
    pub cycle: bool,
    pub wraps: u64,
}
impl Consumer {
    pub const fn new() -> Self {
        Self {
            index: 0,
            cycle: true,
            wraps: 0,
        }
    }
    pub fn advance(&mut self) {
        self.index += 1;
        if self.index == EVENT_TRBS {
            self.index = 0;
            self.cycle = !self.cycle;
            self.wraps += 1;
        }
    }
}

/// Context entries are 32 or 64 bytes; the input control context precedes slot 0.
pub struct Contexts {
    pub stride: usize,
}
impl Contexts {
    pub fn new(csz: bool) -> Self {
        Self {
            stride: if csz { 64 } else { 32 },
        }
    }
    pub fn input(&self, dci: usize) -> usize {
        (dci + 1) * self.stride
    }
    pub fn output(&self, dci: usize) -> usize {
        dci * self.stride
    }
}

// PORTSC RW1C flags and PED (RW1CS) must never be echoed by a read/modify/write.
// Only PP, PIC and wake-enable bits are preserved (5.4.8.1).
pub const PORT_CHANGES: u32 = 0x00fe_0000;
pub fn port_write(old: u32, action: u32) -> u32 {
    (old & 0x0e00_c200) | action
}

/// Device-to-host GET_DESCRIPTOR, USB 2.0 table 9-5 and xHCI 6.4.1.2.
pub fn get_descriptor(kind: u8, index: u8, length: u16) -> Trb {
    let setup =
        0x0680u64 | ((index as u64) << 16) | ((kind as u64) << 24) | ((length as u64) << 48);
    Trb::new(2, setup, 8, (1 << 6) | (3 << 16))
}

pub fn packet_size(speed: u64, encoded: u8) -> Result<u16, &'static str> {
    match speed {
        1_500_000 if encoded == 8 => Ok(8),
        12_000_000 if matches!(encoded, 8 | 16 | 32 | 64) => Ok(encoded as u16),
        480_000_000 if encoded == 64 => Ok(64),
        n if n >= 5_000_000_000 && encoded == 9 => Ok(512),
        _ => Err("invalid bMaxPacketSize0 for negotiated speed"),
    }
}

/// Pending work and waiter publication use a single sequentially-consistent
/// order. The waiter publishes before checking pending, then publishes
/// BlockedOnIO under the scheduler lock and checks pending again. Thus an IRQ
/// either sees its waiter, or the waiter sees the pending bit before sleeping.
/// Only the worker consumes pending; ISR bursts coalesce without losing work.
pub struct Work {
    pending: AtomicBool,
    waiter: AtomicU64,
}
impl Work {
    pub const fn new() -> Self {
        Self {
            pending: AtomicBool::new(false),
            waiter: AtomicU64::new(0),
        }
    }
    pub fn signal(&self) -> u64 {
        self.pending.store(true, Ordering::SeqCst);
        self.waiter.load(Ordering::SeqCst)
    }
    pub fn publish(&self, tid: u64) {
        self.waiter.store(tid, Ordering::SeqCst);
    }
    pub fn withdraw(&self) {
        self.waiter.store(0, Ordering::SeqCst);
    }
    pub fn pending(&self) -> bool {
        self.pending.load(Ordering::SeqCst)
    }
    pub fn take(&self) -> bool {
        self.pending.swap(false, Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn producer_and_event_cycles_cross_two_wraps() {
        let mut p = Producer::new();
        for lap in 0..3 {
            for i in 0..TRBS - 1 {
                assert_eq!(p.index, i);
                assert_eq!(p.cycle, lap % 2 == 0);
                assert_eq!(p.advance(), i == TRBS - 2);
            }
        }
        let mut c = Consumer::new();
        for i in 0..EVENT_TRBS * 3 {
            assert_eq!(c.cycle, i / EVENT_TRBS % 2 == 0);
            c.advance();
        }
        assert_eq!(c.wraps, 3);
    }
    #[test]
    fn contexts_and_setup_fields_match_wire_offsets() {
        for csz in [false, true] {
            let c = Contexts::new(csz);
            assert_eq!(c.input(0), if csz { 64 } else { 32 });
            assert_eq!(c.input(1), if csz { 128 } else { 64 });
            assert_eq!(c.output(1), if csz { 64 } else { 32 });
            assert_eq!(c.input(31) + c.stride, if csz { 2112 } else { 1056 });
        }
        assert_eq!(
            get_descriptor(1, 0, 8),
            Trb {
                parameter: 0x0008_0000_0100_0680,
                status: 8,
                control: 0x0003_0840,
            }
        );
        let event = Trb {
            parameter: 0x1230,
            status: 1 << 24,
            control: (33 << 10) | (7 << 24) | 1,
        };
        assert_eq!((event.kind(), event.code(), event.slot()), (33, 1, 7));
        assert_eq!(packet_size(12_000_000, 32), Ok(32));
        assert_eq!(packet_size(5_000_000_000, 9), Ok(512));
        assert!(packet_size(12_000_000, 9).is_err());
        assert_eq!(port_write(u32::MAX, 1 << 4), 0x0e00_c210);
        assert_eq!(port_write(u32::MAX, 0) & (PORT_CHANGES | 2), 0);
    }
    #[test]
    fn work_arriving_at_each_wait_boundary_is_retained() {
        // IRQ before publication, between publication/check, between check/block,
        // and after block: each must leave pending or a wake of the published TID.
        for boundary in 0..4 {
            let w = Work::new();
            let mut wake = 0;
            if boundary == 0 {
                wake = w.signal();
            }
            w.publish(17);
            if boundary == 1 {
                wake = w.signal();
            }
            let early = w.pending();
            if boundary == 2 {
                wake = w.signal();
            }
            let after_block = w.pending();
            if boundary == 3 {
                wake = w.signal();
            }
            assert!(early || after_block || wake == 17);
            assert!(w.take());
            assert!(!w.take());
            w.withdraw();
            assert_eq!(w.signal(), 0);
            assert!(w.take());
        }
    }
}

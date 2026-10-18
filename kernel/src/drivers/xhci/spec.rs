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

// Exclude PORTSC RW1C flags and PED (RW1CS) from read/modify/write values.
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

/// PP cleared, keeping only the PIC and wake-enable bits (5.4.8, 4.19.1.1.1).
pub fn port_power_off(old: u32) -> u32 {
    old & 0x0e00_c000
}

/// A standard device request as a Setup Stage parameter (USB 2.0 9.3, 9.4).
const fn setup(request_type: u8, request: u8, value: u16, index: u16, length: u16) -> u64 {
    request_type as u64
        | (request as u64) << 8
        | (value as u64) << 16
        | (index as u64) << 32
        | (length as u64) << 48
}
/// SET_CONFIGURATION (USB 2.0 9.4.7).
pub const fn set_configuration(value: u8) -> u64 {
    setup(0, 9, value as u16, 0, 0)
}
/// CLEAR_FEATURE(ENDPOINT_HALT) to an endpoint (USB 2.0 9.4.1, table 9-6).
pub const fn clear_endpoint_halt(endpoint: u8) -> u64 {
    setup(2, 1, 0, endpoint as u16, 0)
}

// Transfer TRBs (6.4.1). IDT bit 6, TRT bits 17:16, DIR bit 16, ISP bit 2, IOC bit 5.
/// Setup Stage with no Data Stage (TRT 0).
pub fn setup_no_data(setup: u64) -> Trb {
    Trb::new(2, setup, 8, 1 << 6)
}
/// Setup Stage followed by an OUT Data Stage (TRT 2).
pub fn setup_data_out(setup: u64) -> Trb {
    Trb::new(2, setup, 8, (1 << 6) | (2 << 16))
}
/// OUT Data Stage of `length` bytes.
pub fn data_out(buffer: u64, length: u32) -> Trb {
    Trb::new(3, buffer, length, 0)
}
/// IN Status Stage, for requests with no data or OUT data, interrupting on completion.
pub fn status_in() -> Trb {
    Trb::new(4, 0, 0, (1 << 16) | (1 << 5))
}
/// Normal TRB for an interrupt-IN TD: interrupt on a short packet or on completion.
pub fn normal_in(buffer: u64, length: u32) -> Trb {
    Trb::new(1, buffer, length, (1 << 2) | (1 << 5))
}

// Command TRBs (6.4.3): Slot ID bits 31:24, Endpoint ID bits 20:16.
pub fn disable_slot(slot: u8) -> Trb {
    Trb::new(10, 0, 0, (slot as u32) << 24)
}
pub fn configure_endpoint(input: u64, slot: u8) -> Trb {
    Trb::new(12, input, 0, (slot as u32) << 24)
}
/// Reset Endpoint with TSP 0: the data toggle is reset (4.6.8).
pub fn reset_endpoint(slot: u8, dci: u8) -> Trb {
    Trb::new(14, 0, 0, (slot as u32) << 24 | (dci as u32) << 16)
}
pub fn stop_endpoint(slot: u8, dci: u8) -> Trb {
    Trb::new(15, 0, 0, (slot as u32) << 24 | (dci as u32) << 16)
}
/// Set TR Dequeue Pointer with no streams: SCT 0, DCS in bit 0 (6.4.3.9).
pub fn set_dequeue(slot: u8, dci: u8, pointer: u64, cycle: bool) -> Trb {
    Trb::new(
        16,
        (pointer & !15) | cycle as u64,
        0,
        (slot as u32) << 24 | (dci as u32) << 16,
    )
}

/// The Interval field for an interrupt endpoint (6.2.3.6, table 6-12): LS/FS
/// bInterval counts 1 ms frames and is rounded down to a power of two of
/// 125 us units (3-10); HS and SS bInterval is an exponent, Interval = bInterval-1.
pub fn interrupt_interval(speed: u64, b_interval: u8) -> Result<u8, &'static str> {
    match speed {
        1_500_000 | 12_000_000 if b_interval != 0 => {
            let units = b_interval as u32 * 8;
            Ok((31 - units.leading_zeros()).clamp(3, 10) as u8)
        }
        s if s >= 480_000_000 && (1..=16).contains(&b_interval) => Ok(b_interval - 1),
        _ => Err("bInterval out of range for the endpoint's speed"),
    }
}

/// The five defined dwords of an Interrupt IN Endpoint Context (6.2.3,
/// 4.8.2.4): Interval, CErr 3, EP Type 7, Max Burst, Max Packet Size, the TR
/// Dequeue Pointer with DCS 1, the Average TRB Length (one report) and the
/// Max ESIT Payload, Max Packet Size * (Max Burst + 1) for USB2 (4.14.2).
pub fn interrupt_in_context(interval: u8, max_packet: u16, burst: u8, ring: u64) -> [u32; 5] {
    let esit = max_packet as u32 * (burst as u32 + 1);
    [
        (interval as u32) << 16 | (esit >> 16) << 24,
        (3 << 1) | (7 << 3) | (burst as u32) << 8 | (max_packet as u32) << 16,
        (ring as u32 & !15) | 1,
        (ring >> 32) as u32,
        max_packet as u32 | (esit & 0xffff) << 16,
    ]
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
    pub fn waiter(&self) -> u64 {
        self.waiter.load(Ordering::SeqCst)
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
    fn class_driver_trbs_and_contexts_match_wire_fields() {
        assert_eq!(interrupt_interval(1_500_000, 10), Ok(6)); // 80 units -> 64 (8 ms)
        assert_eq!(interrupt_interval(12_000_000, 1), Ok(3));
        assert_eq!(interrupt_interval(12_000_000, 255), Ok(10));
        assert_eq!(interrupt_interval(12_000_000, 32), Ok(8));
        assert_eq!(interrupt_interval(480_000_000, 4), Ok(3));
        assert!(interrupt_interval(480_000_000, 17).is_err());
        assert!(interrupt_interval(1_500_000, 0).is_err());
        assert_eq!(
            interrupt_in_context(6, 8, 0, 0x1_2345_6000),
            [0x0006_0000, 0x0008_003e, 0x2345_6001, 1, 0x0008_0008]
        );
        // A high-bandwidth endpoint: Max Burst and the ESIT payload follow it.
        assert_eq!(interrupt_in_context(3, 1024, 2, 0)[1] >> 8 & 0xff, 2);
        assert_eq!(interrupt_in_context(3, 1024, 2, 0)[4] >> 16, 3072);
        assert_eq!(set_configuration(1).to_le_bytes(), [0, 9, 1, 0, 0, 0, 0, 0]);
        assert_eq!(clear_endpoint_halt(0x81).to_le_bytes(), [2, 1, 0, 0, 0x81, 0, 0, 0]);
        assert_eq!(setup_no_data(7).control, (2 << 10) | (1 << 6));
        assert_eq!(setup_data_out(7).control, (2 << 10) | (1 << 6) | (2 << 16));
        assert_eq!((data_out(0x2000, 1).status, data_out(0x2000, 1).control), (1, 3 << 10));
        assert_eq!(status_in().control, (4 << 10) | (1 << 16) | (1 << 5));
        assert_eq!(normal_in(0x3000, 8), Trb { parameter: 0x3000, status: 8, control: (1 << 10) | 0x24 });
        assert_eq!(disable_slot(5).control, (10 << 10) | (5 << 24));
        assert_eq!(configure_endpoint(0x4000, 5), Trb { parameter: 0x4000, status: 0, control: (12 << 10) | (5 << 24) });
        assert_eq!(reset_endpoint(5, 3).control, (14 << 10) | (3 << 16) | (5 << 24));
        assert_eq!(stop_endpoint(5, 3).control, (15 << 10) | (3 << 16) | (5 << 24));
        assert_eq!(set_dequeue(5, 1, 0x5010, true).parameter, 0x5011);
        assert_eq!(set_dequeue(5, 1, 0x5010, false).control, (16 << 10) | (1 << 16) | (5 << 24));
        assert_eq!(port_power_off(u32::MAX), 0x0e00_c000);
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
            assert_eq!(w.waiter(), 17);
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
            assert_eq!(w.waiter(), 0);
            assert_eq!(w.signal(), 0);
            assert!(w.take());
        }
    }
}

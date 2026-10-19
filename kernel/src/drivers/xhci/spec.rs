//! Clean-room encodings from Intel xHCI 1.2, chapters 5-7, and USB 2.0 ch. 9.
//! No implementation from another USB stack was consulted.
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

pub const TRBS: usize = 256;
/// Event ring segment length: room for the completions of the `POSTED_TDS`
/// interrupt-IN TDs each pipe keeps posted, besides commands and port changes
/// (4.9.4).
pub const EVENT_TRBS: usize = 64;

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
    /// Where the next TRB goes, and its cycle bit, in a ring at `base`: the
    /// Set TR Dequeue Pointer target that skips the TDs already queued.
    pub fn next(&self, base: u64) -> (u64, bool) {
        (base + (self.index * 16) as u64, self.cycle)
    }
}

/// Interrupt-IN TDs kept posted on one endpoint. The controller schedules IN
/// transactions on an interrupt endpoint while a TD is posted (xHCI 4.11.2,
/// 4.14.2). With one TD, the endpoint is without one from a completion until
/// the worker handles it, and a Parallels probe boot lost reports sent in that
/// window (#1925), so the worker keeps several posted while it handles one.
pub const POSTED_TDS: usize = 4;

/// The posted TDs of one interrupt-IN pipe, by the buffer slot each fills:
/// slot `i` is bytes `i * stride..` of the pipe's buffer.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Posted {
    tds: [Option<u64>; POSTED_TDS],
}
impl Posted {
    pub const fn new() -> Self {
        Self { tds: [None; POSTED_TDS] }
    }
    /// The slot whose TD starts at `pointer`, if it is posted.
    pub fn slot(&self, pointer: u64) -> Option<usize> {
        self.tds.iter().position(|td| *td == Some(pointer))
    }
    /// `pointer`'s TD completed: its slot is free, and returned.
    pub fn complete(&mut self, pointer: u64) -> Option<usize> {
        let slot = self.slot(pointer)?;
        self.tds[slot] = None;
        Some(slot)
    }
    /// The slots with no TD posted, lowest first.
    pub fn free(&self) -> impl Iterator<Item = usize> + '_ {
        self.tds.iter().enumerate().filter(|(_, td)| td.is_none()).map(|(slot, _)| slot)
    }
    /// The posted TD the controller reaches first among those `completed`
    /// does not name, and its slot: the oldest, in a ring at `base` whose
    /// producer's next TRB is `enqueue`.
    #[cfg(any(test, target_arch = "x86_64", feature = "usb_pointer_replay"))]
    pub fn oldest_uncompleted(&self, completed: &[u64], base: u64, enqueue: (u64, bool)) -> Option<(usize, u64)> {
        self.tds
            .iter()
            .enumerate()
            .filter_map(|(slot, td)| Some((slot, (*td)?)))
            .filter(|(_, at)| !completed.contains(at))
            .max_by_key(|(_, at)| ring_age(base, enqueue, *at))
    }
    pub fn post(&mut self, slot: usize, pointer: u64) {
        self.tds[slot] = Some(pointer);
    }
    /// The ring's dequeue pointer moved past the posted TDs (4.6.10).
    pub fn abandon(&mut self) {
        self.tds = [None; POSTED_TDS];
    }
    #[cfg(any(test, feature = "usb_pointer_replay", feature = "xhci_fault_inject"))]
    pub fn full(&self) -> bool {
        self.tds.iter().all(Option::is_some)
    }
}

/// How many TRBs before the producer's `enqueue` the TRB at `at` was queued,
/// in a ring at `base` whose last TRB is the Link: larger is older.
#[cfg(any(test, target_arch = "x86_64", feature = "usb_pointer_replay"))]
pub fn ring_age(base: u64, enqueue: (u64, bool), at: u64) -> usize {
    let lap = TRBS - 1;
    let to = ((enqueue.0 - base) / 16) as usize;
    let from = ((at - base) / 16) as usize;
    (to + lap - from) % lap
}

/// The TRB after index `index` (whose cycle bit is `cycle`) and its cycle bit,
/// following the Link TRB, which toggles the cycle (4.9.2): a Set TR Dequeue
/// Pointer target past a one-TRB TD.
#[cfg(any(test, target_arch = "x86_64", feature = "usb_pointer_replay"))]
pub fn ring_next(base: u64, index: usize, cycle: bool) -> (u64, bool) {
    if index + 1 == TRBS - 1 {
        (base, !cycle)
    } else {
        (base + ((index + 1) * 16) as u64, cycle)
    }
}

/// Bytes between a pipe's buffer slots for `request`-byte reports: whole
/// 64-byte lines, so cache maintenance of one slot stays out of its
/// neighbour's lines.
pub fn posted_stride(request: u32, buffer: usize) -> Result<usize, &'static str> {
    let stride = (request as usize).div_ceil(64) * 64;
    if request == 0 || stride * POSTED_TDS > buffer {
        return Err("interrupt-IN report too large for its posted buffers");
    }
    Ok(stride)
}

/// USB 2.0 9.2.6.3: after SET_ADDRESS's Status stage the device has 2 ms
/// before it must accept requests at its new address. xHCI 4.6.5 leaves timing
/// that interval, after Address Device completes, to software.
pub const SET_ADDRESS_RECOVERY_NS: u64 = 2_000_000;
/// Attempts at one EP0 request, the first included.
pub const EP0_ATTEMPTS: u32 = 3;
/// The wait before resending a request that halted EP0: the time a device has
/// to complete a request with no Data stage (USB 2.0 9.2.6.4).
pub const EP0_RETRY_NS: u64 = 50_000_000;

/// What follows an EP0 control transfer's completion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ep0Next {
    /// Success: the request completed.
    Done,
    /// EP0 halted (4.8.3): Reset Endpoint, then Set TR Dequeue Pointer onto
    /// the next Setup TD (4.6.8), then send the request again after
    /// `after_ns`. The device clears a protocol stall on that SETUP (USB 2.0
    /// 8.5.3.4, xHCI 4.10.2.1.2).
    Retry { after_ns: u64 },
    /// A Short Packet in the Data stage: EP0 did not halt, and the xHC went on
    /// to the Status TD (4.10.1.1). Once that completes, send the request
    /// again after `after_ns`: the data received is shorter than requested.
    Resend { after_ns: u64 },
    /// EP0 halted on the last attempt: recover it, then report the failure.
    /// The device needs a reset (4.10.2.1.2), which is the caller's.
    RecoverAndFail,
    /// Report the completion as it is: the endpoint did not halt, or a
    /// short read persisted to the last attempt.
    Fail,
}

/// Stall, Babble Detected, USB Transaction Error and Split Transaction Error
/// halt a running endpoint (4.8.3, completion codes in table 6-90).
pub fn halts_endpoint(code: u8) -> bool {
    matches!(code, 3 | 4 | 6 | 36)
}

/// The step after attempt `attempt` (from 1) of an EP0 request completed with `code`.
pub fn ep0_next(code: u8, attempt: u32) -> Ep0Next {
    if code == 1 {
        Ep0Next::Done
    } else if code == 13 && attempt < EP0_ATTEMPTS {
        Ep0Next::Resend {
            after_ns: EP0_RETRY_NS,
        }
    } else if !halts_endpoint(code) {
        Ep0Next::Fail
    } else if attempt < EP0_ATTEMPTS {
        Ep0Next::Retry {
            after_ns: EP0_RETRY_NS,
        }
    } else {
        Ep0Next::RecoverAndFail
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
pub(super) const fn setup(request_type: u8, request: u8, value: u16, index: u16, length: u16) -> u64 {
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
    fn posted_tds_fill_free_slots_and_complete_in_any_order() {
        let mut p = Posted::new();
        assert_eq!(p.free().collect::<Vec<_>>(), [0, 1, 2, 3]);
        for (slot, pointer) in [(0, 0x1000), (1, 0x1010), (2, 0x1020), (3, 0x1030)] {
            p.post(slot, pointer);
        }
        assert!(p.full() && p.free().next().is_none());
        assert_eq!(p.complete(0x1010), Some(1));
        assert_eq!(p.complete(0x1010), None, "a completed TD is not posted twice");
        assert_eq!(p.complete(0x2000), None, "a stale event names no slot");
        assert_eq!(p.free().collect::<Vec<_>>(), [1]);
        p.post(1, 0x1040);
        assert_eq!(p.slot(0x1040), Some(1));
        p.abandon();
        assert!(p.free().count() == POSTED_TDS);
        assert_eq!(posted_stride(8, 4096), Ok(64));
        assert_eq!(posted_stride(65, 4096), Ok(128));
        assert_eq!(posted_stride(1024, 4096), Ok(1024));
        assert!(posted_stride(1025, 4096).is_err() && posted_stride(0, 4096).is_err());
    }
    #[test]
    fn ring_positions_order_tds_and_step_over_the_link() {
        let base = 0x10_0000;
        let mut p = Producer::new();
        for _ in 0..TRBS - 3 {
            p.advance();
        }
        // Three TDs queued at indexes 253 and 254 (the last before the Link
        // at 255), then 0 on the next lap.
        let queued: Vec<(u64, bool)> = (0..3)
            .map(|_| {
                let at = p.next(base);
                p.advance();
                at
            })
            .collect();
        assert_eq!(queued, [(base + 253 * 16, true), (base + 254 * 16, true), (base, false)]);
        let enqueue = p.next(base);
        assert_eq!(enqueue, (base + 16, false));
        let ages: Vec<usize> = queued.iter().map(|q| ring_age(base, enqueue, q.0)).collect();
        assert_eq!(ages, [3, 2, 1]);
        assert_eq!(ring_next(base, 253, true), queued[1]);
        assert_eq!(ring_next(base, 254, true), queued[2]);
        assert_eq!(ring_next(base, 0, false), enqueue);
        // Posted out of slot order across the Link: the oldest is the TD at 253,
        // not the newest at 0, and one already completed is passed over.
        let mut posted = Posted::new();
        posted.post(2, queued[0].0);
        posted.post(0, queued[1].0);
        posted.post(1, queued[2].0);
        assert_eq!(posted.oldest_uncompleted(&[], base, enqueue), Some((2, queued[0].0)));
        assert_eq!(posted.oldest_uncompleted(&[queued[0].0], base, enqueue), Some((0, queued[1].0)));
        assert_eq!(posted.oldest_uncompleted(&[queued[0].0, queued[1].0, queued[2].0], base, enqueue), None);
    }
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
    fn a_halted_ep0_is_recovered_and_its_request_sent_again() {
        // A stalled request is resent until the last attempt, which still
        // recovers EP0 before reporting, so the next request starts clean.
        assert_eq!(ep0_next(6, 1), Ep0Next::Retry { after_ns: EP0_RETRY_NS });
        assert_eq!(ep0_next(6, EP0_ATTEMPTS - 1), Ep0Next::Retry { after_ns: EP0_RETRY_NS });
        assert_eq!(ep0_next(6, EP0_ATTEMPTS), Ep0Next::RecoverAndFail);
        for code in [3, 4, 36] {
            assert_eq!(ep0_next(code, 1), Ep0Next::Retry { after_ns: EP0_RETRY_NS });
        }
        assert_eq!(ep0_next(1, 1), Ep0Next::Done);
        assert_eq!(ep0_next(1, EP0_ATTEMPTS), Ep0Next::Done);
        // A short read does not halt the endpoint: it is sent again with no
        // Reset Endpoint (which would fail with Context State Error), and a
        // read still short on the last attempt is reported. A TRB error is
        // reported at once.
        assert_eq!(ep0_next(13, 1), Ep0Next::Resend { after_ns: EP0_RETRY_NS });
        assert_eq!(ep0_next(13, EP0_ATTEMPTS - 1), Ep0Next::Resend { after_ns: EP0_RETRY_NS });
        assert_eq!(ep0_next(13, EP0_ATTEMPTS), Ep0Next::Fail);
        assert!(!halts_endpoint(13));
        assert_eq!(ep0_next(5, 1), Ep0Next::Fail);
        // The recovery interval is USB 2.0's 2 ms, the retry its 50 ms bound.
        assert_eq!((SET_ADDRESS_RECOVERY_NS, EP0_RETRY_NS), (2_000_000, 50_000_000));
    }
    #[test]
    fn set_dequeue_after_a_stall_skips_the_failed_td_across_the_link() {
        // A stalled no-data request (Setup, Status) as the last two TRBs
        // before the Link TRB: the dequeue goes to the ring's start with the
        // cycle the producer writes there next, so the stale TRBs (old cycle)
        // are not executed again and the resent Setup TD is.
        let base = 0x8000;
        let mut p = Producer::new();
        while p.index != TRBS - 3 {
            p.advance();
        }
        assert_eq!(p.next(base), (base + (TRBS as u64 - 3) * 16, true));
        p.advance(); // Setup Stage
        assert!(p.advance()); // Status Stage, then the Link TRB toggles the cycle
        let (pointer, cycle) = p.next(base);
        assert_eq!((pointer, cycle), (base, false));
        let command = set_dequeue(1, 1, pointer, cycle);
        assert_eq!(command.parameter, base);
        assert_eq!(command.control, (16 << 10) | (1 << 16) | (1 << 24));
        // Mid-ring: the TRB after the failed TD, with DCS set.
        let mut p = Producer::new();
        p.advance();
        p.advance();
        let (pointer, cycle) = p.next(base);
        assert_eq!(set_dequeue(3, 1, pointer, cycle).parameter, base + 32 + 1);
        assert_eq!(reset_endpoint(3, 1).control, (14 << 10) | (1 << 16) | (3 << 24));
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

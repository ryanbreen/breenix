//! The virtio entropy device's request queue (kernel/src/drivers/virtio/rng_queue.rs)
//! against the virtio 1.x specification: its identity, the split virtqueue
//! layout and flags, and the device operation of section 5.4, driven by a
//! simulated device.

use std::mem::{align_of, offset_of, size_of};

use crate::rng_queue::{
    AvailRing, Completion, Descriptor, QueueMemory, RequestQueue, UsedElem, UsedRing, AVAIL_F_NO_INTERRUPT,
    DESC_F_WRITE, DEVICE_ID, LEGACY_ALIGN, PCI_DEVICE_ID, PCI_TRANSITIONAL_DEVICE_ID, QUEUE_SIZE,
};

#[test]
fn identity_matches_the_specification() {
    // 5.4.1: device ID 4. 4.1.2.1: PCI device ID 0x1040 plus the device ID;
    // transitional entropy source 0x1005.
    assert_eq!(DEVICE_ID, 4);
    assert_eq!(PCI_DEVICE_ID, 0x1044);
    assert_eq!(PCI_TRANSITIONAL_DEVICE_ID, 0x1005);
    // 2.7.5 and 2.7.6.
    assert_eq!(DESC_F_WRITE, 2);
    assert_eq!(AVAIL_F_NO_INTERRUPT, 1);
}

#[test]
fn layout_matches_the_split_virtqueue_table() {
    let n = QUEUE_SIZE as usize;
    assert!(QUEUE_SIZE.is_power_of_two());
    // 2.7: descriptor table 16 * size (16-aligned), available ring 6 + 2 * size
    // (2-aligned), used ring 6 + 8 * size (4-aligned).
    assert_eq!(size_of::<Descriptor>() * n, 16 * n);
    assert_eq!(size_of::<AvailRing>(), 6 + 2 * n);
    assert_eq!(offset_of!(UsedRing, avail_event) + 2, 6 + 8 * n);
    assert_eq!(size_of::<UsedElem>(), 8);
    assert!(align_of::<UsedRing>() <= 4);
    assert_eq!(offset_of!(QueueMemory, desc) % 16, 0);
    assert_eq!(offset_of!(QueueMemory, avail), 16 * n);
    assert_eq!(QueueMemory::AVAIL_OFFSET as usize, offset_of!(QueueMemory, avail));
    // 2.7.2 legacy layout: the used ring at the next QueueAlign boundary (4096)
    // after the available ring, which the modern layout's 4-alignment also allows.
    assert_eq!(LEGACY_ALIGN, 4096);
    let legacy_used = (16 * n + 6 + 2 * n).div_ceil(LEGACY_ALIGN) * LEGACY_ALIGN;
    assert_eq!(offset_of!(QueueMemory, used), legacy_used);
    assert_eq!(QueueMemory::USED_OFFSET as usize, legacy_used);
    assert_eq!(align_of::<QueueMemory>(), 4096);
}

/// A device that follows 5.4.6.2: it takes the next available buffer and
/// writes `len` random bytes into it.
struct Device {
    last_avail: u16,
    used_idx: u16,
}

impl Device {
    fn new() -> Self {
        Self { last_avail: 0, used_idx: 0 }
    }

    /// Answer one request, writing `fill` bytes of `byte` and reporting `report` as the used length.
    fn answer(&mut self, memory: &mut QueueMemory, fill: usize, report: u32, id: Option<u32>) {
        assert_ne!(memory.avail.idx, self.last_avail, "the driver offered nothing");
        let head = memory.avail.ring[self.last_avail as usize % QUEUE_SIZE as usize];
        self.last_avail = self.last_avail.wrapping_add(1);
        let descriptor = memory.desc[head as usize];
        // 5.4.6.1: the driver offers only device-writable buffers.
        assert_eq!(descriptor.flags, DESC_F_WRITE);
        assert!(fill <= descriptor.len as usize);
        let buffer = unsafe { std::slice::from_raw_parts_mut(descriptor.addr as *mut u8, descriptor.len as usize) };
        buffer[..fill].fill(0xa5);
        let slot = self.used_idx as usize % QUEUE_SIZE as usize;
        memory.used.ring[slot] = UsedElem { id: id.unwrap_or(u32::from(head)), len: report };
        self.used_idx = self.used_idx.wrapping_add(1);
        memory.used.idx = self.used_idx;
    }
}

fn setup() -> (Box<QueueMemory>, RequestQueue, Vec<u8>) {
    let mut memory = Box::new(QueueMemory::new());
    memory.used.idx = 77; // stale contents the driver must clear
    let queue = RequestQueue::new(&mut memory);
    assert_eq!(memory.used.idx, 0);
    assert_eq!(memory.avail.flags, AVAIL_F_NO_INTERRUPT);
    (memory, queue, vec![0u8; 64])
}

#[test]
fn a_request_returns_the_bytes_the_device_reports() {
    let (mut memory, mut queue, mut buffer) = setup();
    let phys = buffer.as_mut_ptr() as u64;
    assert!(queue.offer(&mut memory, phys, 64));
    assert_eq!(memory.avail.idx, 1);
    assert_eq!(memory.desc[0].addr, phys);
    assert_eq!(memory.desc[0].len, 64);
    assert_eq!(queue.completed(&memory), None, "nothing until the device answers");

    let mut device = Device::new();
    device.answer(&mut memory, 64, 64, None);
    assert_eq!(queue.completed(&memory), Some(Completion::Bytes(64)));
    assert!(buffer.iter().all(|&b| b == 0xa5));
    assert_eq!(queue.completed(&memory), None, "a request completes once");
}

#[test]
fn a_short_answer_is_taken_at_its_length() {
    // 5.4.6.2: the device MAY use less than the entire buffer; 5.4.6.1: the
    // driver MUST examine the length written.
    let (mut memory, mut queue, mut buffer) = setup();
    assert!(queue.offer(&mut memory, buffer.as_mut_ptr() as u64, 64));
    Device::new().answer(&mut memory, 5, 5, None);
    assert_eq!(queue.completed(&memory), Some(Completion::Bytes(5)));
}

#[test]
fn one_request_is_outstanding_at_a_time() {
    let (mut memory, mut queue, mut buffer) = setup();
    let phys = buffer.as_mut_ptr() as u64;
    assert!(queue.offer(&mut memory, phys, 64));
    assert!(!queue.offer(&mut memory, phys, 64));
    assert_eq!(memory.avail.idx, 1);
    // An unanswered request stays outstanding until the device answers it.
    assert_eq!(queue.completed(&memory), None);
    Device::new().answer(&mut memory, 64, 64, None);
    assert_eq!(queue.completed(&memory), Some(Completion::Bytes(64)));
    assert!(!queue.offer(&mut memory, phys, 0), "an empty buffer is never offered");
    assert!(queue.offer(&mut memory, phys, 64));
}

#[test]
fn answers_the_specification_forbids_are_malformed() {
    for (fill, report, id) in [(0, 0, None), (64, 65, None), (64, 64, Some(1))] {
        let (mut memory, mut queue, mut buffer) = setup();
        assert!(queue.offer(&mut memory, buffer.as_mut_ptr() as u64, 64));
        Device::new().answer(&mut memory, fill, report, id);
        assert_eq!(queue.completed(&memory), Some(Completion::Malformed), "fill {fill} report {report} id {id:?}");
    }
}

#[test]
fn ring_indices_wrap() {
    let (mut memory, mut queue, mut buffer) = setup();
    let phys = buffer.as_mut_ptr() as u64;
    let mut device = Device::new();
    // Past the ring size many times, and past the 16-bit index.
    for round in 0..70_000u32 {
        assert!(queue.offer(&mut memory, phys, 64), "round {round}");
        let len = 1 + round % 64;
        device.answer(&mut memory, len as usize, len, None);
        assert_eq!(queue.completed(&memory), Some(Completion::Bytes(len)));
    }
    assert_eq!(memory.avail.idx, (70_000u32 % 65_536) as u16);
}

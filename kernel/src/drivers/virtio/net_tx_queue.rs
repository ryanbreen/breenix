//! The driver's side of a virtio-net transmit queue (virtio 1.2 section 2.7,
//! split virtqueues): which descriptors the device holds, where the next
//! available-ring entry goes, and how far the used ring has been read.
//!
//! One `TxQueue` per transmit queue, behind that queue's own lock. A submission
//! takes the descriptor, writes it and its available-ring entry, orders those
//! stores before the index, and stores `avail.idx` in one critical section, so
//! the index moves forward by exactly one per buffer and is never stored past an
//! entry that has not been written (section 2.7.13, "Supplying Buffers to The
//! Device"). A submission first takes back the descriptors the device has
//! finished with (section 2.7.14), so a full queue drains on the transmit path
//! and does not wait for receive processing.
//!
//! Pure computation over a `TxRing`, so the host tests compile this file as is.

/// Largest queue this bookkeeping tracks: one bit per descriptor in `in_flight`.
pub const MAX_QUEUE_SIZE: u16 = 32;

/// The queue memory the device reads and writes.
pub trait TxRing {
    /// Descriptor `slot` names `len` device-readable bytes at `addr`.
    fn write_desc(&mut self, slot: u16, addr: u64, len: u32);
    /// The available ring's entry `pos` (already reduced modulo the queue size) is `slot`.
    fn write_avail_entry(&mut self, pos: u16, slot: u16);
    /// Section 2.7.13 step 3: the device sees the descriptor and the ring entry
    /// before it sees the index that covers them.
    fn barrier_before_avail_idx(&mut self);
    /// Store `avail.idx`.
    fn write_avail_idx(&mut self, idx: u16);
    /// Section 2.7.13 step 5: the index store is visible before the driver
    /// notifies the device.
    fn barrier_before_notify(&mut self);
    /// `used.idx` as the device last stored it, ordered before the used-ring
    /// entries read after it.
    fn read_used_idx(&mut self) -> u16;
    /// The descriptor id in used-ring entry `pos` (already reduced modulo the queue size).
    fn read_used_id(&mut self, pos: u16) -> u32;
}

/// Why a submission was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TxError {
    /// The queue has not been configured with a size.
    NotReady,
    /// Every descriptor is still held by the device.
    Full,
}

/// One transmit queue's driver state. Every method needs `&mut self`: the lock
/// that holds the queue is what serializes CPUs.
pub struct TxQueue<R> {
    ring: R,
    /// Descriptors in the queue, a power of two; 0 until `configure`.
    size: u16,
    /// The driver's copy of `avail.idx`, the next available-ring position.
    avail_idx: u16,
    /// How far the used ring has been read.
    last_used: u16,
    /// Bit `n` is set while descriptor `n` belongs to the device.
    in_flight: u32,
}

impl<R: TxRing> TxQueue<R> {
    pub const fn new(ring: R) -> Self {
        Self {
            ring,
            size: 0,
            avail_idx: 0,
            last_used: 0,
            in_flight: 0,
        }
    }

    /// Start over with a queue of `device_max` descriptors at most, after the
    /// device's rings have been zeroed. The split ring's positions are indices
    /// modulo the size, which wrap with the 16-bit index only for a power of two.
    pub fn configure(&mut self, device_max: u16, ring_capacity: u16) {
        self.size = split_queue_size(device_max, ring_capacity.min(MAX_QUEUE_SIZE));
        self.avail_idx = 0;
        self.last_used = 0;
        self.in_flight = 0;
    }

    /// The configured queue size (0 when unconfigured).
    pub fn size(&self) -> u16 {
        self.size
    }

    /// Descriptors the device currently holds.
    pub fn in_flight(&self) -> u32 {
        self.in_flight.count_ones()
    }

    /// Take back every descriptor the device has returned. A used entry naming
    /// a descriptor the driver did not hand over is skipped; it frees nothing.
    pub fn reclaim(&mut self) -> usize {
        if self.size == 0 {
            return 0;
        }
        let used_idx = self.ring.read_used_idx();
        let mut reclaimed = 0;
        while self.last_used != used_idx {
            let id = self.ring.read_used_id(self.last_used % self.size);
            self.last_used = self.last_used.wrapping_add(1);
            if id < u32::from(self.size) && self.in_flight & (1 << id) != 0 {
                self.in_flight &= !(1 << id);
                reclaimed += 1;
            }
        }
        reclaimed
    }

    /// Hand one buffer to the device. `fill` gets the descriptor chosen and
    /// returns the bus address and length of the bytes it wrote for it; the
    /// descriptor, its ring entry and the new index are then published in order.
    /// Returns the index stored. The caller notifies the device afterwards.
    pub fn submit(&mut self, fill: impl FnOnce(u16) -> (u64, u32)) -> Result<u16, TxError> {
        if self.size == 0 {
            return Err(TxError::NotReady);
        }
        self.reclaim();
        let free = !self.in_flight & mask(self.size);
        if free == 0 {
            return Err(TxError::Full);
        }
        let slot = free.trailing_zeros() as u16;
        self.in_flight |= 1 << slot;
        let (addr, len) = fill(slot);
        self.ring.write_desc(slot, addr, len);
        self.ring.write_avail_entry(self.avail_idx % self.size, slot);
        self.ring.barrier_before_avail_idx();
        self.avail_idx = self.avail_idx.wrapping_add(1);
        self.ring.write_avail_idx(self.avail_idx);
        self.ring.barrier_before_notify();
        Ok(self.avail_idx)
    }
}

/// The queue size to set: the largest power of two no larger than either the
/// device's maximum or the ring's capacity, or 0 if either is 0.
pub fn split_queue_size(device_max: u16, ring_capacity: u16) -> u16 {
    let limit = device_max.min(ring_capacity);
    if limit == 0 {
        0
    } else {
        1 << (15 - limit.leading_zeros() as u16)
    }
}

fn mask(size: u16) -> u32 {
    if size >= 32 {
        u32::MAX
    } else {
        (1u32 << size) - 1
    }
}

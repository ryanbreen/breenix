//! The driver's side of a virtio-net transmit queue (virtio 1.2 section 2.7,
//! split virtqueues): which descriptors the device holds, where the next
//! available-ring entry goes, how far the used ring has been read, and whether
//! the device is asked to interrupt when it returns buffers.
//!
//! One `TxQueue` per transmit queue, behind that queue's own lock. A submission
//! takes its descriptors, writes them and the available-ring entry for the
//! chain's head, orders those stores before the index, and stores `avail.idx`
//! in one critical section, so the index moves forward by exactly one per
//! buffer and is not stored before the entry it covers is written (section
//! 2.7.13, "Supplying Buffers to The Device"). A submission first takes back
//! the descriptors the device has finished with (section 2.7.14), so a full
//! queue drains on the transmit path and does not wait for receive processing.
//!
//! While the queue has room the device is asked not to interrupt for returned
//! buffers: the next submission reclaims them. When a submission finds the
//! queue full, the queue asks for an interrupt on the next returned buffer and
//! looks at the used ring again, so a completion that raced the request is not
//! missed (section 2.7.14's re-check). That interrupt is what lets a stalled
//! sender go on without receive traffic. With VIRTIO_F_EVENT_IDX the request
//! is `used_event` (section 2.7.7); without it, the available ring's
//! NO_INTERRUPT flag.
//!
//! Pure computation over a `TxRing`, so the host tests compile this file as is.

/// Largest queue this bookkeeping tracks.
pub const MAX_QUEUE_SIZE: u16 = 1024;
/// Most descriptors one submission chains.
pub const MAX_CHAIN: usize = 40;

const WORDS: usize = MAX_QUEUE_SIZE as usize / 64;
/// The available ring's flag asking the device not to interrupt (section 2.7.6).
pub const AVAIL_F_NO_INTERRUPT: u16 = 1;
/// The used ring's flag asking the driver not to notify (section 2.7.8).
pub const USED_F_NO_NOTIFY: u16 = 1;

/// The queue memory the device reads and writes.
pub trait TxRing {
    /// Descriptor `slot` names `len` device-readable bytes at `addr`, and ends
    /// its chain.
    fn write_desc(&mut self, slot: u16, addr: u64, len: u32);
    /// Descriptor `slot` names `len` device-readable bytes at `addr`, and the
    /// chain goes on at descriptor `next`.
    fn write_desc_next(&mut self, slot: u16, addr: u64, len: u32, next: u16);
    /// The available ring's entry `pos` (already reduced modulo the queue size) is `slot`.
    fn write_avail_entry(&mut self, pos: u16, slot: u16);
    /// Section 2.7.13 step 4: the device sees the descriptor and the ring entry
    /// before it sees the index that covers them.
    fn barrier_before_avail_idx(&mut self);
    /// Store `avail.idx`.
    fn write_avail_idx(&mut self, idx: u16);
    /// Section 2.7.13 step 6 and 2.7.13.4.1: the index store is visible before
    /// the driver reads the device's notification suppression and notifies it.
    fn barrier_before_notify(&mut self);
    /// `used.idx` as the device last stored it, ordered before the used-ring
    /// entries read after it.
    fn read_used_idx(&mut self) -> u16;
    /// The descriptor id in used-ring entry `pos` (already reduced modulo the queue size).
    fn read_used_id(&mut self, pos: u16) -> u32;
    /// The device's `avail_event`, after the ring's `used` entries.
    fn read_avail_event(&mut self) -> u16;
    /// The used ring's flags.
    fn read_used_flags(&mut self) -> u16;
    /// Store `used_event`, after the available ring's entries.
    fn write_used_event(&mut self, idx: u16);
    /// Store the available ring's flags.
    fn write_avail_flags(&mut self, flags: u16);
    /// Order the stores before it against the loads after it: a request for an
    /// interrupt against the re-check of `used.idx`.
    fn full_barrier(&mut self);
}

/// Why a submission was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TxError {
    /// The queue has not been configured with a size.
    NotReady,
    /// The device still holds the descriptors a submission needs.
    Full,
    /// The submission needs more descriptors than a chain may have.
    TooLong,
}

/// virtio 1.2 appendix A's `virtq_need_event`: has the 16-bit index moved from
/// `old` to `new` past `event`, so that the other side asked to be told?
pub fn need_event(event: u16, new: u16, old: u16) -> bool {
    new.wrapping_sub(event).wrapping_sub(1) < new.wrapping_sub(old)
}

/// One transmit queue's driver state. Its methods take `&mut self`: the lock
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
    in_flight: [u64; WORDS],
    /// Bit `n` is set while descriptor `n` heads a chain the device holds.
    heads: [u64; WORDS],
    /// The descriptor after `n` in its chain, while `n` is in flight.
    next: [u16; MAX_QUEUE_SIZE as usize],
    /// Descriptors in the chain `n` heads, while it is in flight.
    chain_len: [u8; MAX_QUEUE_SIZE as usize],
    /// VIRTIO_F_EVENT_IDX was negotiated.
    event_idx: bool,
    /// The device was asked to interrupt on its next returned buffer.
    armed: bool,
}

impl<R: TxRing> TxQueue<R> {
    pub const fn new(ring: R) -> Self {
        Self {
            ring,
            size: 0,
            avail_idx: 0,
            last_used: 0,
            in_flight: [0; WORDS],
            heads: [0; WORDS],
            next: [0; MAX_QUEUE_SIZE as usize],
            chain_len: [0; MAX_QUEUE_SIZE as usize],
            event_idx: false,
            armed: false,
        }
    }

    /// Start over with a queue of `device_max` descriptors at most, after the
    /// device's rings have been zeroed. The split ring's positions are indices
    /// modulo the size, which wrap with the 16-bit index only for a power of two.
    pub fn configure(&mut self, device_max: u16, ring_capacity: u16) {
        self.size = split_queue_size(device_max, ring_capacity.min(MAX_QUEUE_SIZE));
        self.avail_idx = 0;
        self.last_used = 0;
        self.in_flight = [0; WORDS];
        self.heads = [0; WORDS];
        self.armed = false;
    }

    /// The ring memory this queue publishes to.
    pub fn ring_mut(&mut self) -> &mut R {
        &mut self.ring
    }

    /// Ask the device not to interrupt for returned buffers while the queue has
    /// room, with `used_event` when `event_idx` was negotiated.
    pub fn start(&mut self, event_idx: bool) {
        self.event_idx = event_idx;
        self.armed = false;
        self.suppress();
    }

    fn suppress(&mut self) {
        if self.event_idx {
            // An index the used ring has just passed: reached again only after
            // 65536 more buffers, far more than the at most 1024 in flight
            // that a reclaim takes back before moving it on.
            self.ring.write_used_event(self.last_used.wrapping_sub(1));
        } else {
            self.ring.write_avail_flags(AVAIL_F_NO_INTERRUPT);
        }
    }

    /// The configured queue size (0 when unconfigured).
    pub fn size(&self) -> u16 {
        self.size
    }

    /// Descriptors the device currently holds.
    pub fn in_flight(&self) -> u32 {
        self.in_flight.iter().map(|w| w.count_ones()).sum()
    }

    fn free(&self) -> u32 {
        u32::from(self.size) - self.in_flight()
    }

    fn bit(map: &[u64; WORDS], n: u16) -> bool {
        map[usize::from(n) / 64] & (1 << (n % 64)) != 0
    }

    fn set(map: &mut [u64; WORDS], n: u16, on: bool) {
        let word = &mut map[usize::from(n) / 64];
        if on {
            *word |= 1 << (n % 64);
        } else {
            *word &= !(1 << (n % 64));
        }
    }

    /// The lowest descriptor the device does not hold.
    fn lowest_free(&self) -> Option<u16> {
        let words = usize::from(self.size).div_ceil(64);
        for (i, word) in self.in_flight[..words].iter().enumerate() {
            let free = !word;
            if free != 0 {
                let n = (i * 64) as u16 + free.trailing_zeros() as u16;
                return (n < self.size).then_some(n);
            }
        }
        None
    }

    /// Take back the descriptors the device has returned since the last call.
    /// A used entry naming a descriptor that does not head a chain the device
    /// holds is skipped without freeing a descriptor. Returns the chains taken
    /// back.
    pub fn reclaim(&mut self) -> usize {
        if self.size == 0 {
            return 0;
        }
        let used_idx = self.ring.read_used_idx();
        let mut reclaimed = 0;
        while self.last_used != used_idx {
            let id = self.ring.read_used_id(self.last_used % self.size);
            self.last_used = self.last_used.wrapping_add(1);
            if id >= u32::from(self.size) || !Self::bit(&self.heads, id as u16) {
                continue;
            }
            let head = id as u16;
            Self::set(&mut self.heads, head, false);
            let mut slot = head;
            for _ in 0..self.chain_len[usize::from(head)] {
                Self::set(&mut self.in_flight, slot, false);
                slot = self.next[usize::from(slot)];
            }
            reclaimed += 1;
        }
        if reclaimed != 0 {
            if self.event_idx || self.armed {
                // Keep `used_event` behind the ring, or put NO_INTERRUPT back.
                self.armed = false;
                self.suppress();
            }
        }
        reclaimed
    }

    /// Ask the device to interrupt on its next returned buffer, then read the
    /// used ring again: a buffer returned before the request was seen raises
    /// no interrupt, so it is taken back now.
    fn arm_and_recheck(&mut self) -> usize {
        self.armed = true;
        if self.event_idx {
            self.ring.write_used_event(self.last_used);
        } else {
            self.ring.write_avail_flags(0);
        }
        self.ring.full_barrier();
        self.reclaim()
    }

    /// Hand one buffer to the device. `fill` gets the descriptor chosen and
    /// returns the bus address and length of the bytes it wrote for it; the
    /// descriptor, its ring entry and the new index are then published in order.
    /// Returns the index stored. The caller then asks `should_notify`.
    pub fn submit(&mut self, fill: impl FnOnce(u16) -> (u64, u32)) -> Result<u16, TxError> {
        self.submit_chain(1, |slots, out| out[0] = fill(slots[0]))
    }

    /// Hand one buffer of `count` descriptors to the device, chained in the
    /// order `fill` is given them. `fill` writes the bytes for each descriptor
    /// and returns their bus addresses and lengths in `out`.
    pub fn submit_chain(
        &mut self,
        count: usize,
        fill: impl FnOnce(&[u16], &mut [(u64, u32)]),
    ) -> Result<u16, TxError> {
        if self.size == 0 {
            return Err(TxError::NotReady);
        }
        if count == 0 || count > MAX_CHAIN || count > usize::from(self.size) {
            return Err(TxError::TooLong);
        }
        self.reclaim();
        // A re-check that takes chains back puts suppression back, so while
        // the chain still does not fit, ask again: the refusal must leave the
        // request standing, or the next completion raises no interrupt.
        // Each pass takes back at least one chain, so this ends.
        while (self.free() as usize) < count {
            if self.arm_and_recheck() == 0 {
                return Err(TxError::Full);
            }
        }
        let mut slots = [0u16; MAX_CHAIN];
        for i in 0..count {
            let slot = self.lowest_free().ok_or(TxError::Full)?;
            Self::set(&mut self.in_flight, slot, true);
            slots[i] = slot;
        }
        let mut out = [(0u64, 0u32); MAX_CHAIN];
        fill(&slots[..count], &mut out[..count]);
        let head = slots[0];
        for i in 0..count {
            let (addr, len) = out[i];
            if i + 1 < count {
                self.next[usize::from(slots[i])] = slots[i + 1];
                self.ring.write_desc_next(slots[i], addr, len, slots[i + 1]);
            } else {
                self.ring.write_desc(slots[i], addr, len);
            }
        }
        self.chain_len[usize::from(head)] = count as u8;
        Self::set(&mut self.heads, head, true);
        self.ring.write_avail_entry(self.avail_idx % self.size, head);
        self.ring.barrier_before_avail_idx();
        self.avail_idx = self.avail_idx.wrapping_add(1);
        self.ring.write_avail_idx(self.avail_idx);
        self.ring.barrier_before_notify();
        Ok(self.avail_idx)
    }

    /// After a submission stored `new_idx`, whether the device asked to be
    /// notified of it (section 2.7.10.1): with EVENT_IDX, when the index moved
    /// past `avail_event`; without it, unless the used ring's NO_NOTIFY is set.
    /// `submit` has already ordered its index store before this read.
    pub fn should_notify(&mut self, new_idx: u16) -> bool {
        if self.event_idx {
            let event = self.ring.read_avail_event();
            need_event(event, new_idx, new_idx.wrapping_sub(1))
        } else {
            self.ring.read_used_flags() & USED_F_NO_NOTIFY == 0
        }
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

//! The driver's side of a virtio-blk request queue (virtio 1.2 sections 2.7
//! and 5.2): several request chains in flight on one split virtqueue, each
//! chain a readable header, its data segments and a writable status byte.
//!
//! The engine owns the descriptor free list, the map from a chain's head
//! descriptor to the chain slot that issued it, and the used-ring cursor. The
//! free list and each chain's links live in driver memory, so a device that
//! writes the descriptor table cannot redirect them. Each chain slot owns its
//! header, its status byte and its data frames (individually allocated kernel
//! frames, one descriptor each unless physically adjacent) from submission
//! until its waiter takes the result, or for good once the queue stops.
//!
//! Submission writes the chain's descriptors and its available-ring entry;
//! `publish` then orders them before the one `avail.idx` store that makes a
//! window of chains visible (section 2.7.13). `drain` reads the used ring
//! entry by entry, in whatever order the device returned the chains, and
//! accepts only the head of a chain it issued (section 2.7.8). An unknown
//! head, or an index that runs past the chains outstanding, stops the queue:
//! no chain is completed or reused after that, and the slots keep their
//! memory until the device is reset and the queue rebuilt.
//!
//! The same-range table orders requests from different callers: overlapping
//! ranges are serialized when either writes, and a flush covers the whole
//! device, so it waits for the writes in flight (section 5.2.6.2: a flush
//! commits the writes completed before it was sent) and holds later ones.
//!
//! Pure computation over the `RingMemory` and `DmaMemory` traits, so the host
//! tests compile this file as is.

/// Largest split queue the engine tracks.
pub const MAX_QUEUE_SIZE: u16 = 256;
/// Chain slots per device.
pub const MAX_CHAINS: usize = 16;
/// Data frames per chain.
pub const MAX_SEGMENTS: usize = 16;
/// Bytes in one data frame.
pub const FRAME_BYTES: usize = 4096;
/// Virtio-blk's wire sector (section 5.2.6: sector numbers and data lengths
/// count 512-byte units whatever blk_size the device suggests).
pub const SECTOR_BYTES: usize = 512;
/// Bytes of the request header the device reads (type, reserved, sector).
pub const HEADER_BYTES: u32 = 16;

const DESC_F_NEXT: u16 = 1;
const DESC_F_WRITE: u16 = 2;
const NO_SLOT: u8 = u8::MAX;
const STATUS_UNWRITTEN: u8 = 0xff;

/// Request types (section 5.2.6).
pub mod request_type {
    pub const IN: u32 = 0;
    pub const OUT: u32 = 1;
    pub const FLUSH: u32 = 4;
}

/// Status values (section 5.2.6).
pub mod status {
    pub const OK: u8 = 0;
    pub const IOERR: u8 = 1;
    pub const UNSUPP: u8 = 2;
}

/// One descriptor as the device reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Desc {
    pub addr: u64,
    pub len: u32,
    pub flags: u16,
    pub next: u16,
}

/// The queue memory the device reads and writes.
pub trait RingMemory {
    /// Store descriptor `index`.
    fn write_desc(&mut self, index: u16, desc: Desc);
    /// The available ring's entry `pos` (already reduced modulo the size) is `head`.
    fn write_avail_entry(&mut self, pos: u16, head: u16);
    /// Descriptors, buffers and ring entries are visible before the index
    /// that covers them.
    fn barrier_before_avail_idx(&mut self);
    /// Store `avail.idx`.
    fn write_avail_idx(&mut self, idx: u16);
    /// The index store is visible before the driver notifies the device.
    fn barrier_before_notify(&mut self);
    /// `used.idx` as the device last stored it, ordered before the used-ring
    /// entries and device-written buffers read after it.
    fn read_used_idx(&mut self) -> u16;
    /// Used-ring entry `pos` (already reduced modulo the size): (id, len).
    fn read_used_elem(&mut self, pos: u16) -> (u32, u32);
}

/// The chains' own buffers, by bus address.
pub trait DmaMemory {
    fn write_bytes(&mut self, addr: u64, bytes: &[u8]);
    fn read_bytes(&mut self, addr: u64, out: &mut [u8]);
}

/// Where a split queue's parts sit in its memory (section 2.7: the
/// descriptor table, then the available ring with its used_event, then the
/// used ring at the next `used_align` boundary, as the legacy layout has it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SplitLayout {
    pub avail: usize,
    pub used: usize,
    pub bytes: usize,
}

pub const fn split_layout(size: u16, used_align: usize) -> SplitLayout {
    let size = size as usize;
    let avail = 16 * size;
    let avail_end = avail + 6 + 2 * size;
    let used = avail_end.div_ceil(used_align) * used_align;
    SplitLayout { avail, used, bytes: used + 6 + 8 * size }
}

/// The queue size to set: the largest power of two no larger than either the
/// device's maximum or `limit`, or 0 if either is 0.
pub fn split_queue_size(device_max: u32, limit: u16) -> u16 {
    let limit = device_max.min(u32::from(limit));
    if limit == 0 {
        0
    } else {
        1 << (31 - limit.leading_zeros())
    }
}

/// One chain slot's buffers, fixed when the device is set up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChainStorage {
    pub header: u64,
    pub status: u64,
    pub frames: [u64; MAX_SEGMENTS],
    pub frame_count: usize,
}

impl ChainStorage {
    pub const EMPTY: Self = Self { header: 0, status: 0, frames: [0; MAX_SEGMENTS], frame_count: 0 };
}

/// What the device negotiated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Data descriptors per request (VIRTIO_BLK_F_SEG_MAX), 0 when not offered.
    pub seg_max: u32,
    /// Bytes per descriptor (VIRTIO_BLK_F_SIZE_MAX), 0 when not offered.
    pub size_max: u32,
    /// The used length is trusted (VIRTIO_F_VERSION_1). A legacy device's
    /// used lengths are ignored (section 5.2.6.3).
    pub exact_used_len: bool,
}

impl Limits {
    /// Data descriptors one chain may carry.
    pub fn segment_cap(self) -> usize {
        if self.seg_max == 0 { MAX_SEGMENTS } else { (self.seg_max as usize).min(MAX_SEGMENTS) }
    }

    /// Bytes one data descriptor may carry, in whole sectors.
    pub fn descriptor_cap(self) -> usize {
        if self.size_max == 0 { usize::MAX } else { self.size_max as usize / SECTOR_BYTES * SECTOR_BYTES }
    }

    /// Data descriptors and bytes a chain of `frames` frames carries when no
    /// two of its frames are adjacent.
    pub fn chain_shape(self, frames: usize) -> (usize, usize) {
        let cap = self.descriptor_cap();
        if cap == 0 {
            return (0, 0);
        }
        let per_frame = FRAME_BYTES.div_ceil(cap.min(FRAME_BYTES));
        let mut descriptors = 0;
        let mut bytes = 0;
        for _ in 0..frames {
            let room = self.segment_cap() - descriptors;
            if room == 0 {
                break;
            }
            let pieces = per_frame.min(room);
            descriptors += pieces;
            bytes += (pieces * cap).min(FRAME_BYTES);
        }
        (descriptors, bytes)
    }

    /// Chain slots of `frames` frames each that fit together in a ring of `size`.
    pub fn chains_for(self, size: u16, frames: usize) -> usize {
        let (descriptors, _) = self.chain_shape(frames);
        if descriptors == 0 {
            return 0;
        }
        (size as usize / (descriptors + 2)).min(MAX_CHAINS)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetupError {
    QueueSize,
    NoChains,
    SegmentLimit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubmitError {
    /// The queue stopped after an abandoned request or a broken used ring.
    Stopped,
    /// The slot was not reserved.
    NotReserved,
    /// The length is not whole sectors, or more than the slot carries.
    Length,
    /// Not enough free descriptors.
    Full,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestError {
    /// The device has not returned the chain.
    Pending,
    /// The chain was on the device when the queue stopped; its memory stays
    /// with the device.
    Lost,
    /// The device reported an I/O error.
    Media,
    /// The device does not support the request.
    Unsupported,
    /// The status byte held no status the device defines.
    InvalidStatus(u8),
    /// The used length is not the bytes the request makes the device write.
    UsedLength(u32),
    /// The slot holds no returned request, or the read buffer has the wrong length.
    NotIssued,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Slot {
    Free,
    Reserved,
    Issued { head: u16, kind: u32, data_len: usize },
    Done { kind: u32, data_len: usize, status: u8, used_len: u32 },
    Lost,
}

/// Outcome of one `drain`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Drain {
    /// `n` chains came back (0 when the device returned no new chain).
    Completed(usize),
    /// The used ring named a chain the driver did not issue, or ran past the
    /// chains outstanding: the queue has stopped.
    Broken,
    /// The queue had already stopped; the used ring was not read.
    Stopped,
}

pub struct BlkQueue<R> {
    ring: R,
    size: u16,
    limits: Limits,
    storage: [ChainStorage; MAX_CHAINS],
    slot_bytes: [usize; MAX_CHAINS],
    slots: [Slot; MAX_CHAINS],
    chains: usize,
    /// Driver-private descriptor free list and chain links.
    next_free: [u16; MAX_QUEUE_SIZE as usize],
    links: [u16; MAX_QUEUE_SIZE as usize],
    chain_len: [u16; MAX_QUEUE_SIZE as usize],
    head_slot: [u8; MAX_QUEUE_SIZE as usize],
    free_head: u16,
    free_count: u16,
    avail_idx: u16,
    published_idx: u16,
    last_used: u16,
    outstanding: u16,
    stopped: bool,
}

impl<R: RingMemory> BlkQueue<R> {
    /// A queue of `size` descriptors (a power of two up to MAX_QUEUE_SIZE)
    /// whose rings were cleared, with one slot per entry of `storage`. The
    /// slots, each at its largest chain, must fit in the ring together.
    pub fn new(ring: R, size: u16, limits: Limits, storage: &[ChainStorage]) -> Result<Self, SetupError> {
        if size == 0 || size > MAX_QUEUE_SIZE || !size.is_power_of_two() {
            return Err(SetupError::QueueSize);
        }
        if storage.is_empty() || storage.len() > MAX_CHAINS {
            return Err(SetupError::NoChains);
        }
        let mut queue = Self {
            ring,
            size,
            limits,
            storage: [ChainStorage::EMPTY; MAX_CHAINS],
            slot_bytes: [0; MAX_CHAINS],
            slots: [Slot::Free; MAX_CHAINS],
            chains: storage.len(),
            next_free: [0; MAX_QUEUE_SIZE as usize],
            links: [0; MAX_QUEUE_SIZE as usize],
            chain_len: [0; MAX_QUEUE_SIZE as usize],
            head_slot: [NO_SLOT; MAX_QUEUE_SIZE as usize],
            free_head: 0,
            free_count: size,
            avail_idx: 0,
            published_idx: 0,
            last_used: 0,
            outstanding: 0,
            stopped: false,
        };
        let mut descriptors = 0usize;
        for (index, chain) in storage.iter().enumerate() {
            if chain.frame_count == 0 || chain.frame_count > MAX_SEGMENTS {
                return Err(SetupError::NoChains);
            }
            let (data_descriptors, bytes) = limits.chain_shape(chain.frame_count);
            if bytes < SECTOR_BYTES {
                return Err(SetupError::SegmentLimit);
            }
            queue.storage[index] = *chain;
            queue.slot_bytes[index] = bytes;
            descriptors += data_descriptors + 2;
        }
        if descriptors > size as usize {
            return Err(SetupError::SegmentLimit);
        }
        for index in 0..size {
            queue.next_free[index as usize] = index.wrapping_add(1);
        }
        Ok(queue)
    }

    pub fn size(&self) -> u16 {
        self.size
    }

    pub fn chains(&self) -> usize {
        self.chains
    }

    /// Data bytes each chain slot carries (the smallest slot's).
    pub fn chain_bytes(&self) -> usize {
        self.slot_bytes[..self.chains].iter().copied().min().unwrap_or(0)
    }

    pub fn free_descriptors(&self) -> u16 {
        self.free_count
    }

    pub fn outstanding(&self) -> u16 {
        self.outstanding
    }

    pub fn is_stopped(&self) -> bool {
        self.stopped
    }

    pub fn last_used(&self) -> u16 {
        self.last_used
    }

    pub fn published_idx(&self) -> u16 {
        self.published_idx
    }

    /// `used.idx` as the device holds it now, for failure reports.
    pub fn device_used_idx(&mut self) -> u16 {
        self.ring.read_used_idx()
    }

    /// Head descriptor of a slot the device holds.
    pub fn slot_head(&self, slot: usize) -> Option<u16> {
        match self.slots.get(slot) {
            Some(Slot::Issued { head, .. }) => Some(*head),
            _ => None,
        }
    }

    /// Whether `slot` still waits for the device.
    pub fn is_issued(&self, slot: usize) -> bool {
        matches!(self.slots.get(slot), Some(Slot::Issued { .. }))
    }

    /// Take a free chain slot.
    pub fn reserve(&mut self) -> Option<usize> {
        if self.stopped {
            return None;
        }
        let slot = (0..self.chains).find(|&i| self.slots[i] == Slot::Free)?;
        self.slots[slot] = Slot::Reserved;
        Some(slot)
    }

    /// Give back a reserved slot that was not submitted.
    pub fn unreserve(&mut self, slot: usize) {
        if self.slots.get(slot) == Some(&Slot::Reserved) {
            self.slots[slot] = Slot::Free;
        }
    }

    /// Slots a caller could reserve now (0 once stopped).
    pub fn free_slots(&self) -> usize {
        if self.stopped {
            0
        } else {
            self.slots[..self.chains].iter().filter(|slot| **slot == Slot::Free).count()
        }
    }

    /// Build a reserved slot's chain and its available-ring entry: `kind` is a
    /// `request_type`, with `len` data bytes at `sector` (no data for a flush). A
    /// write's bytes are copied into the slot's frames first. The device sees
    /// the chain only after `publish`.
    pub fn submit(
        &mut self,
        dma: &mut impl DmaMemory,
        slot: usize,
        kind: u32,
        sector: u64,
        write_data: Option<&[u8]>,
        len: usize,
    ) -> Result<u16, SubmitError> {
        if self.stopped {
            return Err(SubmitError::Stopped);
        }
        if self.slots.get(slot) != Some(&Slot::Reserved) {
            return Err(SubmitError::NotReserved);
        }
        let flush = kind == request_type::FLUSH;
        if flush != (len == 0) || len % SECTOR_BYTES != 0 || len > self.slot_bytes[slot] {
            return Err(SubmitError::Length);
        }
        if (kind == request_type::OUT) != write_data.is_some() || write_data.is_some_and(|data| data.len() != len) {
            return Err(SubmitError::Length);
        }
        let storage = self.storage[slot];
        let mut data_descriptors = 0usize;
        self.for_each_segment(&storage, len, |_, _| data_descriptors += 1);
        if data_descriptors > self.limits.segment_cap() {
            return Err(SubmitError::Length);
        }
        let needed = data_descriptors as u16 + 2;
        if needed > self.free_count {
            return Err(SubmitError::Full);
        }

        // The buffers are ready before any descriptor names them.
        let mut header = [0u8; HEADER_BYTES as usize];
        header[0..4].copy_from_slice(&kind.to_le_bytes());
        header[8..16].copy_from_slice(&if flush { 0 } else { sector }.to_le_bytes());
        dma.write_bytes(storage.header, &header);
        dma.write_bytes(storage.status, &[STATUS_UNWRITTEN]);
        if let Some(data) = write_data {
            for (chunk, frame) in data.chunks(FRAME_BYTES).zip(storage.frames.iter()) {
                dma.write_bytes(*frame, chunk);
            }
        }

        let head = self.take_desc();
        let data_flags = if kind == request_type::IN { DESC_F_WRITE } else { 0 };
        let mut current = head;
        let mut emit = |queue: &mut Self, addr: u64, bytes: u32, flags: u16, last: bool| {
            let next = if last { 0 } else { queue.take_desc() };
            queue.links[current as usize] = next;
            queue.ring.write_desc(current, Desc { addr, len: bytes, flags: flags | if last { 0 } else { DESC_F_NEXT }, next });
            current = next;
        };
        emit(self, storage.header, HEADER_BYTES, 0, false);
        let mut segments = [(0u64, 0u32); MAX_SEGMENTS];
        let mut count = 0;
        self.for_each_segment(&storage, len, |addr, bytes| {
            segments[count] = (addr, bytes);
            count += 1;
        });
        for &(addr, bytes) in &segments[..count] {
            emit(self, addr, bytes, data_flags, false);
        }
        emit(self, storage.status, 1, DESC_F_WRITE, true);

        self.chain_len[head as usize] = needed;
        self.head_slot[head as usize] = slot as u8;
        self.slots[slot] = Slot::Issued { head, kind, data_len: len };
        self.ring.write_avail_entry(self.avail_idx % self.size, head);
        self.avail_idx = self.avail_idx.wrapping_add(1);
        self.outstanding += 1;
        Ok(head)
    }

    /// Make the submitted chains visible with one index store. Returns
    /// whether the device needs a notification.
    pub fn publish(&mut self) -> bool {
        if self.avail_idx == self.published_idx {
            return false;
        }
        self.ring.barrier_before_avail_idx();
        self.ring.write_avail_idx(self.avail_idx);
        self.published_idx = self.avail_idx;
        self.ring.barrier_before_notify();
        true
    }

    /// Read the chains the device has returned. `completed(slot)` runs for
    /// each one once its status byte was captured.
    pub fn drain(&mut self, dma: &mut impl DmaMemory, mut completed: impl FnMut(usize)) -> Drain {
        if self.stopped {
            return Drain::Stopped;
        }
        let used_idx = self.ring.read_used_idx();
        let published = self.published_idx.wrapping_sub(self.last_used);
        if used_idx.wrapping_sub(self.last_used) > published.min(self.outstanding) {
            self.stop();
            return Drain::Broken;
        }
        let mut count = 0;
        while self.last_used != used_idx {
            let (id, used_len) = self.ring.read_used_elem(self.last_used % self.size);
            let slot = if id < u32::from(self.size) { self.head_slot[id as usize] } else { NO_SLOT };
            let issued = match self.slots.get(slot as usize) {
                Some(&Slot::Issued { head, kind, data_len }) if u32::from(head) == id => Some((kind, data_len)),
                _ => None,
            };
            let Some((kind, data_len)) = issued else {
                self.stop();
                return Drain::Broken;
            };
            self.last_used = self.last_used.wrapping_add(1);
            let mut status = [STATUS_UNWRITTEN];
            dma.read_bytes(self.storage[slot as usize].status, &mut status);
            self.release_chain(id as u16);
            self.outstanding -= 1;
            self.slots[slot as usize] = Slot::Done { kind, data_len, status: status[0], used_len };
            count += 1;
            completed(slot as usize);
        }
        Drain::Completed(count)
    }

    /// Take a returned chain's result and free its slot. A read's bytes are
    /// copied into `read_out` only when the request succeeded.
    pub fn finish(&mut self, dma: &mut impl DmaMemory, slot: usize, read_out: Option<&mut [u8]>) -> Result<(), RequestError> {
        let (kind, data_len, status, used_len) = match self.slots.get(slot) {
            Some(&Slot::Done { kind, data_len, status, used_len }) => (kind, data_len, status, used_len),
            Some(Slot::Issued { .. }) => return Err(RequestError::Pending),
            Some(Slot::Lost) => return Err(RequestError::Lost),
            _ => return Err(RequestError::NotIssued),
        };
        self.slots[slot] = Slot::Free;
        match status {
            status::OK => {}
            status::IOERR => return Err(RequestError::Media),
            status::UNSUPP => return Err(RequestError::Unsupported),
            other => return Err(RequestError::InvalidStatus(other)),
        }
        let expected = 1 + if kind == request_type::IN { data_len as u32 } else { 0 };
        if self.limits.exact_used_len && used_len != expected {
            return Err(RequestError::UsedLength(used_len));
        }
        if kind == request_type::IN {
            let out = read_out.filter(|out| out.len() == data_len).ok_or(RequestError::NotIssued)?;
            let storage = self.storage[slot];
            for (chunk, frame) in out.chunks_mut(FRAME_BYTES).zip(storage.frames.iter()) {
                dma.read_bytes(*frame, chunk);
            }
        }
        Ok(())
    }

    /// Stop the queue with requests still on the device: after an abandoned
    /// wait, or a device that stopped answering. Each chain the device holds
    /// is lost with its memory, no slot is reserved again, and later used
    /// entries are ignored until the device is reset and the queue rebuilt.
    pub fn stop(&mut self) {
        self.stopped = true;
        for slot in &mut self.slots[..self.chains] {
            if matches!(slot, Slot::Issued { .. }) {
                *slot = Slot::Lost;
            }
        }
    }

    /// The data descriptors for the first `len` bytes of a slot's frames:
    /// physically adjacent frames share a descriptor up to size_max, and a
    /// frame larger than size_max is split.
    fn for_each_segment(&self, storage: &ChainStorage, len: usize, mut emit: impl FnMut(u64, u32)) {
        let cap = self.limits.descriptor_cap();
        let mut pending: Option<(u64, usize)> = None;
        let mut left = len;
        for &frame in &storage.frames[..storage.frame_count] {
            let mut offset = 0;
            while left > 0 && offset < FRAME_BYTES {
                let addr = frame + offset as u64;
                let mut take = left.min(FRAME_BYTES - offset);
                if let Some((start, bytes)) = pending {
                    if start + bytes as u64 == addr && bytes < cap {
                        let grow = take.min(cap - bytes);
                        pending = Some((start, bytes + grow));
                        offset += grow;
                        left -= grow;
                        continue;
                    }
                    emit(start, bytes as u32);
                }
                take = take.min(cap);
                pending = Some((addr, take));
                offset += take;
                left -= take;
            }
            if left == 0 {
                break;
            }
        }
        if let Some((start, bytes)) = pending {
            emit(start, bytes as u32);
        }
    }

    fn take_desc(&mut self) -> u16 {
        let index = self.free_head;
        self.free_head = self.next_free[index as usize];
        self.free_count -= 1;
        index
    }

    /// Return a chain's descriptors, following the links the driver wrote.
    fn release_chain(&mut self, head: u16) {
        let mut index = head;
        for _ in 0..self.chain_len[head as usize] {
            let next = self.links[index as usize];
            self.next_free[index as usize] = self.free_head;
            self.free_head = index;
            self.free_count += 1;
            index = next;
        }
        self.chain_len[head as usize] = 0;
        self.head_slot[head as usize] = NO_SLOT;
    }
}

/// Requests admitted to the device, by sector range. Overlapping ranges wait
/// for each other when either writes; a flush is a write of the whole device.
pub struct RangeTable<const N: usize> {
    entries: [Option<(u64, u64, bool)>; N],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Admission {
    Admitted(usize),
    /// An admitted request conflicts, or the table is full: wait for a release.
    Wait,
}

impl<const N: usize> RangeTable<N> {
    pub const fn new() -> Self {
        Self { entries: [None; N] }
    }

    /// Admit the sectors `[start, end)`.
    pub fn try_admit(&mut self, start: u64, end: u64, write: bool) -> Admission {
        let conflict = self.entries.iter().flatten().any(|&(s, e, w)| (w || write) && s < end && start < e);
        if conflict {
            return Admission::Wait;
        }
        match self.entries.iter().position(Option::is_none) {
            Some(index) => {
                self.entries[index] = Some((start, end, write));
                Admission::Admitted(index)
            }
            None => Admission::Wait,
        }
    }

    pub fn release(&mut self, index: usize) {
        if let Some(entry) = self.entries.get_mut(index) {
            *entry = None;
        }
    }

    pub fn is_empty(&self) -> bool {
        self.entries.iter().all(Option::is_none)
    }
}

impl<const N: usize> Default for RangeTable<N> {
    fn default() -> Self {
        Self::new()
    }
}

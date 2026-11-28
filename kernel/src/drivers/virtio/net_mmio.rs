//! VirtIO network device driver for ARM64, on the MMIO transport (virtio 1.2
//! sections 2.7, 4.2 and 5.1), legacy (version 1) and modern (version 2).
//!
//! Each queue pair is a receive queue and a transmit queue of up to 1024
//! entries, sized from the device's QueueNumMax and the driver's DMA budget,
//! each behind its own lock (`RXQ`, `TXQ`). Their rings and copy buffers are
//! frames allocated at `init`, held for the device's lifetime. With
//! VIRTIO_NET_F_CTRL_VQ and VIRTIO_NET_F_MQ the driver asks for one pair per
//! CPU through the control queue, and keeps one pair if the device refuses.
//! A transmitted frame goes to the queue its IPv4 flow hashes to, so a flow
//! keeps its order; receive drains the active queues in turn.
//!
//! Negotiated when offered: MRG_RXBUF (a frame over several receive buffers),
//! EVENT_IDX (notification suppression by index), CSUM and HOST_TSO4
//! (`transmit_offload`), MTU, and on the legacy interface ANY_LAYOUT. Without
//! ANY_LAYOUT a legacy device gets the header in a descriptor of its own
//! (section 5.1.6.6). GUEST_CSUM and the guest segmentation features are not
//! negotiated: the stack checks received checksums itself.
//!
//! Transmit completions are taken back on the transmit path, and from the
//! network softirq after an interrupt. While a transmit queue has room the
//! device is asked not to interrupt for completions; when it fills, the queue
//! asks for one (`net_tx_queue`), so a full queue becomes available again
//! without receive traffic.
//!
//! The device's addresses and what was negotiated are written once, at the end
//! of `init`, into `DEVICE`.

use super::mmio::{device_id, VirtioMmioDevice, VIRTIO_MMIO_BASE, VIRTIO_MMIO_COUNT, VIRTIO_MMIO_SIZE};
use super::net_tx_queue::{need_event, split_queue_size, TxError, TxQueue, TxRing, MAX_CHAIN, USED_F_NO_NOTIFY};
use crate::irq_safe_mutex::IrqSafeMutex;
use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{AtomicUsize, Ordering};

/// Feature bits (virtio 1.2 sections 5.1.3 and 6).
pub mod features {
    pub const CSUM: u64 = 1 << 0;
    pub const MTU: u64 = 1 << 3;
    pub const MAC: u64 = 1 << 5;
    pub const HOST_TSO4: u64 = 1 << 11;
    pub const MRG_RXBUF: u64 = 1 << 15;
    pub const STATUS: u64 = 1 << 16;
    pub const CTRL_VQ: u64 = 1 << 17;
    pub const MQ: u64 = 1 << 22;
    pub const ANY_LAYOUT: u64 = 1 << 27;
    pub const EVENT_IDX: u64 = 1 << 29;
    pub const VERSION_1: u64 = 1 << 32;
}

/// virtio_net_hdr flags and GSO types (section 5.1.6).
const HDR_F_NEEDS_CSUM: u8 = 1;
const GSO_NONE: u8 = 0;
const GSO_TCPV4: u8 = 1;

/// The largest Ethernet frame without a negotiated MTU (section 5.1.6.3).
pub const MAX_PACKET_SIZE: usize = 1514;
const ETHERNET_HEADER: usize = 14;
/// The largest frame a TCPv4 segmentation request can describe: an IPv4
/// packet of 65535 bytes behind the Ethernet header.
const MAX_TSO_FRAME: usize = ETHERNET_HEADER + 65535;

/// Descriptor flags (section 2.7.5).
const DESC_F_NEXT: u16 = 1;
const DESC_F_WRITE: u16 = 2;

/// Queue pairs at most, one per CPU the kernel supports.
const MAX_PAIRS: usize = 8;
/// Largest queue the driver sets up.
const MAX_DEPTH: u16 = 1024;
/// Smallest queue the driver shrinks to when memory is short.
const MIN_DEPTH: u16 = 16;
/// Pages of rings, copy buffers and staging the driver may hold.
const DMA_BUDGET_PAGES: usize = 2048;
/// One copy buffer per descriptor: half a page.
const BUF: usize = 2048;
const PAGE: usize = 4096;
/// Copy buffers come in runs of this many contiguous pages.
const CHUNK_PAGES: usize = 16;
const BUFS_PER_CHUNK: usize = CHUNK_PAGES * PAGE / BUF;
const MAX_CHUNKS: usize = MAX_DEPTH as usize / BUFS_PER_CHUNK;
const POSTED_WORDS: usize = MAX_DEPTH as usize / 64;
/// The control queue's depth: one command is outstanding at a time.
const CTRL_DEPTH: u16 = 8;
/// How long `init` waits for a control command, in polls of the used ring.
const CTRL_POLLS: u32 = 10_000_000;

/// Base IRQ for VirtIO MMIO devices on the QEMU virt machine:
/// IRQ = VIRTIO_IRQ_BASE + slot number.
const VIRTIO_IRQ_BASE: u32 = 48;

/// VirtIO MMIO register offsets used without a probe.
const REG_QUEUE_NOTIFY: u64 = 0x050;
const REG_INTERRUPT_STATUS: u64 = 0x060;
const REG_INTERRUPT_ACK: u64 = 0x064;

#[inline(always)]
fn virt_to_phys(addr: u64) -> u64 {
    addr - crate::memory::physical_memory_offset().as_u64()
}

/// Barriers for memory the device reads and writes. The device is an observer
/// in the outer-shareable domain.
mod barrier {
    /// Stores before it reach the device before stores after it.
    #[inline(always)]
    pub fn write() {
        unsafe { core::arch::asm!("dmb oshst", options(nostack, preserves_flags)) }
    }
    /// Loads before it complete before loads after it.
    #[inline(always)]
    pub fn read() {
        unsafe { core::arch::asm!("dmb oshld", options(nostack, preserves_flags)) }
    }
    /// Accesses before it complete before those after it start, device-register
    /// stores included: an index store before the read of the other side's
    /// suppression, and before the QueueNotify store.
    #[inline(always)]
    pub fn full() {
        unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)) }
    }
}

/// A split virtqueue's memory (section 2.7): the descriptor table, the
/// available ring with `used_event`, padding to the next page, then the used
/// ring with `avail_event`, as the legacy layout of section 2.7.2 places them.
/// The modern interface takes the same parts by address. Addresses are
/// direct-map virtual addresses.
#[derive(Clone, Copy)]
struct Ring {
    desc: u64,
    avail: u64,
    used: u64,
    size: u16,
}

impl Ring {
    const EMPTY: Ring = Ring { desc: 0, avail: 0, used: 0, size: 0 };

    /// (offset of the available ring, offset of the used ring, total bytes)
    /// for `size` entries, with the used ring aligned to `align`.
    const fn layout(size: u16, align: usize) -> (usize, usize, usize) {
        let n = size as usize;
        let avail = 16 * n;
        let used = (avail + 2 * (3 + n)).next_multiple_of(align);
        let total = (used + 2 * 3 + 8 * n).next_multiple_of(PAGE);
        (avail, used, total)
    }

    fn pages(size: u16) -> usize {
        Self::layout(size, PAGE).2 / PAGE
    }

    fn at(base: u64, size: u16) -> Ring {
        let (avail, used, _) = Self::layout(size, PAGE);
        Ring { desc: base, avail: base + avail as u64, used: base + used as u64, size }
    }

    fn write_desc(&self, slot: u16, addr: u64, len: u32, flags: u16, next: u16) {
        let d = self.desc + 16 * u64::from(slot);
        // SAFETY: `slot` is below `size`; the table is this ring's memory.
        unsafe {
            write_volatile(d as *mut u64, addr);
            write_volatile((d + 8) as *mut u32, len);
            write_volatile((d + 12) as *mut u16, flags);
            write_volatile((d + 14) as *mut u16, next);
        }
    }

    fn write_avail_entry(&self, pos: u16, head: u16) {
        unsafe { write_volatile((self.avail + 4 + 2 * u64::from(pos)) as *mut u16, head) }
    }

    fn write_avail_idx(&self, idx: u16) {
        unsafe { write_volatile((self.avail + 2) as *mut u16, idx) }
    }

    fn write_avail_flags(&self, flags: u16) {
        unsafe { write_volatile(self.avail as *mut u16, flags) }
    }

    fn write_used_event(&self, idx: u16) {
        unsafe { write_volatile((self.avail + 4 + 2 * u64::from(self.size)) as *mut u16, idx) }
    }

    /// `used.idx`, ordered before the reads of the entries and buffers it covers.
    fn read_used_idx(&self) -> u16 {
        let idx = unsafe { read_volatile((self.used + 2) as *const u16) };
        barrier::read();
        idx
    }

    fn read_used_elem(&self, pos: u16) -> (u32, u32) {
        let e = self.used + 4 + 8 * u64::from(pos);
        unsafe { (read_volatile(e as *const u32), read_volatile((e + 4) as *const u32)) }
    }

    fn read_used_flags(&self) -> u16 {
        unsafe { read_volatile(self.used as *const u16) }
    }

    fn read_avail_event(&self) -> u16 {
        unsafe { read_volatile((self.used + 4 + 8 * u64::from(self.size)) as *const u16) }
    }
}

/// Copy buffers, `BUF` bytes each, in runs of `CHUNK_PAGES` contiguous pages.
#[derive(Clone, Copy)]
struct Pool {
    chunks: [u64; MAX_CHUNKS],
}

impl Pool {
    const EMPTY: Pool = Pool { chunks: [0; MAX_CHUNKS] };

    fn pages(buffers: u16) -> usize {
        usize::from(buffers).div_ceil(BUFS_PER_CHUNK) * CHUNK_PAGES
    }

    fn alloc(buffers: u16) -> Option<Pool> {
        let mut pool = Pool::EMPTY;
        for chunk in pool.chunks.iter_mut().take(usize::from(buffers).div_ceil(BUFS_PER_CHUNK)) {
            *chunk = alloc_pages(CHUNK_PAGES, false)?;
        }
        Some(pool)
    }

    /// The direct-map address of buffer `i`.
    fn addr(&self, i: u16) -> u64 {
        let i = usize::from(i);
        self.chunks[i / BUFS_PER_CHUNK] + ((i % BUFS_PER_CHUNK) * BUF) as u64
    }
}

/// `pages` contiguous frames as a direct-map address, filled with 0 bytes when `clear`.
/// Kept for the device's lifetime.
fn alloc_pages(pages: usize, clear: bool) -> Option<u64> {
    let mut frames = [None; CHUNK_PAGES * 2];
    if pages > frames.len() {
        return None;
    }
    let base = crate::memory::frame_allocator::allocate_contiguous_frames(pages, &mut frames)?;
    let virt = base.start_address().as_u64() + crate::memory::physical_memory_offset().as_u64();
    if clear {
        // SAFETY: the frames were just claimed for this driver.
        unsafe { core::ptr::write_bytes(virt as *mut u8, 0, pages * PAGE) };
    }
    Some(virt)
}

/// What the driver and device agreed (section 5.1.6).
#[derive(Clone, Copy)]
struct Negotiated {
    features: u64,
    /// virtio_net_hdr bytes before each frame: 12 with VERSION_1 or
    /// MRG_RXBUF, else the legacy 10 (section 5.1.6.1).
    hdr_len: usize,
    /// A legacy device without ANY_LAYOUT: the header has its own descriptor.
    separate_header: bool,
    /// The largest frame received or sent: 1514, or the MTU plus the header.
    max_frame: usize,
}

impl Negotiated {
    fn has(&self, bit: u64) -> bool {
        self.features & bit != 0
    }
}

/// Choose the features to accept from those `offered` by a device of MMIO
/// `version`, whose `mtu` field (read only when MTU is offered) is given.
/// A modern device must offer VERSION_1 (section 6.1).
fn negotiate(offered: u64, version: u32, mtu: u16) -> Result<Negotiated, &'static str> {
    use features::*;
    let modern = version != 1;
    if modern && offered & VERSION_1 == 0 {
        return Err("modern virtio-net device without VERSION_1");
    }
    let mut want = MAC | STATUS | EVENT_IDX | CSUM;
    if offered & CSUM != 0 {
        want |= HOST_TSO4;
    }
    if offered & (CTRL_VQ | MQ) == CTRL_VQ | MQ {
        want |= CTRL_VQ | MQ;
    }
    let any_layout = modern || offered & ANY_LAYOUT != 0;
    if modern {
        want |= VERSION_1;
    } else {
        want |= ANY_LAYOUT;
    }
    // Merged buffers carry the header in the first buffer of a frame, which a
    // legacy device without ANY_LAYOUT cannot be given.
    let merge = any_layout && offered & MRG_RXBUF != 0;
    if merge {
        want |= MRG_RXBUF;
    }
    let hdr_len = if modern || merge { 12 } else { 10 };
    let mut max_frame = MAX_PACKET_SIZE;
    // The stack sends 1500-byte packets, so a smaller MTU is not taken; a
    // larger one must fit the receive buffers or merge across them.
    let mtu_frame = usize::from(mtu) + ETHERNET_HEADER;
    if offered & MTU != 0 && mtu >= 1500 && (merge || hdr_len + mtu_frame <= BUF) {
        want |= MTU;
        max_frame = mtu_frame;
    }
    Ok(Negotiated {
        features: offered & want,
        hdr_len,
        separate_header: !any_layout,
        max_frame,
    })
}

/// The device, written once when `init` has finished.
struct DeviceInfo {
    /// Physical base of the MMIO window.
    base: u64,
    /// Its HHDM address, for register access without a probe.
    base_virt: u64,
    /// The MMIO slot, which fixes the interrupt.
    slot: usize,
    mac: [u8; 6],
    neg: Negotiated,
    /// Queue pairs in use: queue `2k` receives and `2k + 1` transmits.
    pairs: usize,
}

static DEVICE: spin::Once<DeviceInfo> = spin::Once::new();

/// A transmit queue's memory, as `TxQueue` publishes to it.
struct MmioTxRing(Ring);

impl TxRing for MmioTxRing {
    fn write_desc(&mut self, slot: u16, addr: u64, len: u32) {
        self.0.write_desc(slot, addr, len, 0, 0);
    }
    fn write_desc_next(&mut self, slot: u16, addr: u64, len: u32, next: u16) {
        self.0.write_desc(slot, addr, len, DESC_F_NEXT, next);
    }
    fn write_avail_entry(&mut self, pos: u16, slot: u16) {
        self.0.write_avail_entry(pos, slot);
    }
    fn barrier_before_avail_idx(&mut self) {
        barrier::write();
    }
    fn write_avail_idx(&mut self, idx: u16) {
        self.0.write_avail_idx(idx);
    }
    fn barrier_before_notify(&mut self) {
        barrier::full();
    }
    fn read_used_idx(&mut self) -> u16 {
        self.0.read_used_idx()
    }
    fn read_used_id(&mut self, pos: u16) -> u32 {
        self.0.read_used_elem(pos).0
    }
    fn read_avail_event(&mut self) -> u16 {
        self.0.read_avail_event()
    }
    fn read_used_flags(&mut self) -> u16 {
        self.0.read_used_flags()
    }
    fn write_used_event(&mut self, idx: u16) {
        self.0.write_used_event(idx);
    }
    fn write_avail_flags(&mut self, flags: u16) {
        self.0.write_avail_flags(flags);
    }
    fn full_barrier(&mut self) {
        barrier::full();
    }
}

/// One transmit queue: its descriptors and the copy buffer behind each.
struct TxState {
    queue: TxQueue<MmioTxRing>,
    pool: Pool,
}

/// One receive queue (section 5.1.6.3 and 5.1.6.4). Buffer `b` is `BUF` bytes
/// at `pool.addr(b)`, offered as descriptor `b`, or as descriptors `2b` (the
/// header) and `2b + 1` (the frame) when the header needs its own.
struct RxState {
    ring: Ring,
    pool: Pool,
    /// Descriptors per buffer: 1, or 2 with a separate header.
    descs_per_buf: u16,
    buffers: u16,
    /// The driver's copy of `avail.idx`, and the last value stored.
    avail_idx: u16,
    published: u16,
    last_used: u16,
    /// Bit `b` is set while the device holds buffer `b`.
    posted: [u64; POSTED_WORDS],
    /// Buffers returned by the device and not yet offered again: those of
    /// frames handed up, and of completions dropped.
    held: [u16; MAX_DEPTH as usize],
    held_len: usize,
    /// Where a frame merged from several buffers is assembled.
    staging: u64,
    neg: Negotiated,
    /// Completions dropped as malformed.
    dropped: u64,
}

impl RxState {
    const EMPTY: RxState = RxState {
        ring: Ring::EMPTY,
        pool: Pool::EMPTY,
        descs_per_buf: 1,
        buffers: 0,
        avail_idx: 0,
        published: 0,
        last_used: 0,
        posted: [0; POSTED_WORDS],
        held: [0; MAX_DEPTH as usize],
        held_len: 0,
        staging: 0,
        neg: Negotiated { features: 0, hdr_len: 10, separate_header: false, max_frame: MAX_PACKET_SIZE },
        dropped: 0,
    };

    fn is_posted(&self, b: u16) -> bool {
        self.posted[usize::from(b) / 64] & (1 << (b % 64)) != 0
    }

    fn set_posted(&mut self, b: u16, on: bool) {
        let w = &mut self.posted[usize::from(b) / 64];
        if on {
            *w |= 1 << (b % 64);
        } else {
            *w &= !(1 << (b % 64));
        }
    }

    /// Write buffer `b`'s descriptors.
    fn write_buffer_descs(&self, b: u16) {
        let phys = virt_to_phys(self.pool.addr(b));
        if self.descs_per_buf == 1 {
            self.ring.write_desc(b, phys, BUF as u32, DESC_F_WRITE, 0);
        } else {
            let hdr = self.neg.hdr_len as u32;
            self.ring.write_desc(2 * b, phys, hdr, DESC_F_WRITE | DESC_F_NEXT, 2 * b + 1);
            self.ring.write_desc(2 * b + 1, phys + u64::from(hdr), BUF as u32 - hdr, DESC_F_WRITE, 0);
        }
    }

    /// Put buffer `b` on the available ring; `publish` makes it visible.
    fn offer(&mut self, b: u16) {
        self.ring
            .write_avail_entry(self.avail_idx % self.ring.size, b * self.descs_per_buf);
        self.avail_idx = self.avail_idx.wrapping_add(1);
        self.set_posted(b, true);
    }

    /// Store `avail.idx` for the buffers offered since the last store, and
    /// say whether the device asked to be notified (section 2.7.10.1).
    fn publish(&mut self) -> bool {
        if self.avail_idx == self.published {
            return false;
        }
        let old = self.published;
        barrier::write();
        self.ring.write_avail_idx(self.avail_idx);
        self.published = self.avail_idx;
        barrier::full();
        if self.neg.has(features::EVENT_IDX) {
            need_event(self.ring.read_avail_event(), self.avail_idx, old)
        } else {
            self.ring.read_used_flags() & USED_F_NO_NOTIFY == 0
        }
    }

    /// Give back the returned buffers. Returns whether to notify the device.
    fn recycle(&mut self) -> bool {
        for i in 0..self.held_len {
            let b = self.held[i];
            self.offer(b);
        }
        self.held_len = 0;
        self.publish()
    }

    fn hold(&mut self, b: u16) {
        self.set_posted(b, false);
        self.held[self.held_len] = b;
        self.held_len += 1;
    }

    /// The buffer a used entry names, if the device holds it.
    fn buffer_of(&self, id: u32) -> Option<u16> {
        let dpb = u32::from(self.descs_per_buf);
        let b = id / dpb;
        (id % dpb == 0 && b < u32::from(self.buffers) && self.is_posted(b as u16)).then_some(b as u16)
    }

    /// The next frame on this queue; or, once the used ring is empty and
    /// the device has been asked to interrupt for the next one, no frame. A frame stays
    /// valid until this queue's next `next_frame` or `recycle`.
    fn next_frame(&mut self) -> Option<&'static [u8]> {
        if self.buffers == 0 {
            return None;
        }
        let size = self.ring.size;
        loop {
            let mut used = self.ring.read_used_idx();
            if used == self.last_used {
                if !self.neg.has(features::EVENT_IDX) {
                    return None;
                }
                // Section 2.7.14: ask for an interrupt on the next buffer,
                // then look again, since a buffer used before the request was
                // seen raises no interrupt.
                self.ring.write_used_event(self.last_used);
                barrier::full();
                used = self.ring.read_used_idx();
                if used == self.last_used {
                    return None;
                }
            }
            let hdr = self.neg.hdr_len;
            let (id, len) = self.ring.read_used_elem(self.last_used % size);
            let Some(b) = self.buffer_of(id) else {
                // Not a buffer the device holds, so there is no buffer to give back.
                self.last_used = self.last_used.wrapping_add(1);
                self.dropped += 1;
                continue;
            };
            let len = len as usize;
            let base = self.pool.addr(b);
            let count = if self.neg.has(features::MRG_RXBUF) {
                unsafe { u16::from_le(read_volatile((base + 10) as *const u16)) }
            } else {
                1
            };
            let max_buffers = (hdr + self.neg.max_frame).div_ceil(BUF);
            if len < hdr || len > BUF || count == 0 || usize::from(count) > max_buffers {
                self.last_used = self.last_used.wrapping_add(1);
                self.hold(b);
                self.dropped += 1;
                continue;
            }
            if count == 1 {
                self.last_used = self.last_used.wrapping_add(1);
                self.hold(b);
                if len - hdr > self.neg.max_frame {
                    self.dropped += 1;
                    continue;
                }
                // SAFETY: the buffer is held until `recycle`.
                return Some(unsafe { core::slice::from_raw_parts((base + hdr as u64) as *const u8, len - hdr) });
            }
            // A merged frame: the `count` buffers the header names must be visible,
            // as the device publishes them together (section 5.1.6.4.1).
            if used.wrapping_sub(self.last_used) < count {
                return None;
            }
            if let Some(frame) = self.merge(count, hdr) {
                return Some(frame);
            }
        }
    }

    /// Take the `count` used entries of a merged frame. Each but the last must
    /// fill its buffer and the total must fit the largest frame; otherwise
    /// the frame is dropped, its buffers held for `recycle`.
    fn merge(&mut self, count: u16, hdr: usize) -> Option<&'static [u8]> {
        let size = self.ring.size;
        // Frame bytes assembled so far.
        let mut written = 0usize;
        let mut valid = self.staging != 0;
        for i in 0..count {
            let (id, len) = self.ring.read_used_elem(self.last_used.wrapping_add(i) % size);
            let len = len as usize;
            let Some(b) = self.buffer_of(id) else {
                // Not ours, so it is not held; the frame is lost.
                valid = false;
                continue;
            };
            self.hold(b);
            let last = i + 1 == count;
            // The first entry's length was checked to cover the header.
            let skip = if i == 0 { hdr } else { 0 };
            if len > BUF || (!last && len != BUF) || len <= skip || written + len - skip > self.neg.max_frame {
                valid = false;
            }
            if valid {
                // SAFETY: the buffer is held, and the staging area holds
                // `max_frame` bytes, which `written + len - skip` stays within.
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        (self.pool.addr(b) + skip as u64) as *const u8,
                        (self.staging + written as u64) as *mut u8,
                        len - skip,
                    )
                };
                written += len - skip;
            }
        }
        self.last_used = self.last_used.wrapping_add(count);
        if !valid {
            self.dropped += 1;
            return None;
        }
        Some(unsafe { core::slice::from_raw_parts(self.staging as *const u8, written) })
    }
}

/// The control queue (section 5.1.6.5): one command at a time, from `init`.
struct CtrlState {
    ring: Ring,
    /// A page: the class and command at 0, the data at 64, the ack at 128.
    buf: u64,
    avail_idx: u16,
    last_used: u16,
}

const RX_EMPTY: IrqSafeMutex<RxState> = IrqSafeMutex::new(RxState::EMPTY);
const TX_EMPTY: IrqSafeMutex<TxState> = IrqSafeMutex::new(TxState {
    queue: TxQueue::new(MmioTxRing(Ring::EMPTY)),
    pool: Pool::EMPTY,
});

/// Each receive queue, under its own lock.
static RXQ: [IrqSafeMutex<RxState>; MAX_PAIRS] = [RX_EMPTY; MAX_PAIRS];
/// Each transmit queue: descriptor ownership, the available index and the
/// used cursor, changed under that queue's lock (section 2.7.13).
static TXQ: [IrqSafeMutex<TxState>; MAX_PAIRS] = [TX_EMPTY; MAX_PAIRS];
/// The receive queue `receive` looks at first.
static RX_NEXT: AtomicUsize = AtomicUsize::new(0);

/// Write QueueNotify for `queue` through the HHDM mapping.
fn notify(base_virt: u64, queue: u32) {
    unsafe {
        crate::arch_impl::aarch64::mmio::write32((base_virt + REG_QUEUE_NOTIFY) as *mut u32, queue);
    }
}

/// Initialize the VirtIO network device
pub fn init() -> Result<(), &'static str> {
    crate::serial_println!("[virtio-net] Searching for network device...");

    for i in 0..VIRTIO_MMIO_COUNT {
        let base = VIRTIO_MMIO_BASE + (i as u64) * VIRTIO_MMIO_SIZE;
        if let Some(mut device) = VirtioMmioDevice::probe(base) {
            if device.device_id() == device_id::NETWORK {
                crate::serial_println!("[virtio-net] Found network device at {:#x} (slot {})", base, i);
                let (mac, neg, pairs) = init_device(&mut device)?;
                let irq = VIRTIO_IRQ_BASE + i as u32;
                crate::serial_println!("[virtio-net] Network device IRQ {} (will enable after net init)", irq);
                DEVICE.call_once(|| DeviceInfo {
                    base,
                    base_virt: crate::memory::physical_memory_offset().as_u64() + base,
                    slot: i,
                    mac,
                    neg,
                    pairs,
                });
                return Ok(());
            }
        }
    }

    Err("No VirtIO network device found")
}

/// A little-endian u16 of the device configuration (legacy: guest-endian,
/// which is little-endian on this kernel).
fn config_u16(device: &VirtioMmioDevice, offset: usize) -> u16 {
    u16::from_le_bytes([device.read_config_u8(offset), device.read_config_u8(offset + 1)])
}

/// A queue's size: the device's maximum, at most `MAX_DEPTH`, as a power of two.
fn queue_depth(device: &VirtioMmioDevice, queue: u32) -> u16 {
    device.select_queue(queue);
    let max = device.get_queue_num_max().min(u32::from(MAX_DEPTH)) as u16;
    split_queue_size(max, MAX_DEPTH)
}

/// Pages one queue pair of `depth` entries takes.
fn pair_pages(depth: u16, staging: usize) -> usize {
    2 * Ring::pages(depth) + 2 * Pool::pages(depth) + staging
}

/// Give the selected queue of `depth` entries at `ring` to the device.
fn place_queue(device: &VirtioMmioDevice, version: u32, queue: u32, ring: &Ring) {
    device.select_queue(queue);
    device.set_queue_num(u32::from(ring.size));
    if version == 1 {
        device.set_queue_align(PAGE as u32);
        device.set_queue_pfn((virt_to_phys(ring.desc) / PAGE as u64) as u32);
    } else {
        device.set_queue_desc(virt_to_phys(ring.desc));
        device.set_queue_avail(virt_to_phys(ring.avail));
        device.set_queue_used(virt_to_phys(ring.used));
        device.set_queue_ready(true);
    }
}

/// Allocate and place queue pair `k` at `depth`. False when memory ran out.
fn setup_pair(device: &VirtioMmioDevice, version: u32, k: usize, depth: u16, neg: &Negotiated, staging_pages: usize) -> bool {
    let rx_index = 2 * k as u32;
    let rx_depth = depth.min(queue_depth(device, rx_index));
    let tx_depth = depth.min(queue_depth(device, rx_index + 1));
    if rx_depth < MIN_DEPTH || tx_depth < MIN_DEPTH {
        return false;
    }
    let dpb = if neg.separate_header { 2 } else { 1 };
    let (Some(rx_ring), Some(tx_ring)) = (
        alloc_pages(Ring::pages(rx_depth), true),
        alloc_pages(Ring::pages(tx_depth), true),
    ) else {
        return false;
    };
    let (Some(rx_pool), Some(tx_pool)) = (Pool::alloc(rx_depth / dpb), Pool::alloc(tx_depth)) else {
        return false;
    };
    let staging = if staging_pages != 0 {
        match alloc_pages(staging_pages, false) {
            Some(s) => s,
            None => return false,
        }
    } else {
        0
    };

    let rx_ring = Ring::at(rx_ring, rx_depth);
    {
        let mut rx = RXQ[k].lock();
        *rx = RxState::EMPTY;
        rx.ring = rx_ring;
        rx.pool = rx_pool;
        rx.descs_per_buf = dpb;
        rx.buffers = rx_depth / dpb;
        rx.staging = staging;
        rx.neg = *neg;
    }
    place_queue(device, version, rx_index, &rx_ring);

    let tx_ring = Ring::at(tx_ring, tx_depth);
    {
        let mut tx = TXQ[k].lock();
        tx.queue = TxQueue::new(MmioTxRing(tx_ring));
        tx.queue.configure(tx_depth, tx_depth);
        tx.queue.start(neg.has(features::EVENT_IDX));
        tx.pool = tx_pool;
    }
    place_queue(device, version, rx_index + 1, &tx_ring);
    true
}

/// Offer the `buffers` receive buffers of pair `k`. Returns whether to notify.
fn fill_rx(k: usize) -> bool {
    let mut rx = RXQ[k].lock();
    for b in 0..rx.buffers {
        rx.write_buffer_descs(b);
        rx.offer(b);
    }
    rx.publish()
}

/// Send one control command and wait for the device's answer (section
/// 5.1.6.5): the class and command, then the data, both device-readable, then
/// the ack byte, device-writable, each in its own descriptor as the legacy
/// framing rules require. True when the device answered VIRTIO_NET_OK.
fn control(device: &VirtioMmioDevice, queue: u32, ctrl: &mut CtrlState, class: u8, command: u8, data: &[u8]) -> bool {
    let (cmd, payload, ack) = (ctrl.buf, ctrl.buf + 64, ctrl.buf + 128);
    unsafe {
        write_volatile(cmd as *mut u8, class);
        write_volatile((cmd + 1) as *mut u8, command);
        core::ptr::copy_nonoverlapping(data.as_ptr(), payload as *mut u8, data.len().min(64));
        write_volatile(ack as *mut u8, 0xff);
    }
    let ring = &ctrl.ring;
    ring.write_desc(0, virt_to_phys(cmd), 2, DESC_F_NEXT, 1);
    ring.write_desc(1, virt_to_phys(payload), data.len().min(64) as u32, DESC_F_NEXT, 2);
    ring.write_desc(2, virt_to_phys(ack), 1, DESC_F_WRITE, 0);
    ring.write_avail_entry(ctrl.avail_idx % ring.size, 0);
    barrier::write();
    ctrl.avail_idx = ctrl.avail_idx.wrapping_add(1);
    ring.write_avail_idx(ctrl.avail_idx);
    barrier::full();
    device.notify_queue(queue);
    for _ in 0..CTRL_POLLS {
        if ring.read_used_idx() != ctrl.last_used {
            ctrl.last_used = ctrl.last_used.wrapping_add(1);
            return unsafe { read_volatile(ack as *const u8) } == 0;
        }
        core::hint::spin_loop();
    }
    false
}

/// Bring the device up: returns its MAC address, what was negotiated, and the
/// queue pairs in use.
fn init_device(device: &mut VirtioMmioDevice) -> Result<([u8; 6], Negotiated, usize), &'static str> {
    use super::mmio::status;
    let version = device.version();
    crate::serial_println!("[virtio-net] Device version: {}", version);

    // Section 3.1.1: reset, then ACKNOWLEDGE and DRIVER. A legacy device takes
    // the guest page size after the reset, before any queue (section 4.2.4).
    device.reset();
    for _ in 0..1000 {
        if device.read_status() == 0 {
            break;
        }
        core::hint::spin_loop();
    }
    if version == 1 {
        device.set_guest_page_size(PAGE as u32);
    }
    device.write_status(status::ACKNOWLEDGE);
    device.write_status(status::ACKNOWLEDGE | status::DRIVER);

    let offered = device.read_device_features();
    // The configuration may be read before the features are accepted
    // (section 3.1.1 step 4); mtu is at byte 10 (section 5.1.4).
    let mtu = if offered & features::MTU != 0 { config_u16(device, 10) } else { 0 };
    let neg = match negotiate(offered, version, mtu) {
        Ok(neg) => neg,
        Err(e) => {
            device.write_status(status::FAILED);
            return Err(e);
        }
    };
    device.write_driver_features(neg.features);
    if version != 1 {
        device.write_status(status::ACKNOWLEDGE | status::DRIVER | status::FEATURES_OK);
        if device.read_status() & status::FEATURES_OK == 0 {
            device.write_status(status::FAILED);
            return Err("Device did not accept features");
        }
    }
    crate::serial_println!(
        "[virtio-net] Features offered {:#x}, accepted {:#x}; header {} bytes{}, largest frame {}",
        offered,
        neg.features,
        neg.hdr_len,
        if neg.separate_header { " in its own descriptor" } else { "" },
        neg.max_frame
    );

    let mac = if neg.has(features::MAC) {
        core::array::from_fn(|i| device.read_config_u8(i))
    } else {
        // A locally administered address (section 5.1.4.2).
        [0x02, 0x42, 0x52, 0x4e, 0x58, 0x01]
    };
    crate::serial_println!(
        "[virtio-net] MAC address: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
    );

    // Queue pairs: one per CPU the firmware reports, as many as the device
    // has, and as many as the DMA budget holds at the chosen depth.
    let multiqueue = neg.has(features::CTRL_VQ) && neg.has(features::MQ);
    let device_pairs = if neg.has(features::MQ) { config_u16(device, 8).max(1) } else { 1 };
    // Only a multiqueue device needs the CPU count, which on QEMU is a round of
    // PSCI queries.
    let want = if multiqueue {
        let cpus = crate::arch_impl::aarch64::smp::reported_cpus().0 as usize;
        cpus.clamp(1, crate::arch_impl::aarch64::smp::MAX_CPUS).min(usize::from(device_pairs)).min(MAX_PAIRS)
    } else {
        1
    };
    let staging_pages = if neg.has(features::MRG_RXBUF) && neg.hdr_len + neg.max_frame > BUF {
        neg.max_frame.div_ceil(PAGE)
    } else {
        0
    };
    let mut depth = queue_depth(device, 0).max(queue_depth(device, 1));
    while depth > MIN_DEPTH && pair_pages(depth, staging_pages) > DMA_BUDGET_PAGES {
        depth /= 2;
    }
    let mut pairs = want.min((DMA_BUDGET_PAGES / pair_pages(depth, staging_pages).max(1)).max(1));
    while !setup_pair(device, version, 0, depth, &neg, staging_pages) {
        if depth <= MIN_DEPTH {
            device.write_status(status::FAILED);
            return Err("No memory for the virtio-net queues");
        }
        depth /= 2;
    }
    for k in 1..pairs {
        if !setup_pair(device, version, k, depth, &neg, staging_pages) {
            pairs = k;
            break;
        }
    }
    // The control queue follows the device's queue pairs (section 5.1.2); it
    // is needed only to ask for more than one.
    let mut ctrl = None;
    if multiqueue && pairs > 1 {
        let index = 2 * u32::from(device_pairs);
        let ctrl_depth = CTRL_DEPTH.min(queue_depth(device, index));
        if ctrl_depth >= 4 {
            if let (Some(ring), Some(buf)) = (alloc_pages(Ring::pages(ctrl_depth), true), alloc_pages(1, true)) {
                let ring = Ring::at(ring, ctrl_depth);
                place_queue(device, version, index, &ring);
                ctrl = Some((index, CtrlState { ring, buf, avail_idx: 0, last_used: 0 }));
            }
        }
    }

    // Section 3.1.1 step 7: the receive queues are filled before DRIVER_OK,
    // and the device is told of them after it.
    let mut kick = [false; MAX_PAIRS];
    for (k, kick) in kick.iter_mut().enumerate().take(pairs) {
        *kick = fill_rx(k);
    }
    device.driver_ok();
    for (k, &kick) in kick.iter().enumerate().take(pairs) {
        if kick {
            device.notify_queue(2 * k as u32);
        }
    }

    // Section 5.1.6.5.6: only the first pair is used until the device accepts
    // VQ_PAIRS_SET; a refusal or no control queue keeps it at one.
    let mut active = 1;
    if pairs > 1 {
        if let Some((index, ctrl)) = ctrl.as_mut() {
            let count = (pairs as u16).to_le_bytes();
            if control(device, *index, ctrl, 4, 0, &count) {
                active = pairs;
            }
        }
    }
    let rx_depth = RXQ[0].lock().ring.size;
    let tx_depth = TXQ[0].lock().queue.size();
    crate::serial_println!(
        "[virtio-net] {} queue pair(s) active of {} set up (device {}, wanted {}); RX depth {}, TX depth {}",
        active, pairs, device_pairs, want, rx_depth, tx_depth
    );
    crate::serial_println!("[virtio-net] Network device initialized successfully");
    Ok((mac, neg, active))
}

/// The transmit queue for `frame`: its IPv4 flow's hash over the queue pairs,
/// so a flow keeps its order; anything else goes on the first.
fn tx_queue_for(frame: &[u8], pairs: usize) -> usize {
    if pairs <= 1 || frame.len() < ETHERNET_HEADER + 20 || frame[12..14] != [0x08, 0x00] {
        return 0;
    }
    let ip = &frame[ETHERNET_HEADER..];
    let ihl = usize::from(ip[0] & 0x0f) * 4;
    let protocol = ip[9];
    // FNV-1a over the addresses, the protocol and, for TCP and UDP, the ports.
    let mut hash: u32 = 0x811c_9dc5;
    let mut mix = |bytes: &[u8]| {
        for &b in bytes {
            hash = (hash ^ u32::from(b)).wrapping_mul(0x0100_0193);
        }
    };
    mix(&ip[12..20]);
    mix(&[protocol]);
    if (protocol == 6 || protocol == 17) && ip.len() >= ihl + 4 {
        mix(&ip[ihl..ihl + 4]);
    }
    hash as usize % pairs
}

/// Hardware offload for one transmitted frame (section 5.1.6.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TxOffload {
    /// An IPv4 TCP frame whose TCP checksum field holds the pseudo-header sum:
    /// the device completes the checksum and, if the payload is longer than
    /// `mss`, cuts the frame into segments of `mss` payload bytes.
    Tcp4 { mss: u16 },
}

/// The virtio_net_hdr bytes for `offload` on `frame`, or why the frame cannot
/// be offloaded. The header lengths and the TCP checksum offsets come from the
/// frame itself, checked against it.
fn offload_header(neg: &Negotiated, frame: &[u8], offload: TxOffload) -> Result<[u8; 12], &'static str> {
    let TxOffload::Tcp4 { mss } = offload;
    if !neg.has(features::CSUM) {
        return Err("checksum offload not negotiated");
    }
    if mss == 0 || frame.len() < ETHERNET_HEADER + 20 || frame[12..14] != [0x08, 0x00] {
        return Err("not an IPv4 frame");
    }
    let ip = &frame[ETHERNET_HEADER..];
    let ihl = usize::from(ip[0] & 0x0f) * 4;
    let total = usize::from(u16::from_be_bytes([ip[2], ip[3]]));
    if ip[0] >> 4 != 4 || ihl < 20 || ip[9] != 6 || total != ip.len() || ip.len() < ihl + 20 {
        return Err("not a whole IPv4 TCP packet");
    }
    let tcp_header = usize::from(ip[ihl + 12] >> 4) * 4;
    let headers = ETHERNET_HEADER + ihl + tcp_header;
    if tcp_header < 20 || headers > frame.len() {
        return Err("TCP header runs past the frame");
    }
    let segment = frame.len() - headers > usize::from(mss);
    if segment && (!neg.has(features::HOST_TSO4) || frame.len() > MAX_TSO_FRAME) {
        return Err("segmentation offload cannot take this frame");
    }
    let csum_start = (ETHERNET_HEADER + ihl) as u16;
    let mut h = [0u8; 12];
    h[0] = HDR_F_NEEDS_CSUM;
    h[1] = if segment { GSO_TCPV4 } else { GSO_NONE };
    h[2..4].copy_from_slice(&(headers as u16).to_le_bytes());
    h[4..6].copy_from_slice(&(if segment { mss } else { 0 }).to_le_bytes());
    h[6..8].copy_from_slice(&csum_start.to_le_bytes());
    h[8..10].copy_from_slice(&16u16.to_le_bytes());
    Ok(h)
}

/// Why `transmit_offload` did not send a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OffloadError {
    /// The frame or the negotiated features cannot carry this offload; the
    /// caller sends the frame in software instead.
    Unsupported(&'static str),
    /// The device is not ready or its queue is full; try again later.
    Device(&'static str),
}

/// The largest frame `transmit_offload` segments in one request, when the
/// device negotiated TCPv4 segmentation offload.
pub fn tso_max_frame() -> Option<usize> {
    let info = DEVICE.get()?;
    let chain = (MAX_CHAIN - 1) * BUF - info.neg.hdr_len;
    info.neg.has(features::HOST_TSO4).then(|| MAX_TSO_FRAME.min(chain))
}

/// Transmit `frame` with `offload`: an IPv4 TCP frame, or batch of segments,
/// whose TCP checksum the device completes. Refused without a change to the
/// queue when the frame or the negotiated features cannot carry it; the
/// caller then sends it in software.
pub fn transmit_offload(frame: &[u8], offload: TxOffload) -> Result<(), OffloadError> {
    let info = DEVICE.get().ok_or(OffloadError::Device("Network device not initialized"))?;
    let header = offload_header(&info.neg, frame, offload).map_err(OffloadError::Unsupported)?;
    submit(info, frame, &header).map_err(|e| match e {
        TxError::TooLong => OffloadError::Unsupported("Packet too large"),
        other => OffloadError::Device(tx_error_text(other)),
    })
}

fn tx_error_text(error: TxError) -> &'static str {
    match error {
        TxError::Full => "TX queue full",
        TxError::TooLong => "Packet too large",
        TxError::NotReady => "Network device not initialized",
    }
}

/// Transmit a frame without offload.
pub fn transmit(data: &[u8]) -> Result<(), &'static str> {
    let info = DEVICE.get().ok_or("Network device not initialized")?;
    if data.len() > info.neg.max_frame {
        return Err("Packet too large");
    }
    submit(info, data, &[0; 12]).map_err(tx_error_text)
}

/// Copy the header and `frame` into a transmit queue's buffers and publish
/// them as one chain, the header in a descriptor of its own when the device
/// needs that, then notify the device if it asked.
fn submit(info: &DeviceInfo, frame: &[u8], header: &[u8; 12]) -> Result<(), TxError> {
    let k = tx_queue_for(frame, info.pairs);
    let hdr = &header[..info.neg.hdr_len];
    let separate = info.neg.separate_header;
    let count = if separate {
        1 + frame.len().div_ceil(BUF)
    } else {
        (hdr.len() + frame.len()).div_ceil(BUF)
    };
    let mut guard = TXQ[k].lock();
    let state = &mut *guard;
    let pool = state.pool;
    let submitted = state.queue.submit_chain(count, |slots, out| {
        let mut copied = 0usize;
        for (i, (&slot, out)) in slots.iter().zip(out.iter_mut()).enumerate() {
            let buf = pool.addr(slot);
            let mut at = 0usize;
            if i == 0 {
                // SAFETY: the slot's buffer is ours until the device returns it.
                unsafe { core::ptr::copy_nonoverlapping(hdr.as_ptr(), buf as *mut u8, hdr.len()) };
                at = hdr.len();
            }
            if !(separate && i == 0) {
                let n = (BUF - at).min(frame.len() - copied);
                unsafe {
                    core::ptr::copy_nonoverlapping(frame[copied..].as_ptr(), (buf + at as u64) as *mut u8, n)
                };
                copied += n;
                at += n;
            }
            *out = (virt_to_phys(buf), at as u32);
        }
    });
    let idx = submitted?;
    if state.queue.should_notify(idx) {
        notify(info.base_virt, 2 * k as u32 + 1);
    }
    Ok(())
}

/// Take back completed transmissions on the active queues; returns how many.
pub fn reclaim_tx_completed() -> usize {
    let Some(info) = DEVICE.get() else { return 0 };
    TXQ[..info.pairs].iter().map(|q| q.lock().queue.reclaim()).sum()
}

/// The next received frame on any active queue (non-blocking). It stays valid
/// until the next `receive` that looks at its queue, or `recycle_rx_buffers`.
///
/// A queue with no further frame gives back the buffers it holds: its earlier
/// frames are no longer valid, and a drain whose completions were all dropped
/// hands nothing up, so its caller has no batch to recycle.
pub fn receive() -> Option<&'static [u8]> {
    let info = DEVICE.get()?;
    let start = RX_NEXT.load(Ordering::Relaxed);
    for i in 0..info.pairs {
        let k = (start + i) % info.pairs;
        let mut rx = RXQ[k].lock();
        if let Some(frame) = rx.next_frame() {
            RX_NEXT.store((k + 1) % info.pairs, Ordering::Relaxed);
            return Some(frame);
        }
        if rx.recycle() {
            notify(info.base_virt, 2 * k as u32);
        }
    }
    None
}

/// Offer the receive buffers taken since the last call back to the device.
/// Call after processing a batch of received frames.
pub fn recycle_rx_buffers() {
    let Some(info) = DEVICE.get() else { return };
    for (k, queue) in RXQ[..info.pairs].iter().enumerate() {
        // VirtIO 1.2 section 2.7: only a used buffer has returned to the
        // driver, so only those are offered again.
        let kick = queue.lock().recycle();
        if kick {
            notify(info.base_virt, 2 * k as u32);
        }
    }
}

/// Completions dropped as malformed, over the receive queues.
pub fn rx_dropped() -> u64 {
    let Some(info) = DEVICE.get() else { return 0 };
    RXQ[..info.pairs].iter().map(|q| q.lock().dropped).sum()
}

/// The accepted feature bits.
pub fn negotiated_features() -> Option<u64> {
    DEVICE.get().map(|d| d.neg.features)
}

/// Get the MAC address
pub fn mac_address() -> Option<[u8; 6]> {
    DEVICE.get().map(|d| d.mac)
}

/// Enable the network device's interrupt in the GIC.
///
/// Called after the network stack is fully initialized (ARP resolved, etc.)
/// to avoid interrupt storms during the synchronous init polling loop.
pub fn enable_net_irq() {
    let Some(irq) = get_irq() else {
        return;
    };
    crate::serial_println!("[virtio-net] Enabling IRQ {} for network device", irq);
    use crate::arch_impl::aarch64::gic;
    use crate::arch_impl::traits::InterruptController;
    gic::Gicv2::enable_irq(irq as u8);
}

/// Get the IRQ number for the network device (if initialized)
pub fn get_irq() -> Option<u32> {
    DEVICE.get().map(|d| VIRTIO_IRQ_BASE + d.slot as u32)
}

/// Interrupt handler for VirtIO network device
///
/// Called from the GIC interrupt dispatcher. Acknowledges the device interrupt
/// and raises the NetRx softirq, which takes back transmit completions and
/// drains the receive queues: the MMIO device has one interrupt for its
/// queues.
///
/// Uses direct volatile MMIO access (no probe/lock) — safe in interrupt context.
pub fn handle_interrupt() {
    let Some(info) = DEVICE.get() else {
        return;
    };

    unsafe {
        let status_addr = (info.base_virt + REG_INTERRUPT_STATUS) as *const u32;
        let status = crate::arch_impl::aarch64::mmio::read32(status_addr);
        if status != 0 {
            let ack_addr = (info.base_virt + REG_INTERRUPT_ACK) as *mut u32;
            crate::arch_impl::aarch64::mmio::write32(ack_addr, status);
        }
    }

    crate::task::softirqd::raise_softirq(crate::task::softirqd::SoftirqType::NetRx);
}

/// Test the network device
pub fn test_device() -> Result<(), &'static str> {
    let info = DEVICE.get().ok_or("Network device not initialized")?;
    let mac = info.mac;
    crate::serial_println!(
        "[virtio-net] Device test - MAC: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} at {:#x}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5], info.base
    );
    crate::serial_println!("[virtio-net] Test passed!");
    Ok(())
}

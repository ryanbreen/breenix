//! One split virtqueue (VirtIO 1.3 CSD01, section 2.7), as the control queue.
//!
//! The driver owns a descriptor from the moment it is taken from the free list until the
//! device returns its chain's head in the used ring (section 2.7.13), or until the device
//! is reset, which ends every use (section 2.4). Nothing here frees or reuses a buffer;
//! the caller's request holds its buffers for that same span.

use alloc::vec::Vec;
use core::sync::atomic::{fence, Ordering};

use crate::gpu::dma::Pages;

const DESC_F_NEXT: u16 = 1;
const DESC_F_WRITE: u16 = 2;
const DESC_SIZE: usize = 16;

/// The most descriptors one queue uses: one page of descriptor table.
pub const MAX_SIZE: u16 = 256;
/// One full SUBMIT_3D chain, kept out of the normal free list for terminal output.
pub(super) const PANIC_DESCRIPTORS: u16 = 18;

/// One buffer of a chain: device-readable unless `write`.
#[derive(Clone, Copy)]
pub struct Segment {
    pub addr: u64,
    pub len: u32,
    pub write: bool,
}

pub struct Queue {
    size: u16,
    desc: Pages,
    avail: Pages,
    used: Pages,
    free: Vec<u16>,
    /// Descriptors each in-flight head's chain holds.
    chain: Vec<u16>,
    next_avail: u16,
    last_used: u16,
}

impl Queue {
    /// A queue of `size` entries (a power of two, at most [`MAX_SIZE`]).
    pub fn new(size: u16) -> Option<Queue> {
        let size = size.min(MAX_SIZE);
        if size <= PANIC_DESCRIPTORS || !size.is_power_of_two() {
            return None;
        }
        let mut q = Queue {
            size,
            desc: Pages::new(1)?,
            avail: Pages::new(1)?,
            used: Pages::new(1)?,
            free: Vec::new(),
            chain: alloc::vec![0; size as usize],
            next_avail: 0,
            last_used: 0,
        };
        q.reset();
        Some(q)
    }

    /// Forget every chain: after a device reset, or before first use.
    pub fn reset(&mut self) {
        let _access = crate::gpu::panic::Access::enter();
        self.desc.fill(0, crate::gpu::dma::PAGE, 0);
        self.avail.fill(0, crate::gpu::dma::PAGE, 0);
        self.used.fill(0, crate::gpu::dma::PAGE, 0);
        self.desc.maintain(0, crate::gpu::dma::PAGE, false);
        self.avail.maintain(0, crate::gpu::dma::PAGE, false);
        self.used.maintain(0, crate::gpu::dma::PAGE, false);
        self.free.clear();
        self.free.extend((0..self.size - PANIC_DESCRIPTORS).rev());
        self.chain.iter_mut().for_each(|c| *c = 0);
        self.next_avail = 0;
        self.last_used = 0;
    }

    pub fn size(&self) -> u16 {
        self.size
    }

    pub fn normal_capacity(&self) -> u16 {
        self.size - PANIC_DESCRIPTORS
    }

    pub fn free_count(&self) -> usize {
        self.free.len()
    }

    pub fn desc_addr(&self) -> u64 {
        self.desc.page_addr(0)
    }

    pub fn avail_addr(&self) -> u64 {
        self.avail.page_addr(0)
    }

    pub fn used_addr(&self) -> u64 {
        self.used.page_addr(0)
    }

    pub(super) fn terminal(&self, doorbell: u64) -> super::emergency::Queue {
        super::emergency::Queue::new(
            self.size,
            self.desc.page_va(0),
            self.avail.page_va(0),
            self.used.page_va(0),
            doorbell,
        )
    }

    /// Chain `segs` and make it available; returns its head, or `None` without side
    /// effects when there are not enough free descriptors. The caller notifies.
    pub fn add(&mut self, segs: &[Segment]) -> Option<u16> {
        if segs.is_empty() || segs.len() > self.free.len() {
            return None;
        }
        let ids: Vec<u16> = (0..segs.len())
            .map(|_| self.free.pop().unwrap_or(0))
            .collect();
        let _access = crate::gpu::panic::Access::enter();
        let base = self.desc.page_va(0);
        for (i, seg) in segs.iter().enumerate() {
            let id = ids[i] as usize;
            let mut flags = if seg.write { DESC_F_WRITE } else { 0 };
            let next = if i + 1 < segs.len() {
                flags |= DESC_F_NEXT;
                ids[i + 1]
            } else {
                0
            };
            let d = base + (id * DESC_SIZE) as u64;
            // SAFETY: the descriptor table page is owned; id < size.
            unsafe {
                core::ptr::write_volatile(d as *mut u64, seg.addr);
                core::ptr::write_volatile((d + 8) as *mut u32, seg.len);
                core::ptr::write_volatile((d + 12) as *mut u16, flags);
                core::ptr::write_volatile((d + 14) as *mut u16, next);
            }
        }
        let head = ids[0];
        self.chain[head as usize] = segs.len() as u16;
        let ring_slot = (self.next_avail % self.size) as u64;
        let avail = self.avail.page_va(0);
        // SAFETY: the avail ring page is owned; ring_slot < size.
        unsafe { core::ptr::write_volatile((avail + 4 + 2 * ring_slot) as *mut u16, head) };
        self.desc.maintain(0, self.size as usize * DESC_SIZE, false);
        // Descriptors and ring entry before the index (section 2.7.13.3.1).
        fence(Ordering::SeqCst);
        self.next_avail = self.next_avail.wrapping_add(1);
        // SAFETY: as above.
        unsafe { core::ptr::write_volatile((avail + 2) as *mut u16, self.next_avail) };
        self.avail.maintain(0, 4 + 2 * self.size as usize, false);
        fence(Ordering::SeqCst);
        Some(head)
    }

    /// The next chain the device returned, as its head and the bytes it wrote; its
    /// descriptors go back to the free list. `Err` when the entry names no chain the
    /// driver made available: a device fault (`docs/gui/gpu.md` section 1.4.1).
    pub fn pop_used(&mut self) -> Result<Option<(u16, u32)>, ()> {
        let _access = crate::gpu::panic::Access::enter();
        let used = self.used.page_va(0);
        self.used.maintain(0, 4 + 8 * self.size as usize, true);
        // SAFETY: the used ring page is owned.
        let idx = unsafe { core::ptr::read_volatile((used + 2) as *const u16) };
        if idx == self.last_used {
            return Ok(None);
        }
        // The index before the entry it covers (section 2.7.14).
        fence(Ordering::SeqCst);
        let slot = (self.last_used % self.size) as u64;
        // SAFETY: as above; slot < size.
        let (id, len) = unsafe {
            (
                core::ptr::read_volatile((used + 4 + 8 * slot) as *const u32),
                core::ptr::read_volatile((used + 8 + 8 * slot) as *const u32),
            )
        };
        self.last_used = self.last_used.wrapping_add(1);
        if id >= self.size as u32 || self.chain[id as usize] == 0 {
            return Err(());
        }
        let head = id as u16;
        self.release_chain(head);
        Ok(Some((head, len)))
    }

    /// Return a returned chain's descriptors to the free list by walking its links.
    fn release_chain(&mut self, head: u16) {
        let count = core::mem::take(&mut self.chain[head as usize]);
        let base = self.desc.page_va(0);
        let mut id = head;
        for _ in 0..count {
            self.free.push(id);
            let d = base + (id as usize * DESC_SIZE) as u64;
            // SAFETY: owned descriptor table.
            let (flags, next) = unsafe {
                (
                    core::ptr::read_volatile((d + 12) as *const u16),
                    core::ptr::read_volatile((d + 14) as *const u16),
                )
            };
            if flags & DESC_F_NEXT == 0 {
                break;
            }
            id = next;
        }
    }
}

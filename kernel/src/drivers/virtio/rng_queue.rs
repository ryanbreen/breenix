//! The entropy device's request queue (virtio 1.x section 5.4): one split
//! virtqueue (section 2.6) on which the driver offers a device-writable buffer
//! and the device returns it with random bytes in it.
//!
//! One request is outstanding at a time, in descriptor 0. The memory
//! fits both layouts the transports use: the legacy one, whose used ring starts
//! at the first `LEGACY_ALIGN` boundary after the available ring, and the
//! modern one, whose three parts the driver places separately (descriptor table
//! 16-aligned, available ring 2-aligned, used ring 4-aligned).
//!
//! Pure computation on the queue memory, so the host tests compile this file as is.

use core::ptr::{addr_of, addr_of_mut, read_volatile, write_volatile};
use core::sync::atomic::{fence, Ordering};

/// The entropy device's virtio device ID.
pub const DEVICE_ID: u32 = 4;
/// Its PCI device ID: 0x1040 plus the device ID.
pub const PCI_DEVICE_ID: u16 = 0x1040 + DEVICE_ID as u16;
/// Its transitional PCI device ID.
pub const PCI_TRANSITIONAL_DEVICE_ID: u16 = 0x1005;
/// Entries in the queue, a power of two as the split layout requires.
pub const QUEUE_SIZE: u16 = 4;
/// VIRTQ_DESC_F_WRITE: the buffer is write-only for the device.
pub const DESC_F_WRITE: u16 = 2;
/// VIRTQ_AVAIL_F_NO_INTERRUPT: the driver polls and wants no interrupt.
pub const AVAIL_F_NO_INTERRUPT: u16 = 1;
/// The legacy interface's queue alignment (QueueAlign, and the PCI legacy layout).
pub const LEGACY_ALIGN: usize = 4096;

const N: usize = QUEUE_SIZE as usize;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Descriptor {
    pub addr: u64,
    pub len: u32,
    pub flags: u16,
    pub next: u16,
}

#[repr(C)]
pub struct AvailRing {
    pub flags: u16,
    pub idx: u16,
    pub ring: [u16; N],
    pub used_event: u16,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct UsedElem {
    pub id: u32,
    pub len: u32,
}

#[repr(C)]
pub struct UsedRing {
    pub flags: u16,
    pub idx: u16,
    pub ring: [UsedElem; N],
    pub avail_event: u16,
}

const DESC_BYTES: usize = 16 * N;
const AVAIL_BYTES: usize = 6 + 2 * N;

#[repr(C, align(4096))]
pub struct QueueMemory {
    pub desc: [Descriptor; N],
    pub avail: AvailRing,
    _pad: [u8; LEGACY_ALIGN - DESC_BYTES - AVAIL_BYTES],
    pub used: UsedRing,
}

const _: () = assert!(core::mem::size_of::<Descriptor>() == 16);
const _: () = assert!(core::mem::offset_of!(QueueMemory, avail) == DESC_BYTES);
const _: () = assert!(core::mem::offset_of!(QueueMemory, used) == LEGACY_ALIGN);

impl QueueMemory {
    pub const fn new() -> Self {
        Self {
            desc: [Descriptor { addr: 0, len: 0, flags: 0, next: 0 }; N],
            avail: AvailRing { flags: 0, idx: 0, ring: [0; N], used_event: 0 },
            _pad: [0; LEGACY_ALIGN - DESC_BYTES - AVAIL_BYTES],
            used: UsedRing { flags: 0, idx: 0, ring: [UsedElem { id: 0, len: 0 }; N], avail_event: 0 },
        }
    }

    /// Byte offsets of the available and used rings from the descriptor table.
    pub const AVAIL_OFFSET: u64 = DESC_BYTES as u64;
    pub const USED_OFFSET: u64 = LEGACY_ALIGN as u64;
}

/// What a returned request held.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Completion {
    /// The device wrote this many bytes (one or more) at the start of the buffer.
    Bytes(u32),
    /// The device returned something the specification does not allow: another
    /// descriptor, no bytes, or more bytes than the buffer holds.
    Malformed,
}

/// The driver's side of the queue.
pub struct RequestQueue {
    last_used: u16,
    offered: u32,
}

impl RequestQueue {
    /// Clear the rings. Call before the transport tells the device where they are.
    pub fn new(memory: &mut QueueMemory) -> Self {
        memory.desc = [Descriptor { addr: 0, len: 0, flags: 0, next: 0 }; N];
        memory.avail = AvailRing { flags: AVAIL_F_NO_INTERRUPT, idx: 0, ring: [0; N], used_event: 0 };
        memory.used = UsedRing { flags: 0, idx: 0, ring: [UsedElem { id: 0, len: 0 }; N], avail_event: 0 };
        Self { last_used: 0, offered: 0 }
    }

    /// Offer the buffer at `buffer_phys` (`len` bytes, at least one) to the device.
    /// Returns false while an earlier request is outstanding; the caller then
    /// notifies the device on queue 0.
    pub fn offer(&mut self, memory: &mut QueueMemory, buffer_phys: u64, len: u32) -> bool {
        if self.offered != 0 || len == 0 {
            return false;
        }
        memory.desc[0] = Descriptor { addr: buffer_phys, len, flags: DESC_F_WRITE, next: 0 };
        // SAFETY: the pointers are to fields of `memory`, which the caller holds as `&mut`.
        unsafe {
            let avail = addr_of_mut!(memory.avail);
            let idx = read_volatile(addr_of!((*avail).idx));
            write_volatile(addr_of_mut!((*avail).ring[idx as usize % N]), 0);
            // The descriptor and ring entry are visible before the index that publishes them.
            fence(Ordering::SeqCst);
            write_volatile(addr_of_mut!((*avail).idx), idx.wrapping_add(1));
        }
        fence(Ordering::SeqCst);
        self.offered = len;
        true
    }

    /// The outstanding request's result, once the device has returned it.
    pub fn completed(&mut self, memory: &QueueMemory) -> Option<Completion> {
        if self.offered == 0 {
            return None;
        }
        // SAFETY: as in `offer`; the device writes the used ring, so it is read volatile.
        let element = unsafe {
            let used = addr_of!(memory.used);
            if read_volatile(addr_of!((*used).idx)) == self.last_used {
                return None;
            }
            // Read the element only after the index that publishes it.
            fence(Ordering::SeqCst);
            read_volatile(addr_of!((*used).ring[self.last_used as usize % N]))
        };
        self.last_used = self.last_used.wrapping_add(1);
        let offered = core::mem::replace(&mut self.offered, 0);
        Some(if element.id != 0 || element.len == 0 || element.len > offered {
            Completion::Malformed
        } else {
            Completion::Bytes(element.len)
        })
    }
}

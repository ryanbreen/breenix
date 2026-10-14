//! A write-through copy of the device blocks a mounted ext2 filesystem
//! transferred most recently.
//!
//! Every name lookup reads its whole directory, and every operation reads the
//! inode table, bitmaps and indirect blocks it touches, so without a copy the
//! device requests per file creation grow with the size of the directory. The
//! copy is direct-mapped: device block `b` can only be in slot `b % slots`.
//!
//! The copy never holds data the device does not. A write forgets its blocks
//! before it starts and records them after it succeeds, unless another write
//! began or was still at the device meanwhile (their order at the device is
//! then unknown). A read records what it got only if no write was at the
//! device when it started and none began before it finished.

use alloc::vec::Vec;
use spin::Mutex;

/// The bytes the copy holds, whatever the device's block size.
const CACHE_BYTES: usize = 512 * 1024;
const EMPTY: u64 = u64::MAX;

struct Slots {
    /// The device block each slot holds, or `EMPTY`.
    tags: Vec<u64>,
    /// `tags.len()` blocks of `block_size` bytes.
    data: Vec<u8>,
    /// Writes begun since the mount.
    writes: u64,
    /// Writes at the device now.
    in_flight: u32,
}

pub(super) struct SectorCache {
    block_size: usize,
    slots: Mutex<Slots>,
}

/// What a read that missed may record when it returns.
#[derive(Clone, Copy)]
pub(super) struct Fill(Option<u64>);

/// What a write may record when it returns.
#[derive(Clone, Copy)]
pub(super) struct Store(u64);

impl SectorCache {
    /// A copy for a device of `block_size`-byte blocks, or None when the
    /// memory for one is not available (the mount then runs without).
    pub(super) fn new(block_size: usize) -> Option<Self> {
        if block_size == 0 || block_size > CACHE_BYTES {
            return None;
        }
        let count = CACHE_BYTES / block_size;
        let mut tags = Vec::new();
        tags.try_reserve_exact(count).ok()?;
        tags.resize(count, EMPTY);
        let mut data = Vec::new();
        data.try_reserve_exact(count * block_size).ok()?;
        data.resize(count * block_size, 0);
        Some(Self {
            block_size,
            slots: Mutex::new(Slots { tags, data, writes: 0, in_flight: 0 }),
        })
    }

    /// The slot lock is never taken in interrupt context, but a holder must
    /// not be preempted by a thread that then spins on it.
    fn with<R>(&self, f: impl FnOnce(&mut Slots) -> R) -> R {
        crate::arch_without_interrupts(|| f(&mut self.slots.lock()))
    }

    fn store(&self, slots: &mut Slots, block: u64, buf: &[u8]) {
        let count = slots.tags.len() as u64;
        for (chunk, b) in buf.chunks_exact(self.block_size).zip(block..) {
            let slot = (b % count) as usize;
            slots.tags[slot] = b;
            slots.data[slot * self.block_size..(slot + 1) * self.block_size].copy_from_slice(chunk);
        }
    }

    /// Copy the blocks from `block` on into `buf` if every one is held;
    /// otherwise return what the read from the device may record.
    pub(super) fn lookup(&self, block: u64, buf: &mut [u8]) -> Result<(), Fill> {
        self.with(|slots| {
            let count = slots.tags.len() as u64;
            let held = buf
                .chunks_exact(self.block_size)
                .zip(block..)
                .all(|(_, b)| slots.tags[(b % count) as usize] == b);
            if !held {
                return Err(Fill((slots.in_flight == 0).then_some(slots.writes)));
            }
            for (chunk, b) in buf.chunks_exact_mut(self.block_size).zip(block..) {
                let slot = (b % count) as usize;
                chunk.copy_from_slice(&slots.data[slot * self.block_size..(slot + 1) * self.block_size]);
            }
            Ok(())
        })
    }

    /// Record what a read that missed got from the device.
    pub(super) fn fill(&self, fill: Fill, block: u64, buf: &[u8]) {
        let Fill(Some(writes)) = fill else { return };
        self.with(|slots| {
            if slots.in_flight == 0 && slots.writes == writes {
                self.store(slots, block, buf);
            }
        })
    }

    /// Forget the blocks a write is about to change.
    pub(super) fn begin_write(&self, block: u64, len: usize) -> Store {
        self.with(|slots| {
            slots.writes += 1;
            slots.in_flight += 1;
            let count = slots.tags.len() as u64;
            for b in block..block + (len / self.block_size) as u64 {
                let slot = (b % count) as usize;
                if slots.tags[slot] == b {
                    slots.tags[slot] = EMPTY;
                }
            }
            Store(slots.writes)
        })
    }

    /// Record a write's blocks if it succeeded and was the only write at the
    /// device since it began.
    pub(super) fn end_write(&self, store: Store, block: u64, written: Option<&[u8]>) {
        self.with(|slots| {
            if let Some(buf) = written {
                if slots.in_flight == 1 && slots.writes == store.0 {
                    self.store(slots, block, buf);
                }
            }
            slots.in_flight -= 1;
        })
    }
}

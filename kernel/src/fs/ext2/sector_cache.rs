//! A write-through copy of the device blocks a mounted ext2 filesystem
//! transferred most recently.
//!
//! Every name lookup reads its whole directory, and every operation reads the
//! inode table, bitmaps and indirect blocks it touches, so without a copy the
//! device requests per file creation grow with the size of the directory.
//!
//! The copy is set-associative: device block `b` can be in any of the `WAYS`
//! slots of one set, chosen by a hash of `b`, and a set replaces its least
//! recently used block. Every ext2 block group puts its bitmaps and inode table
//! at the same offset, and groups are a power of two in size, so a
//! direct-mapped copy put the next group's metadata in the slots of the blocks
//! each creation rewrites, and they evicted each other on every creation.
//!
//! The copy never holds data the device does not. A write forgets its blocks
//! before it starts and records them after it succeeds only if it was the
//! only write at the device throughout: none was in flight when it began and
//! none began before it ended (otherwise their order at the device is
//! unknown). A read records what it got only if no write was at the device
//! when it started and none began before it finished.

use alloc::vec::Vec;
use spin::Mutex;

/// The bytes the copy holds, whatever the device's block size.
const CACHE_BYTES: usize = 512 * 1024;
/// The slots a block may occupy.
const WAYS: usize = 8;
const EMPTY: u64 = u64::MAX;

struct Slots {
    /// The device block each slot holds, or `EMPTY`.
    tags: Vec<u64>,
    /// When each slot was last used, by `clock`.
    used: Vec<u64>,
    /// `tags.len()` blocks of `block_size` bytes.
    data: Vec<u8>,
    /// Counts slot uses.
    clock: u64,
    /// Writes begun since the mount.
    writes: u64,
    /// Writes at the device now.
    in_flight: u32,
}

impl Slots {
    /// The first slot of the set block `b` belongs to.
    fn set(&self, b: u64) -> usize {
        let sets = (self.tags.len() / WAYS) as u64;
        // Fibonacci hashing: blocks at a regular stride spread over the sets.
        ((b.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 32) % sets) as usize * WAYS
    }

    /// The slot holding block `b`, if one does.
    fn find(&self, b: u64) -> Option<usize> {
        let set = self.set(b);
        (set..set + WAYS).find(|slot| self.tags[*slot] == b)
    }

    /// Mark `slot` used now.
    fn touch(&mut self, slot: usize) {
        self.clock += 1;
        self.used[slot] = self.clock;
    }
}

pub(super) struct SectorCache {
    block_size: usize,
    slots: Mutex<Slots>,
}

/// What a read that missed may record when it returns.
#[derive(Clone, Copy)]
pub(super) struct Fill(Option<u64>);

/// What a write may record when it returns: the write's place in the count
/// of writes, if it began with no other write in flight.
#[derive(Clone, Copy)]
pub(super) struct Store(Option<u64>);

impl SectorCache {
    /// A copy for a device of `block_size`-byte blocks, or None when the
    /// memory for one is not available (the mount then runs without).
    pub(super) fn new(block_size: usize) -> Option<Self> {
        if block_size == 0 || block_size > CACHE_BYTES / WAYS {
            return None;
        }
        let count = CACHE_BYTES / block_size / WAYS * WAYS;
        let mut tags = Vec::new();
        tags.try_reserve_exact(count).ok()?;
        tags.resize(count, EMPTY);
        let mut used = Vec::new();
        used.try_reserve_exact(count).ok()?;
        used.resize(count, 0);
        let mut data = Vec::new();
        data.try_reserve_exact(count * block_size).ok()?;
        data.resize(count * block_size, 0);
        Some(Self {
            block_size,
            slots: Mutex::new(Slots { tags, used, data, clock: 0, writes: 0, in_flight: 0 }),
        })
    }

    /// The slot lock is never taken in interrupt context, but a holder must
    /// not be preempted by a thread that then spins on it.
    fn with<R>(&self, f: impl FnOnce(&mut Slots) -> R) -> R {
        crate::arch_without_interrupts(|| f(&mut self.slots.lock()))
    }

    fn store(&self, slots: &mut Slots, block: u64, buf: &[u8]) {
        for (chunk, b) in buf.chunks_exact(self.block_size).zip(block..) {
            // The block's own slot, else an empty one, else the least recently used.
            let set = slots.set(b);
            let slot = slots.find(b).unwrap_or_else(|| {
                (set..set + WAYS)
                    .min_by_key(|slot| (slots.tags[*slot] != EMPTY, slots.used[*slot]))
                    .unwrap_or(set)
            });
            slots.tags[slot] = b;
            slots.touch(slot);
            slots.data[slot * self.block_size..(slot + 1) * self.block_size].copy_from_slice(chunk);
        }
    }

    /// Copy the blocks from `block` on into `buf` if every one is held;
    /// otherwise return what the read from the device may record.
    pub(super) fn lookup(&self, block: u64, buf: &mut [u8]) -> Result<(), Fill> {
        self.with(|slots| {
            let count = buf.len() / self.block_size;
            if !(block..block + count as u64).all(|b| slots.find(b).is_some()) {
                return Err(Fill((slots.in_flight == 0).then_some(slots.writes)));
            }
            for (chunk, b) in buf.chunks_exact_mut(self.block_size).zip(block..) {
                if let Some(slot) = slots.find(b) {
                    slots.touch(slot);
                    chunk.copy_from_slice(&slots.data[slot * self.block_size..(slot + 1) * self.block_size]);
                }
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
            let alone = slots.in_flight == 0;
            slots.writes += 1;
            slots.in_flight += 1;
            for b in block..block + (len / self.block_size) as u64 {
                if let Some(slot) = slots.find(b) {
                    slots.tags[slot] = EMPTY;
                }
            }
            Store(alone.then_some(slots.writes))
        })
    }

    /// Record a write's blocks if it succeeded and was the only write at the
    /// device from its start to its end.
    pub(super) fn end_write(&self, store: Store, block: u64, written: Option<&[u8]>) {
        self.with(|slots| {
            if let Some(buf) = written {
                if store.0 == Some(slots.writes) {
                    self.store(slots, block, buf);
                }
            }
            slots.in_flight -= 1;
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: usize = 512;

    fn write(cache: &SectorCache, block: u64, byte: u8) {
        let store = cache.begin_write(block, SIZE);
        cache.end_write(store, block, Some(&[byte; SIZE]));
    }

    fn held(cache: &SectorCache, block: u64) -> Option<u8> {
        let mut buf = [0u8; SIZE];
        cache.lookup(block, &mut buf).ok().map(|()| buf[0])
    }

    #[test]
    fn blocks_a_block_group_apart_are_held_together() {
        // An 8 MiB block group is 16384 sectors: each group's metadata sits at
        // the same offset, which a direct-mapped copy put in one slot.
        let cache = SectorCache::new(SIZE).unwrap();
        for group in 0..WAYS as u64 {
            write(&cache, 3 + group * 16384, group as u8);
        }
        for group in 0..WAYS as u64 {
            assert_eq!(held(&cache, 3 + group * 16384), Some(group as u8));
        }
    }

    #[test]
    fn the_least_recently_used_block_of_a_set_is_replaced() {
        let cache = SectorCache::new(SIZE).unwrap();
        let first = 7u64;
        let set = cache.with(|slots| slots.set(first));
        let mut members = (first..).filter(|b| cache.with(|slots| slots.set(*b)) == set);
        let blocks: Vec<u64> = (&mut members).take(WAYS).collect();
        for (n, b) in blocks.iter().enumerate() {
            write(&cache, *b, n as u8);
        }
        assert_eq!(held(&cache, blocks[0]), Some(0));
        write(&cache, members.next().unwrap(), 0xff);
        assert_eq!(held(&cache, blocks[0]), Some(0));
        assert_eq!(held(&cache, blocks[1]), None);
    }

    #[test]
    fn overlapping_writes_record_nothing() {
        let cache = SectorCache::new(SIZE).unwrap();
        // A begins, B begins and ends, A ends: the device may hold either.
        let a = cache.begin_write(5, SIZE);
        let b = cache.begin_write(5, SIZE);
        cache.end_write(b, 5, Some(&[2; SIZE]));
        cache.end_write(a, 5, Some(&[1; SIZE]));
        assert_eq!(held(&cache, 5), None);
        // A begins, B begins, A ends, B ends.
        let a = cache.begin_write(5, SIZE);
        let b = cache.begin_write(5, SIZE);
        cache.end_write(a, 5, Some(&[1; SIZE]));
        cache.end_write(b, 5, Some(&[2; SIZE]));
        assert_eq!(held(&cache, 5), None);
    }

    #[test]
    fn a_read_overtaken_by_a_write_records_nothing() {
        let cache = SectorCache::new(SIZE).unwrap();
        let mut buf = [0u8; SIZE];
        let Err(fill) = cache.lookup(9, &mut buf) else { panic!("an empty copy held a block") };
        write(&cache, 9, 2);
        cache.fill(fill, 9, &[1; SIZE]);
        assert_eq!(held(&cache, 9), Some(2));
    }
}

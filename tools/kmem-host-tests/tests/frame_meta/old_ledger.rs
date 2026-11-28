//! The frame-generation ledger as it stood before E1-3, from
//! `kernel/src/memory/frame_allocator.rs` at 7ce607c7, kept so the lease
//! semantics of the new metadata can be compared with it.
//!
//! The bodies are the kernel's. Only its glue is replaced: the statics are
//! fields, `PhysFrame` is a frame address, counters are left out because each
//! outcome is returned, and `with_free_frames` reads `held` for "this CPU
//! already holds the free list", which in the kernel happens when the
//! capacity reservation grows the heap and the heap allocates a frame.

use crate::boot_ranges::RangeSet;
use std::sync::atomic::{AtomicU16, AtomicU32, Ordering};
use std::sync::OnceLock;

const LEDGER_CHUNK_FRAMES: usize = 64 * 1024;
const MAX_TRACKED_FRAMES: usize = u32::MAX as usize;
const ST_NEVER: u32 = 0;
const ST_ALLOCATED: u32 = 1;
const ST_FREE: u32 = 2;
const STATE_MASK: u32 = 0b11;
const LEAF_EXTERNAL: u32 = u32::MAX;
const LOW_MEMORY_FLOOR: u64 = 0x100000;

struct FrameLedgerSlot {
    allocation: AtomicU32,
    leaf_refs: AtomicU32,
    site: AtomicU16,
}

impl FrameLedgerSlot {
    fn new(allocation: u32) -> Self {
        Self {
            allocation: AtomicU32::new(allocation),
            leaf_refs: AtomicU32::new(0),
            site: AtomicU16::new(0),
        }
    }

    fn load(&self, order: Ordering) -> u32 {
        self.allocation.load(order)
    }

    fn compare_exchange(
        &self,
        current: u32,
        new: u32,
        success: Ordering,
        failure: Ordering,
    ) -> Result<u32, u32> {
        self.allocation
            .compare_exchange(current, new, success, failure)
    }
}

struct FrameLedger {
    chunks: &'static [OnceLock<&'static [FrameLedgerSlot]>],
    total_frames: usize,
    initial_frontier: usize,
}

impl FrameLedger {
    fn try_new(total_frames: usize, initial_frontier: usize) -> Result<Self, ()> {
        let chunk_count = total_frames.div_ceil(LEDGER_CHUNK_FRAMES);
        let mut chunks = Vec::new();
        chunks.try_reserve_exact(chunk_count).map_err(|_| ())?;
        chunks.resize_with(chunk_count, OnceLock::new);
        Ok(Self {
            chunks: Vec::leak(chunks),
            total_frames,
            initial_frontier,
        })
    }

    fn ensure_chunk(&self, index: usize) -> Result<&'static [FrameLedgerSlot], ()> {
        if index >= self.total_frames {
            return Err(());
        }

        let chunk_index = index / LEDGER_CHUNK_FRAMES;
        if let Some(slots) = self.chunks[chunk_index].get() {
            return Ok(*slots);
        }

        let start = chunk_index * LEDGER_CHUNK_FRAMES;
        let end = (start + LEDGER_CHUNK_FRAMES).min(self.total_frames);
        let mut slots = Vec::new();
        slots.try_reserve_exact(end - start).map_err(|_| ())?;
        let mut slot_index = start;
        slots.resize_with(end - start, || {
            let state = if slot_index < self.initial_frontier {
                (1 << 2) | ST_ALLOCATED
            } else {
                ST_NEVER
            };
            slot_index += 1;
            FrameLedgerSlot::new(state)
        });

        let mut prepared = Some(slots);
        let published = self.chunks[chunk_index].get_or_init(|| {
            Vec::leak(prepared.take().expect("prepared frame-ledger chunk"))
                as &'static [FrameLedgerSlot]
        });
        Ok(*published)
    }

    fn get(&self, index: usize) -> Option<&FrameLedgerSlot> {
        if index >= self.total_frames {
            return None;
        }
        self.chunks[index / LEDGER_CHUNK_FRAMES]
            .get()
            .and_then(|slots| slots.get(index % LEDGER_CHUNK_FRAMES))
    }
}

#[derive(Clone, Copy)]
pub struct FrameLease {
    pub frame: u64,
    index: u32,
    pub generation: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReturnOutcome {
    Returned,
    LostContended,
    RefusedDoubleRelease,
    RefusedStale,
    RefusedNeverAllocated,
    RefusedUntracked,
    RefusedLiveLeaf,
}

enum PrepareFrame {
    Ready,
    Contended,
    Exhausted,
}

enum ClaimError {
    Duplicate,
    Untracked,
}

pub struct OldLedger<'a> {
    info: &'a RangeSet,
    next_free_frame: usize,
    ledger: Option<FrameLedger>,
    free_frames: Vec<u64>,
    free_frame_capacity: usize,
    /// This CPU holds the free list (`FREE_FRAMES_OWNER == me`).
    pub held: bool,
}

impl<'a> OldLedger<'a> {
    pub fn new(info: &'a RangeSet) -> Self {
        Self {
            info,
            next_free_frame: 0,
            ledger: None,
            free_frames: Vec::new(),
            free_frame_capacity: 0,
            held: false,
        }
    }

    pub fn free_list_len(&self) -> usize {
        self.free_frames.len()
    }

    pub fn reserved_capacity(&self) -> usize {
        self.free_frame_capacity
    }

    fn with_free_frames<R>(&mut self, f: impl FnOnce(Option<&mut Vec<u64>>) -> R) -> R {
        if self.held {
            return f(None);
        }
        f(Some(&mut self.free_frames))
    }

    fn get_usable_frame(&self, n: usize) -> Option<u64> {
        let mut count = 0;
        for region in self.info.iter() {
            let region_frames = (region.end - region.start) / 4096;

            if count + region_frames as usize > n {
                let frame_offset = n - count;
                let frame_addr = region.start + (frame_offset as u64 * 4096);
                if frame_addr < LOW_MEMORY_FLOOR {
                    return None;
                }
                return Some(frame_addr);
            }
            count += region_frames as usize;
        }
        None
    }

    fn frame_ordinal(&self, address: u64) -> Option<usize> {
        let mut ordinal = 0usize;
        for region in self.info.iter() {
            if (region.start..region.end).contains(&address) {
                return Some(ordinal + ((address - region.start) / 4096) as usize);
            }
            ordinal += ((region.end - region.start) / 4096) as usize;
        }
        None
    }

    /// `init_frame_ledger`, with the boot `expect`s returned as `Err`.
    pub fn init_frame_ledger(&mut self) -> Result<(), ()> {
        if self.ledger.is_some() {
            return Ok(());
        }
        let advertised_frames = self
            .info
            .iter()
            .map(|region| ((region.end - region.start) / 4096) as usize)
            .sum::<usize>();
        let total_frames = advertised_frames.min(MAX_TRACKED_FRAMES);
        let frontier = self.next_free_frame.min(total_frames);
        let ledger = FrameLedger::try_new(total_frames, frontier)?;

        let mut chunk_start = 0;
        while chunk_start < frontier {
            ledger.ensure_chunk(chunk_start)?;
            chunk_start += LEDGER_CHUNK_FRAMES;
        }

        let required = frontier;
        if self.free_frames.capacity() < required {
            let additional = required.saturating_sub(self.free_frames.len());
            self.free_frames.try_reserve(additional).map_err(|_| ())?;
        }
        self.free_frame_capacity = self.free_frames.capacity();
        self.ledger = Some(ledger);
        Ok(())
    }

    fn ensure_free_frame_capacity(&mut self, required: usize) -> PrepareFrame {
        if self.free_frame_capacity >= required {
            return PrepareFrame::Ready;
        }
        let mut capacity = None;
        let result = self.with_free_frames(|free_list| match free_list {
            None => PrepareFrame::Contended,
            Some(free_list) => {
                if free_list.capacity() < required {
                    let additional = required.saturating_sub(free_list.len());
                    if free_list.try_reserve(additional).is_err() {
                        return PrepareFrame::Exhausted;
                    }
                }
                capacity = Some(free_list.capacity());
                PrepareFrame::Ready
            }
        });
        if let Some(capacity) = capacity {
            self.free_frame_capacity = capacity;
        }
        result
    }

    fn prepare_frame_for_allocation(&mut self, index: usize) -> PrepareFrame {
        let Some(ledger) = self.ledger.as_ref() else {
            return PrepareFrame::Ready;
        };
        if ledger.ensure_chunk(index).is_err() {
            return PrepareFrame::Exhausted;
        }
        self.ensure_free_frame_capacity(index + 1)
    }

    fn allocate_sequential(&mut self) -> Option<u64> {
        let mut capacity_contentions = 0u8;
        loop {
            let current = self.next_free_frame;
            let frame = self.get_usable_frame(current)?;

            match self.prepare_frame_for_allocation(current) {
                // The kernel resets the count here for a lost frontier CAS,
                // which a single host thread never sees.
                PrepareFrame::Ready => {}
                PrepareFrame::Contended if capacity_contentions < 8 => {
                    capacity_contentions += 1;
                    continue;
                }
                PrepareFrame::Contended => {}
                PrepareFrame::Exhausted => return None,
            }

            self.next_free_frame = current + 1;
            return Some(frame);
        }
    }

    fn allocate_candidate(&mut self) -> Option<u64> {
        if let Some(frame) = self.with_free_frames(|free_list| free_list.and_then(|list| list.pop())) {
            return Some(frame);
        }
        self.allocate_sequential()
    }

    fn claim_frame(&self, frame: u64) -> Result<Option<FrameLease>, ClaimError> {
        let Some(ledger) = self.ledger.as_ref() else {
            return Ok(None);
        };
        let Some(index) = self.frame_ordinal(frame) else {
            return Err(ClaimError::Untracked);
        };
        let Some(slot) = ledger.get(index) else {
            return Err(ClaimError::Untracked);
        };

        loop {
            let observed = slot.load(Ordering::Acquire);
            match observed & STATE_MASK {
                ST_NEVER | ST_FREE => {
                    if slot.leaf_refs.load(Ordering::Acquire) != 0 {
                        return Err(ClaimError::Duplicate);
                    }
                    let generation = (observed >> 2).wrapping_add(1) & 0x3fff_ffff;
                    let allocated = (generation << 2) | ST_ALLOCATED;
                    if slot
                        .compare_exchange(observed, allocated, Ordering::AcqRel, Ordering::Acquire)
                        .is_ok()
                    {
                        slot.site.store(1, Ordering::Relaxed);
                        return Ok(Some(FrameLease {
                            frame,
                            index: index as u32,
                            generation,
                        }));
                    }
                }
                ST_ALLOCATED => return Err(ClaimError::Duplicate),
                _ => return Err(ClaimError::Untracked),
            }
        }
    }

    /// `allocate_frame_leased`.
    pub fn allocate_frame_leased(&mut self) -> Option<FrameLease> {
        assert!(self.ledger.is_some(), "frame ledger not initialized");
        loop {
            let frame = self.allocate_candidate()?;
            match self.claim_frame(frame) {
                Ok(lease) => return lease,
                Err(ClaimError::Duplicate | ClaimError::Untracked) => continue,
            }
        }
    }

    pub fn return_lease(&mut self, lease: FrameLease) -> ReturnOutcome {
        let lease_index = lease.index as usize;
        if self.frame_ordinal(lease.frame) != Some(lease_index) {
            return ReturnOutcome::RefusedUntracked;
        }
        let ledger = self.ledger.as_ref().expect("frame ledger not initialized");
        let Some(slot) = ledger.get(lease_index) else {
            return if lease_index < ledger.total_frames {
                ReturnOutcome::RefusedNeverAllocated
            } else {
                ReturnOutcome::RefusedUntracked
            };
        };

        loop {
            let observed = slot.load(Ordering::Acquire);
            if observed & STATE_MASK == ST_NEVER {
                return ReturnOutcome::RefusedNeverAllocated;
            }
            if observed >> 2 != lease.generation {
                return ReturnOutcome::RefusedStale;
            }
            match observed & STATE_MASK {
                ST_ALLOCATED => {
                    if slot.leaf_refs.load(Ordering::Acquire) != 0 {
                        return ReturnOutcome::RefusedLiveLeaf;
                    }
                    let free = (observed & !STATE_MASK) | ST_FREE;
                    if slot
                        .compare_exchange(observed, free, Ordering::AcqRel, Ordering::Acquire)
                        .is_ok()
                    {
                        break;
                    }
                }
                ST_FREE => return ReturnOutcome::RefusedDoubleRelease,
                _ => return ReturnOutcome::RefusedUntracked,
            }
        }

        let frame = lease.frame;
        let push = |free_list: &mut Vec<u64>| {
            if free_list.len() == free_list.capacity() {
                return ReturnOutcome::LostContended;
            }
            free_list.push(frame);
            ReturnOutcome::Returned
        };
        self.with_free_frames(|free_list| free_list.map_or(ReturnOutcome::LostContended, push))
    }

    pub fn current_lease_for_frame(&self, frame: u64) -> FrameLease {
        let index = self.frame_ordinal(frame).unwrap_or(usize::MAX);
        let generation = self
            .ledger
            .as_ref()
            .and_then(|ledger| ledger.get(index))
            .map(|slot| slot.load(Ordering::Acquire) >> 2)
            .unwrap_or(0);
        FrameLease {
            frame,
            index: u32::try_from(index).unwrap_or(u32::MAX),
            generation,
        }
    }

    fn slot(&self, frame: u64) -> Option<&FrameLedgerSlot> {
        let index = self.frame_ordinal(frame)?;
        self.ledger.as_ref()?.get(index)
    }

    /// `acquire_leaf_mapping`: Ok(true) for an external frame.
    pub fn acquire_leaf_mapping(&self, frame: u64) -> Result<bool, &'static str> {
        let Some(slot) = self.slot(frame) else {
            return Err("leaf frame has no realized allocator slot");
        };
        if slot.load(Ordering::Acquire) & STATE_MASK != ST_ALLOCATED {
            return Err("leaf frame is not allocator-owned and live");
        }
        loop {
            let refs = slot.leaf_refs.load(Ordering::Acquire);
            if refs == LEAF_EXTERNAL {
                return Ok(true);
            }
            if refs == LEAF_EXTERNAL - 1 {
                return Err("leaf mapping reference count exhausted");
            }
            if slot
                .leaf_refs
                .compare_exchange(refs, refs + 1, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                if slot.load(Ordering::Acquire) & STATE_MASK == ST_ALLOCATED {
                    return Ok(false);
                }
                slot.leaf_refs.fetch_sub(1, Ordering::AcqRel);
                return Err("leaf frame changed allocator state during classification");
            }
        }
    }

    /// `decref_leaf_mapping`.
    pub fn decref_leaf_mapping(&self, frame: u64) -> bool {
        let Some(slot) = self.slot(frame) else {
            return false;
        };
        if slot.load(Ordering::Acquire) & STATE_MASK != ST_ALLOCATED {
            return false;
        }
        loop {
            let refs = slot.leaf_refs.load(Ordering::Acquire);
            if refs == LEAF_EXTERNAL || refs == 0 {
                return false;
            }
            if slot
                .leaf_refs
                .compare_exchange(refs, refs - 1, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return refs == 1;
            }
        }
    }

    /// `leaf_mapping_refcount`.
    pub fn leaf_mapping_refcount(&self, frame: u64) -> u32 {
        self.slot(frame)
            .map(|slot| slot.leaf_refs.load(Ordering::Acquire))
            .unwrap_or(0)
    }
}

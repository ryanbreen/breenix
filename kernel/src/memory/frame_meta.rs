//! Frame metadata sections (E1 plan, sections 4.2 and 4.3).
//!
//! A 32-byte `FrameMeta` describes a 4 KiB frame. The entries live in
//! 4 KiB metadata pages carved from the boot range set before any heap exists,
//! one page for each 512 KiB of physical address space that holds usable RAM,
//! under a sparse directory: a fixed top of 128 entries inside the map (one
//! per 32 TiB, up to the 52-bit physical address limit), root pages of 512
//! leaf pointers, leaf pages of 512 section descriptors, and descriptors of
//! one pointer per metadata page. Every carved piece is one 4 KiB page, so the
//! build needs no contiguous run. A lookup is the top entry and then four
//! dependent loads, and holes cost nothing below the top.
//! claim-lint:ok: tools/kmem-host-tests/tests/frame_meta.rs sparse_ranges_across_a_terabyte_need_no_large_run.
//!
//! The lease rules are the generation ledger's, unchanged: a claim advances a
//! 30-bit generation, a return must present it, and a refused return leaks the
//! frame rather than risking two owners.
//! claim-lint:ok: tools/kmem-host-tests/tests/frame_meta.rs lease_refusals_match_the_old_ledger_seed_5eede103. The free list is threaded through the
//! entries, so a return needs no capacity and is always recorded.
//!
//! The module has no kernel dependency; the kernel side supplies the locks,
//! the counters and the direct map (`frame_allocator.rs`).

use super::boot_ranges::{RangeError, RangeSet, RangeStorage};
use arm64_boot_contract::phys_memory::PAGE_SIZE;
use core::ptr;
use core::sync::atomic::{AtomicU16, AtomicU32, AtomicU64, AtomicU8, Ordering};

pub const ST_NEVER: u32 = 0;
pub const ST_ALLOCATED: u32 = 1;
pub const ST_FREE: u32 = 2;
/// A frame that shares a metadata page with usable RAM but is not usable RAM
/// itself. `entry` returns `None` for it, so claims and returns refuse it as
/// untracked.
/// claim-lint:ok: tools/kmem-host-tests/tests/frame_meta.rs each_usable_frame_has_one_entry_and_holes_have_none.
const ST_UNTRACKED: u32 = 3;
pub const STATE_MASK: u32 = 0b11;
const GENERATION_MASK: u32 = 0x3fff_ffff;
/// `leaf_refs` of a frame whose mappings its owner holds outside process
/// page tables (`mark_external`).
pub const LEAF_EXTERNAL: u32 = u32::MAX;
const NO_FRAME: u64 = u64::MAX;

/// The state of one 4 KiB frame. The buddy order, zone and flag bytes of the
/// plan's layout are padding until the buddy allocator reads them (E1-8).
/// `next` and `slab` are the two words of the plan's `use` union: a free
/// frame uses the first, a slab page the second.
#[repr(C, align(32))]
pub struct FrameMeta {
    /// Two state bits and a 30-bit generation that only increases.
    state_gen: AtomicU32,
    /// Mapping references from process page tables, or `LEAF_EXTERNAL`.
    leaf_refs: AtomicU32,
    /// The allocation site of the current generation, or 0 when unknown.
    site: AtomicU16,
    /// The census owner the allocation named, or 0 to classify by site.
    owner: AtomicU8,
    /// The next frame on the free list, or `NO_FRAME`.
    next: AtomicU64,
    /// The descriptor of the slab this frame holds, or 0 (6.2).
    slab: AtomicU64,
}

const _: () = assert!(size_of::<FrameMeta>() == 32);

impl FrameMeta {
    pub fn state(&self) -> u32 {
        self.state_gen.load(Ordering::Acquire) & STATE_MASK
    }

    pub fn generation(&self) -> u32 {
        self.state_gen.load(Ordering::Acquire) >> 2
    }

    pub fn leaf_refs(&self) -> u32 {
        self.leaf_refs.load(Ordering::Acquire)
    }

    pub fn site(&self) -> u16 {
        self.site.load(Ordering::Relaxed)
    }

    pub fn owner(&self) -> u8 {
        self.owner.load(Ordering::Relaxed)
    }

    /// The descriptor address of the slab this frame holds, or 0.
    #[inline]
    pub fn slab(&self) -> u64 {
        self.slab.load(Ordering::Acquire)
    }

    /// Record the slab this allocated frame holds; 0 clears the record. The
    /// slab layer writes it under its cache lock, before an object of the
    /// slab is handed out, and clears it before the frame is returned.
    #[inline]
    pub fn set_slab(&self, descriptor: u64) {
        self.slab.store(descriptor, Ordering::Release);
    }
}

/// Entries in one metadata page, and the frames that page describes.
pub const ENTRIES_PER_PAGE: usize = PAGE_SIZE as usize / size_of::<FrameMeta>();
const ENTRY_BITS: u32 = ENTRIES_PER_PAGE.trailing_zeros();
/// A section is 2^15 frames (128 MiB).
const SECTION_BITS: u32 = 15;
const PAGES_PER_SECTION: usize = 1 << (SECTION_BITS - ENTRY_BITS);
const SECTIONS_PER_LEAF: usize = PAGE_SIZE as usize / size_of::<usize>();
/// A leaf page covers 2^24 frames (64 GiB).
const LEAF_BITS: u32 = SECTION_BITS + SECTIONS_PER_LEAF.trailing_zeros();
const LEAVES_PER_ROOT: usize = PAGE_SIZE as usize / size_of::<usize>();
/// A root page covers 2^33 frames (32 TiB).
const ROOT_BITS: u32 = LEAF_BITS + LEAVES_PER_ROOT.trailing_zeros();
/// Frame numbers below the 52-bit physical address limit, the largest that
/// x86-64 (MAXPHYADDR) and ARMv8 (PARange) define.
const PFN_BITS: u32 = 52 - PAGE_SIZE.trailing_zeros();
const TOP_LEN: usize = 1 << (PFN_BITS - ROOT_BITS);
const DESCRIPTORS_PER_PAGE: usize = PAGE_SIZE as usize / size_of::<Section>();

const _: () = assert!(ENTRIES_PER_PAGE.is_power_of_two() && ENTRY_BITS < SECTION_BITS);
const _: () = assert!(size_of::<Section>() <= PAGE_SIZE as usize);
const _: () = assert!(size_of::<Leaf>() == PAGE_SIZE as usize);
const _: () = assert!(size_of::<Root>() == PAGE_SIZE as usize);

type MetaPage = [FrameMeta; ENTRIES_PER_PAGE];

/// One pointer per metadata page of a section; null where that 512 KiB holds
/// no usable RAM.
#[repr(C)]
struct Section([*const MetaPage; PAGES_PER_SECTION]);

#[repr(C)]
struct Leaf([*const Section; SECTIONS_PER_LEAF]);

#[repr(C)]
struct Root([*const Leaf; LEAVES_PER_ROOT]);

/// What the boot carve spent on metadata, in pages.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MetaStats {
    pub metadata_pages: usize,
    pub sections: usize,
    pub descriptor_pages: usize,
    pub leaf_pages: usize,
    /// Root pages, one per 32 TiB that holds usable RAM.
    pub root_pages: usize,
    /// Pages the last carve took beyond what the final range set needs: at
    /// most one for each of the four levels a carved page can empty.
    pub spare_pages: usize,
}

impl MetaStats {
    pub fn carved_pages(&self) -> usize {
        self.metadata_pages + self.descriptor_pages + self.leaf_pages + self.root_pages
            + self.spare_pages
    }
}

/// The sparse metadata directory. It is built once on the boot CPU and is
/// read-only afterwards; the entries themselves are atomics.
pub struct FrameMetaMap {
    top: [*const Root; TOP_LEN],
    stats: MetaStats,
}

// `build` is the directory's only writer, and no code returns its pages.
unsafe impl Send for FrameMetaMap {}
unsafe impl Sync for FrameMetaMap {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReturnOutcome {
    Returned,
    RefusedDoubleRelease,
    RefusedStale,
    RefusedNeverAllocated,
    RefusedUntracked,
    RefusedLiveLeaf,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClaimError {
    /// The frame is allocated to another owner.
    Allocated,
    /// The frame is free but still mapped.
    LiveLeaf,
    /// The frame has no metadata.
    Untracked,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeafMappingClass {
    Owned,
    External,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Decref {
    /// That was the last mapping: the owner may return the frame.
    Last,
    Remaining,
    External,
    /// Untracked, not live, or no mapping to drop.
    Refused,
}

impl FrameMetaMap {
    pub const fn empty() -> Self {
        Self { top: [ptr::null(); TOP_LEN], stats: MetaStats {
            metadata_pages: 0, sections: 0, descriptor_pages: 0, leaf_pages: 0, root_pages: 0,
            spare_pages: 0,
        } }
    }

    pub fn stats(&self) -> MetaStats {
        self.stats
    }

    /// The entry of a usable frame, if the frame is usable RAM.
    /// claim-lint:ok: tools/kmem-host-tests/tests/frame_meta.rs each_usable_frame_has_one_entry_and_holes_have_none.
    #[inline]
    pub fn entry(&self, pfn: u64) -> Option<&FrameMeta> {
        let root = *self.top.get(usize::try_from(pfn >> ROOT_BITS).ok()?)?;
        if root.is_null() {
            return None;
        }
        // SAFETY: `build` wrote the directory before publication; a non-null
        // pointer names a page carved for that slot, which no code reuses.
        let meta = unsafe {
            let leaf = (*root).0[(pfn >> LEAF_BITS) as usize & (LEAVES_PER_ROOT - 1)];
            if leaf.is_null() {
                return None;
            }
            let section = (*leaf).0[(pfn >> SECTION_BITS) as usize & (SECTIONS_PER_LEAF - 1)];
            if section.is_null() {
                return None;
            }
            let page = (*section).0[(pfn >> ENTRY_BITS) as usize & (PAGES_PER_SECTION - 1)];
            if page.is_null() {
                return None;
            }
            &(*page)[pfn as usize & (ENTRIES_PER_PAGE - 1)]
        };
        (meta.state_gen.load(Ordering::Relaxed) & STATE_MASK != ST_UNTRACKED).then_some(meta)
    }

    /// Visit the usable frames' entries in ascending frame order.
    pub fn for_each(&self, mut visit: impl FnMut(u64, &FrameMeta)) {
        let leaves = self.top.iter().enumerate().filter(|(_, root)| !root.is_null()).flat_map(|(t, &root)| {
            // SAFETY: as in `entry`.
            unsafe { &(*root).0 }.iter().enumerate().map(move |(l, &leaf)| ((t << (ROOT_BITS - LEAF_BITS)) + l, leaf))
        });
        for (index, leaf) in leaves {
            if leaf.is_null() {
                continue;
            }
            for (s, &section) in unsafe { &(*leaf).0 }.iter().enumerate() {
                if section.is_null() {
                    continue;
                }
                for (p, &page) in unsafe { &(*section).0 }.iter().enumerate() {
                    if page.is_null() {
                        continue;
                    }
                    let base = (((index << (LEAF_BITS - SECTION_BITS)) + s) << (SECTION_BITS - ENTRY_BITS)
                        | p) << ENTRY_BITS;
                    for (e, meta) in unsafe { &*page }.iter().enumerate() {
                        if meta.state_gen.load(Ordering::Relaxed) & STATE_MASK != ST_UNTRACKED {
                            visit((base + e) as u64, meta);
                        }
                    }
                }
            }
        }
    }

    /// Claim a never-allocated or free frame: advance its generation and
    /// record who allocated it. Returns the new generation.
    pub fn claim(&self, pfn: u64, site: u16, owner: u8) -> Result<u32, ClaimError> {
        let meta = self.entry(pfn).ok_or(ClaimError::Untracked)?;
        loop {
            let observed = meta.state_gen.load(Ordering::Acquire);
            match observed & STATE_MASK {
                ST_NEVER | ST_FREE => {
                    if meta.leaf_refs.load(Ordering::Acquire) != 0 {
                        return Err(ClaimError::LiveLeaf);
                    }
                    let generation = (observed >> 2).wrapping_add(1) & GENERATION_MASK;
                    let allocated = (generation << 2) | ST_ALLOCATED;
                    if meta
                        .state_gen
                        .compare_exchange(observed, allocated, Ordering::AcqRel, Ordering::Acquire)
                        .is_ok()
                    {
                        meta.site.store(site, Ordering::Relaxed);
                        meta.owner.store(owner, Ordering::Relaxed);
                        return Ok(generation);
                    }
                }
                ST_ALLOCATED => return Err(ClaimError::Allocated),
                _ => return Err(ClaimError::Untracked),
            }
        }
    }

    /// The generation a frame carries now, or 0 for a frame with no entry.
    pub fn current_generation(&self, pfn: u64) -> u32 {
        self.entry(pfn).map_or(0, FrameMeta::generation)
    }

    /// Move an allocated frame to free, if `generation` is its current one and
    /// no mapping holds it. The caller puts a returned frame on its free list.
    pub fn retire(&self, pfn: u64, generation: u32) -> ReturnOutcome {
        let Some(meta) = self.entry(pfn) else {
            return ReturnOutcome::RefusedUntracked;
        };
        loop {
            let observed = meta.state_gen.load(Ordering::Acquire);
            if observed & STATE_MASK == ST_NEVER {
                return ReturnOutcome::RefusedNeverAllocated;
            }
            if observed >> 2 != generation {
                return ReturnOutcome::RefusedStale;
            }
            match observed & STATE_MASK {
                ST_ALLOCATED => {
                    if meta.leaf_refs.load(Ordering::Acquire) != 0 {
                        return ReturnOutcome::RefusedLiveLeaf;
                    }
                    let free = (observed & !STATE_MASK) | ST_FREE;
                    if meta
                        .state_gen
                        .compare_exchange(observed, free, Ordering::AcqRel, Ordering::Acquire)
                        .is_ok()
                    {
                        return ReturnOutcome::Returned;
                    }
                }
                ST_FREE => return ReturnOutcome::RefusedDoubleRelease,
                _ => return ReturnOutcome::RefusedUntracked,
            }
        }
    }

    /// Derive one mapping reference from allocator state before its
    /// descriptor is published. The caller rolls it back if mapping fails.
    pub fn acquire_leaf(&self, pfn: u64) -> Result<LeafMappingClass, &'static str> {
        let Some(meta) = self.entry(pfn) else {
            return Err("leaf frame is neither allocator-owned nor registered external memory");
        };
        if meta.state() != ST_ALLOCATED {
            return Err("leaf frame is not allocator-owned and live");
        }
        loop {
            let refs = meta.leaf_refs.load(Ordering::Acquire);
            if refs == LEAF_EXTERNAL {
                return Ok(LeafMappingClass::External);
            }
            if refs == LEAF_EXTERNAL - 1 {
                return Err("leaf mapping reference count exhausted");
            }
            if meta
                .leaf_refs
                .compare_exchange(refs, refs + 1, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                if meta.state() == ST_ALLOCATED {
                    return Ok(LeafMappingClass::Owned);
                }
                meta.leaf_refs.fetch_sub(1, Ordering::AcqRel);
                return Err("leaf frame changed allocator state during classification");
            }
        }
    }

    /// Release one mapping reference. Untracked, external and non-live
    /// frames fail closed: this operation returns `Refused` or `External`.
    /// claim-lint:ok: tools/kmem-host-tests/tests/frame_meta.rs lease_refusals_match_the_old_ledger_seed_5eede103.
    pub fn decref_leaf(&self, pfn: u64) -> Decref {
        let Some(meta) = self.entry(pfn) else {
            return Decref::Refused;
        };
        if meta.state() != ST_ALLOCATED {
            return Decref::Refused;
        }
        loop {
            let refs = meta.leaf_refs.load(Ordering::Acquire);
            if refs == LEAF_EXTERNAL {
                return Decref::External;
            }
            if refs == 0 {
                return Decref::Refused;
            }
            if meta
                .leaf_refs
                .compare_exchange(refs, refs - 1, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return if refs == 1 { Decref::Last } else { Decref::Remaining };
            }
        }
    }

    /// The mapping count of a frame, or 0 for one with no entry.
    pub fn leaf_refs(&self, pfn: u64) -> u32 {
        self.entry(pfn).map_or(0, FrameMeta::leaf_refs)
    }

    /// Mark a live frame as held outside process page tables. `Ok(false)`
    /// means the frame is not allocator memory.
    pub fn mark_external(&self, pfn: u64) -> Result<bool, &'static str> {
        let Some(meta) = self.entry(pfn) else {
            return Ok(false);
        };
        if meta.state() != ST_ALLOCATED {
            return Err("external leaf frame is not allocator-owned and live");
        }
        loop {
            let refs = meta.leaf_refs.load(Ordering::Acquire);
            if refs == LEAF_EXTERNAL {
                return Ok(true);
            }
            if refs != 0 {
                return Err("external leaf frame already has owned mappings");
            }
            if meta
                .leaf_refs
                .compare_exchange(0, LEAF_EXTERNAL, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return Ok(true);
            }
        }
    }

    /// Clear an external mark so the frame can be returned.
    pub fn clear_external(&self, pfn: u64) -> Result<(), ReturnOutcome> {
        let meta = self.entry(pfn).ok_or(ReturnOutcome::RefusedUntracked)?;
        meta.leaf_refs
            .compare_exchange(LEAF_EXTERNAL, 0, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| ())
            .map_err(|_| ReturnOutcome::RefusedLiveLeaf)
    }
}

/// The free frames, as a LIFO list threaded through their entries. The
/// caller serializes access.
pub struct FreeList {
    head: u64,
    len: usize,
}

impl FreeList {
    pub const fn new() -> Self {
        Self { head: NO_FRAME, len: 0 }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    /// Put a frame on the list. Fails only for a frame with no entry.
    pub fn push(&mut self, map: &FrameMetaMap, pfn: u64) -> bool {
        let Some(meta) = map.entry(pfn) else {
            return false;
        };
        meta.next.store(self.head, Ordering::Relaxed);
        self.head = pfn;
        self.len += 1;
        true
    }

    pub fn pop(&mut self, map: &FrameMetaMap) -> Option<u64> {
        if self.head == NO_FRAME {
            return None;
        }
        let pfn = self.head;
        let meta = map.entry(pfn)?;
        self.head = meta.next.swap(NO_FRAME, Ordering::Relaxed);
        self.len -= 1;
        Some(pfn)
    }
}

/// Carved pages not yet given a place in the directory, linked through
/// their first word.
struct Pool {
    head: *mut u8,
    count: usize,
}

impl Pool {
    fn take(&mut self) -> Result<*mut u8, RangeError> {
        if self.head.is_null() {
            return Err(RangeError::NoFit);
        }
        let page = self.head;
        // SAFETY: each pooled page is a carved, mapped, writable page.
        unsafe {
            self.head = page.cast::<*mut u8>().read();
            page.write_bytes(0, PAGE_SIZE as usize);
        }
        self.count -= 1;
        Ok(page)
    }
}

/// The directory a range set needs.
#[derive(Default)]
struct Shape {
    pages: usize,
    sections: usize,
    leaves: usize,
    roots: usize,
}

impl Shape {
    fn carved_pages(&self) -> usize {
        self.pages + self.sections.div_ceil(DESCRIPTORS_PER_PAGE) + self.leaves + self.roots
    }
}

fn distinct(count: &mut usize, last: &mut Option<u64>, first: u64, end: u64) {
    *count += (end - first + 1) as usize - usize::from(*last == Some(first));
    *last = Some(end);
}

fn shape(set: &RangeSet) -> Result<Shape, RangeError> {
    let mut shape = Shape::default();
    let (mut page, mut section, mut leaf, mut root) = (None, None, None, None);
    for range in set.iter() {
        let (first, last) = (range.start / PAGE_SIZE, range.end / PAGE_SIZE - 1);
        if last >> ROOT_BITS >= TOP_LEN as u64 {
            return Err(RangeError::AboveAddressLimit);
        }
        distinct(&mut shape.pages, &mut page, first >> ENTRY_BITS, last >> ENTRY_BITS);
        distinct(&mut shape.sections, &mut section, first >> SECTION_BITS, last >> SECTION_BITS);
        distinct(&mut shape.leaves, &mut leaf, first >> LEAF_BITS, last >> LEAF_BITS);
        distinct(&mut shape.roots, &mut root, first >> ROOT_BITS, last >> ROOT_BITS);
    }
    Ok(shape)
}

/// Carve the metadata for the frames of `set` out of `set` itself and build
/// the directory over what remains. Each page is carved from RAM that
/// `storage` maps writable; a frame carved for metadata has no entry.
pub fn build(set: &mut RangeSet, storage: &impl RangeStorage) -> Result<FrameMetaMap, RangeError> {
    // Carve everything first: the walk below then sees the final set. A carve
    // that empties a span, section, leaf or root lowers what the set needs, so
    // the pool grows until it covers the set as it is now. One carved page
    // lowers the need by at most 4, so a batch of a fifth of the shortfall
    // takes at most 4 pages beyond the final need.
    let mut pool = Pool { head: ptr::null_mut(), count: 0 };
    loop {
        let need = shape(set)?.carved_pages();
        if pool.count >= need {
            break;
        }
        for _ in 0..(need - pool.count).div_ceil(5) {
            let phys = set.carve(PAGE_SIZE, PAGE_SIZE, u64::MAX)?.start;
            let address = storage.storage_page(phys).ok_or(RangeError::NoMappedStorage)?.as_ptr();
            // SAFETY: a carved page the storage maps writable.
            unsafe { address.cast::<*mut u8>().write(pool.head) };
            pool.head = address;
            pool.count += 1;
        }
    }

    let mut top = [ptr::null::<Root>(); TOP_LEN];
    let mut stats = MetaStats::default();
    let mut descriptors: (*mut Section, usize) = (ptr::null_mut(), DESCRIPTORS_PER_PAGE);
    for range in set.iter() {
        let (first, last) = (range.start / PAGE_SIZE, range.end / PAGE_SIZE - 1);
        for chunk in first >> ENTRY_BITS..=last >> ENTRY_BITS {
            let pfn = chunk << ENTRY_BITS;
            // SAFETY: the directory pages below are carved for this build
            // and zeroed by `Pool::take`; the map is not published yet.
            unsafe {
                let root_slot = &mut top[(pfn >> ROOT_BITS) as usize];
                if root_slot.is_null() {
                    *root_slot = pool.take()?.cast::<Root>();
                    stats.root_pages += 1;
                }
                let root = root_slot.cast_mut();
                let leaf_slot = &mut (*root).0[(pfn >> LEAF_BITS) as usize & (LEAVES_PER_ROOT - 1)];
                if leaf_slot.is_null() {
                    *leaf_slot = pool.take()?.cast::<Leaf>();
                    stats.leaf_pages += 1;
                }
                let leaf = leaf_slot.cast_mut();
                let section_slot = &mut (*leaf).0[(pfn >> SECTION_BITS) as usize & (SECTIONS_PER_LEAF - 1)];
                if section_slot.is_null() {
                    if descriptors.1 == DESCRIPTORS_PER_PAGE {
                        descriptors = (pool.take()?.cast::<Section>(), 0);
                        stats.descriptor_pages += 1;
                    }
                    *section_slot = descriptors.0.add(descriptors.1);
                    descriptors.1 += 1;
                    stats.sections += 1;
                }
                let section = section_slot.cast_mut();
                let page_slot = &mut (*section).0[chunk as usize & (PAGES_PER_SECTION - 1)];
                if page_slot.is_null() {
                    let entries = pool.take()?.cast::<FrameMeta>();
                    for index in 0..ENTRIES_PER_PAGE {
                        entries.add(index).write(FrameMeta {
                            state_gen: AtomicU32::new(ST_UNTRACKED),
                            leaf_refs: AtomicU32::new(0),
                            site: AtomicU16::new(0),
                            owner: AtomicU8::new(0),
                            next: AtomicU64::new(NO_FRAME),
                            slab: AtomicU64::new(0),
                        });
                    }
                    *page_slot = entries.cast::<MetaPage>();
                    stats.metadata_pages += 1;
                }
                let entries = &**page_slot;
                let low = first.max(pfn) - pfn;
                let high = last.min(pfn + ENTRIES_PER_PAGE as u64 - 1) - pfn;
                for meta in &entries[low as usize..=high as usize] {
                    meta.state_gen.store(ST_NEVER, Ordering::Relaxed);
                }
            }
        }
    }
    stats.spare_pages = pool.count;
    Ok(FrameMetaMap { top, stats })
}

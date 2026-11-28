//! E1-3: frame metadata outside the heap, with the old ledger's lease rules.
use std::alloc::{alloc_zeroed, GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::ptr::NonNull;

use arm64_boot_contract::phys_memory::PhysRange;
use boot_ranges::{RangeSet, RangeStorage};

// The tests use the range set and the x86 builder, not the ARM64 map readers.
#[allow(dead_code, unused_imports)]
#[path = "../../../kernel/src/memory/boot_ranges.rs"]
mod boot_ranges;
// The census accessors and the external marks are the kernel's, not the lease rules'.
#[allow(dead_code)]
#[path = "../../../kernel/src/memory/frame_meta.rs"]
mod frame_meta;
#[path = "frame_meta/old_ledger.rs"]
mod old_ledger;

use frame_meta::{ClaimError, Decref, FrameMetaMap, FreeList, ReturnOutcome};

const PAGE: u64 = 4096;

/// The system allocator, refusing every request while this thread's flag is set.
struct FailingAllocator;

thread_local! {
    static REFUSE: Cell<bool> = const { Cell::new(false) };
}

unsafe impl GlobalAlloc for FailingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if REFUSE.with(Cell::get) {
            return std::ptr::null_mut();
        }
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: FailingAllocator = FailingAllocator;

fn without_heap<R>(f: impl FnOnce() -> R) -> R {
    REFUSE.with(|refuse| refuse.set(true));
    let result = f();
    REFUSE.with(|refuse| refuse.set(false));
    result
}

/// Simulated physical memory: each RAM range is backed up front, so nothing
/// allocates while the build runs.
struct Memory {
    ranges: Vec<(u64, u64, *mut u8)>,
}

impl Memory {
    fn new(ranges: &[(u64, u64)]) -> Self {
        let ranges = ranges
            .iter()
            .map(|&(start, end)| {
                let layout = Layout::from_size_align((end - start) as usize, PAGE as usize).unwrap();
                (start, end, unsafe { alloc_zeroed(layout) })
            })
            .collect();
        Self { ranges }
    }
}

unsafe impl RangeStorage for Memory {
    fn storage_page(&self, phys: u64) -> Option<NonNull<u8>> {
        let &(start, _, base) = self
            .ranges
            .iter()
            .find(|&&(start, end, _)| (start..end).contains(&phys))?;
        NonNull::new(unsafe { base.add((phys - start) as usize) })
    }
}

fn range_set(memory: &Memory) -> RangeSet {
    let usable = memory.ranges.iter().map(|&(start, end, _)| PhysRange::new(start, end));
    boot_ranges::from_x86(memory, usable.collect::<Vec<_>>().into_iter()).unwrap()
}

/// Outcomes common to both implementations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Outcome {
    Returned,
    LostContended,
    Double,
    Stale,
    Never,
    Untracked,
    LiveLeaf,
}

/// What a lease test needs from a frame allocator.
trait Subject {
    fn allocate(&mut self) -> Option<(u64, u32)>;
    /// An allocation made while this CPU holds the free list, as the old
    /// kernel's heap growth inside the capacity reservation did.
    fn allocate_nested(&mut self) -> Option<(u64, u32)>;
    fn ret(&mut self, frame: u64, generation: u32) -> Outcome;
    fn free_list_len(&self) -> usize;
    /// The free list's reserved capacity, where it has one.
    fn reserved_capacity(&self) -> Option<usize>;
    /// The generation a frame carries now (the lease `deallocate_frame` builds).
    fn generation(&self, frame: u64) -> u32;
    /// One mapping reference: `Ok(true)` for an external frame.
    fn acquire_leaf(&self, frame: u64) -> Result<bool, ()>;
    /// Drop one mapping reference; true when it was the last.
    fn decref(&self, frame: u64) -> bool;
    fn refcount(&self, frame: u64) -> u32;
}

/// The new metadata with the kernel's glue: a LIFO free list in front of the
/// sequential frontier, and a candidate the metadata refuses is skipped.
struct NewMeta<'a> {
    set: &'a RangeSet,
    map: FrameMetaMap,
    free: FreeList,
    frontier: usize,
}

impl NewMeta<'_> {
    /// `BootInfoFrameAllocator::get_usable_frame`.
    fn usable_frame(&self, n: usize) -> Option<u64> {
        let mut count = 0;
        for range in self.set.iter() {
            let frames = ((range.end - range.start) / PAGE) as usize;
            if count + frames > n {
                return Some(range.start + (n - count) as u64 * PAGE);
            }
            count += frames;
        }
        None
    }

    fn candidate(&mut self) -> Option<u64> {
        if let Some(pfn) = self.free.pop(&self.map) {
            return Some(pfn * PAGE);
        }
        let frame = self.usable_frame(self.frontier)?;
        self.frontier += 1;
        Some(frame)
    }
}

impl Subject for NewMeta<'_> {
    fn allocate(&mut self) -> Option<(u64, u32)> {
        loop {
            let frame = self.candidate()?;
            match self.map.claim(frame / PAGE, 1, 0) {
                Ok(generation) => return Some((frame, generation)),
                Err(ClaimError::Allocated | ClaimError::LiveLeaf | ClaimError::Untracked) => {}
            }
        }
    }

    /// The free list needs no reservation, so nothing allocates under its
    /// hold and an allocation cannot nest inside it.
    fn allocate_nested(&mut self) -> Option<(u64, u32)> {
        self.allocate()
    }

    fn ret(&mut self, frame: u64, generation: u32) -> Outcome {
        let outcome = self.map.retire(frame / PAGE, generation);
        if outcome == ReturnOutcome::Returned {
            assert!(self.free.push(&self.map, frame / PAGE));
        }
        match outcome {
            ReturnOutcome::Returned => Outcome::Returned,
            ReturnOutcome::RefusedDoubleRelease => Outcome::Double,
            ReturnOutcome::RefusedStale => Outcome::Stale,
            ReturnOutcome::RefusedNeverAllocated => Outcome::Never,
            ReturnOutcome::RefusedUntracked => Outcome::Untracked,
            ReturnOutcome::RefusedLiveLeaf => Outcome::LiveLeaf,
        }
    }

    fn free_list_len(&self) -> usize {
        self.free.len()
    }

    fn reserved_capacity(&self) -> Option<usize> {
        None
    }

    fn generation(&self, frame: u64) -> u32 {
        self.map.current_generation(frame / PAGE)
    }

    fn acquire_leaf(&self, frame: u64) -> Result<bool, ()> {
        self.map
            .acquire_leaf(frame / PAGE)
            .map(|class| class == frame_meta::LeafMappingClass::External)
            .map_err(|_| ())
    }

    fn decref(&self, frame: u64) -> bool {
        self.map.decref_leaf(frame / PAGE) == Decref::Last
    }

    fn refcount(&self, frame: u64) -> u32 {
        self.map.leaf_refs(frame / PAGE)
    }
}

fn new_meta<'a>(set: &'a RangeSet, map: FrameMetaMap) -> NewMeta<'a> {
    NewMeta { set, map, free: FreeList::new(), frontier: 0 }
}

impl Subject for old_ledger::OldLedger<'_> {
    fn allocate(&mut self) -> Option<(u64, u32)> {
        self.allocate_frame_leased().map(|lease| (lease.frame, lease.generation))
    }

    fn allocate_nested(&mut self) -> Option<(u64, u32)> {
        self.held = true;
        let lease = self.allocate();
        self.held = false;
        lease
    }

    fn ret(&mut self, frame: u64, generation: u32) -> Outcome {
        let mut lease = self.current_lease_for_frame(frame);
        lease.generation = generation;
        use old_ledger::ReturnOutcome as R;
        match self.return_lease(lease) {
            R::Returned => Outcome::Returned,
            R::LostContended => Outcome::LostContended,
            R::RefusedDoubleRelease => Outcome::Double,
            R::RefusedStale => Outcome::Stale,
            R::RefusedNeverAllocated => Outcome::Never,
            R::RefusedUntracked => Outcome::Untracked,
            R::RefusedLiveLeaf => Outcome::LiveLeaf,
        }
    }

    fn free_list_len(&self) -> usize {
        self.free_list_len()
    }

    fn reserved_capacity(&self) -> Option<usize> {
        Some(self.reserved_capacity())
    }

    fn generation(&self, frame: u64) -> u32 {
        self.current_lease_for_frame(frame).generation
    }

    fn acquire_leaf(&self, frame: u64) -> Result<bool, ()> {
        self.acquire_leaf_mapping(frame).map_err(|_| ())
    }

    fn decref(&self, frame: u64) -> bool {
        self.decref_leaf_mapping(frame)
    }

    fn refcount(&self, frame: u64) -> u32 {
        self.leaf_mapping_refcount(frame)
    }
}

/// The implementation the check-first tests judge.
fn subject<'a>(set: &'a mut RangeSet, memory: &Memory) -> Result<impl Subject + 'a, ()> {
    let map = frame_meta::build(set, memory).map_err(|_| ())?;
    Ok(NewMeta { set, map, free: FreeList::new(), frontier: 0 })
}

const SMALL: [(u64, u64); 2] = [(0x10_0000, 0x40_0000), (0x100_0000, 0x180_0000)];

/// Check first: a return that finds the free list's reserved capacity exactly
/// full was dropped as `LostContended` and the frame lost; each return must be
/// recorded.
#[test]
fn a_return_at_exact_free_list_capacity_is_recorded() {
    let memory = Memory::new(&SMALL);
    let mut set = range_set(&memory);
    let mut subject = subject(&mut set, &memory).expect("subject");
    let mut leases = vec![subject.allocate().expect("first frame")];
    let capacity = subject.reserved_capacity().unwrap_or(4);
    while leases.len() < capacity {
        leases.push(subject.allocate().expect("frame within capacity"));
    }
    leases.push(subject.allocate_nested().expect("frame past capacity"));
    let outcomes: Vec<_> = leases.iter().map(|&(frame, generation)| subject.ret(frame, generation)).collect();
    assert_eq!(outcomes, vec![Outcome::Returned; leases.len()]);
    assert_eq!(subject.free_list_len(), leases.len());
}

/// Check first: the old ledger's chunks came from the heap, so building it
/// with a failing allocator failed. The metadata comes from carved ranges.
#[test]
fn frame_metadata_builds_without_a_heap() {
    let memory = Memory::new(&SMALL);
    let mut set = range_set(&memory);
    let built = without_heap(|| subject(&mut set, &memory).map(|_| ()));
    assert_eq!(built, Ok(()));
}

/// xorshift64, so each sequence is fixed by its seed.
struct Rng(u64);

impl Rng {
    fn below(&mut self, bound: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % bound as u64) as usize
    }
}

/// One fixed sequence of claims, returns, stale and double returns,
/// never-allocated and untracked returns, and leaf references, run against the
/// extracted old ledger and the new metadata over the same usable frames.
/// Each step must give both the same frame, generation, outcome and count,
/// and a frame's generation must only increase.
#[test]
fn lease_refusals_match_the_old_ledger_seed_5eede103() {
    let memory = Memory::new(&SMALL);
    let mut set = range_set(&memory);
    let map = frame_meta::build(&mut set, &memory).expect("frame metadata");
    let set = &set;
    let mut new = new_meta(set, map);
    let mut old = old_ledger::OldLedger::new(set);
    old.init_frame_ledger().expect("old ledger");

    let top = set.iter().last().unwrap().end;
    // A carved metadata page, a hole between the ranges, and the page above RAM.
    let untracked = [top, 0x80_0000, SMALL[1].1 - PAGE];
    assert!(set.iter().all(|range| !untracked.iter().any(|&frame| (range.start..range.end).contains(&frame))));

    let mut rng = Rng(0x5eed_e103);
    let mut live: Vec<(u64, u32)> = Vec::new();
    let mut retired: Vec<(u64, u32)> = Vec::new();
    let mut newest = std::collections::HashMap::<u64, u32>::new();
    let mut steps = [0usize; 8];
    for step in 0..6000 {
        let op = rng.below(8);
        let ran = match op {
            0 | 1 if live.len() < 48 => {
                let (a, b) = (old.allocate(), new.allocate());
                assert_eq!(a, b, "step {step}: allocation");
                let (frame, generation) = a.expect("allocation");
                let previous = newest.insert(frame, generation).unwrap_or(0);
                assert!(generation > previous, "step {step}: generation went back");
                live.push((frame, generation));
                true
            }
            2 | 3 if !live.is_empty() => {
                let (frame, generation) = live.swap_remove(rng.below(live.len()));
                assert_eq!(old.ret(frame, generation), new.ret(frame, generation), "step {step}");
                retired.push((frame, generation));
                true
            }
            4 if !retired.is_empty() => {
                let (frame, generation) = retired[rng.below(retired.len())];
                let outcome = old.ret(frame, generation);
                assert!(matches!(outcome, Outcome::Stale | Outcome::Double), "step {step}: {outcome:?}");
                assert_eq!(outcome, new.ret(frame, generation), "step {step}");
                true
            }
            5 => {
                let frame = if rng.below(2) == 0 {
                    untracked[rng.below(untracked.len())]
                } else {
                    new.usable_frame(new.frontier + 1 + rng.below(32)).unwrap()
                };
                let generation = old.generation(frame);
                assert_eq!(generation, new.generation(frame), "step {step}");
                let outcome = old.ret(frame, generation);
                assert!(matches!(outcome, Outcome::Untracked | Outcome::Never), "step {step}: {outcome:?}");
                assert_eq!(outcome, new.ret(frame, generation), "step {step}");
                assert_eq!(old.acquire_leaf(frame), new.acquire_leaf(frame), "step {step}");
                assert_eq!(old.refcount(frame), new.refcount(frame), "step {step}");
                true
            }
            6 if !live.is_empty() => {
                let (frame, generation) = live[rng.below(live.len())];
                assert_eq!(old.acquire_leaf(frame), Ok(false), "step {step}");
                assert_eq!(new.acquire_leaf(frame), Ok(false), "step {step}");
                assert_eq!(old.refcount(frame), new.refcount(frame), "step {step}");
                let outcome = old.ret(frame, generation);
                assert_eq!(outcome, Outcome::LiveLeaf, "step {step}");
                assert_eq!(outcome, new.ret(frame, generation), "step {step}");
                assert_eq!(old.decref(frame), new.decref(frame), "step {step}");
                assert_eq!(old.decref(frame), new.decref(frame), "step {step}: decref past zero");
                true
            }
            7 if !retired.is_empty() => {
                // Free, or allocated again under a newer generation.
                let (frame, _) = retired[rng.below(retired.len())];
                let acquired = old.acquire_leaf(frame);
                assert_eq!(acquired, new.acquire_leaf(frame), "step {step}");
                assert_eq!(old.refcount(frame), new.refcount(frame), "step {step}");
                if acquired.is_ok() {
                    assert_eq!(old.decref(frame), new.decref(frame), "step {step}");
                }
                true
            }
            _ => false,
        };
        steps[op] += usize::from(ran);
        assert_eq!(old.free_list_len(), new.free_list_len(), "step {step}: free list");
    }
    // The sequence reached every kind of step, and reused frames.
    assert!(steps.iter().all(|&count| count > 300), "{steps:?}");
    assert!(newest.values().any(|&generation| generation > 2));
}

/// Each usable frame resolves to exactly one entry, and each frame that is
/// not usable RAM (a one-page hole inside a metadata page, the space between
/// ranges, a carved metadata page, and above the top) has none.
#[test]
fn each_usable_frame_has_one_entry_and_holes_have_none() {
    const MAP: [(u64, u64); 4] = [
        (0x10_0000, 0x30_0000),
        (0x30_1000, 0x50_0000),
        (0x820_0000, 0x8a0_0000),
        (0x11_8000_0000, 0x11_8020_0000),
    ];
    let memory = Memory::new(&MAP);
    let mut set = range_set(&memory);
    let before: Vec<u64> = set.iter().flat_map(|range| range.start / PAGE..range.end / PAGE).collect();
    let map = frame_meta::build(&mut set, &memory).expect("frame metadata");

    let mut usable = Vec::new();
    for range in set.iter() {
        usable.extend((range.start / PAGE..range.end / PAGE).collect::<Vec<_>>());
    }
    let mut entries = std::collections::HashSet::new();
    for &pfn in &usable {
        let entry = map.entry(pfn).unwrap_or_else(|| panic!("pfn {pfn:#x} has no entry"));
        assert!(entries.insert(entry as *const _ as usize), "pfn {pfn:#x} shares an entry");
    }
    let mut walked = Vec::new();
    map.for_each(|pfn, _| walked.push(pfn));
    assert_eq!(walked, usable);

    let carved: Vec<u64> = before.into_iter().filter(|pfn| !usable.contains(pfn)).collect();
    assert_eq!(carved.len(), map.stats().carved_pages());
    let holes = [0x30_0000 / PAGE, 0x50_0000 / PAGE, 0x81f_f000 / PAGE, 0x8a0_0000 / PAGE,
        0x2_0000_0000 / PAGE, 0x11_8020_0000 / PAGE, 0x4000_0000_0000 / PAGE, u64::MAX / PAGE];
    for &pfn in holes.iter().chain(&carved) {
        assert!(map.entry(pfn).is_none(), "pfn {pfn:#x} is not usable RAM but has an entry");
        assert_eq!(map.claim(pfn, 1, 0), Err(ClaimError::Untracked));
        assert_eq!(map.retire(pfn, 0), ReturnOutcome::RefusedUntracked);
    }
}

/// 64 ranges of 768 KiB, spread across 1 TiB. A layout that needs a
/// contiguous 1 MiB array per section fits in none of them. The page-granular
/// layout places the metadata in at most 192 pages, with a 128-byte root, at
/// most 16 leaf pages and at most 64 descriptors, under 200 KiB together.
#[test]
fn sparse_ranges_across_a_terabyte_need_no_large_run() {
    let ranges: Vec<(u64, u64)> = (0..64u64)
        .map(|i| {
            let start = (1 << 30) + i * (16 << 30) + (i % 3) * (128 << 10);
            (start, start + (768 << 10))
        })
        .collect();
    assert!(ranges.iter().all(|&(start, end)| end - start < 1 << 20));
    assert!(ranges.last().unwrap().1 < 1 << 40);
    let memory = Memory::new(&ranges);
    let mut set = range_set(&memory);
    let map = frame_meta::build(&mut set, &memory).expect("frame metadata");
    let stats = map.stats();

    assert!(stats.metadata_pages <= 192, "{stats:?}");
    assert_eq!((stats.root_entries * 8, stats.root_pages), (128, 1));
    assert!(stats.leaf_pages <= 16 && stats.sections <= 64, "{stats:?}");
    let directory = (stats.root_pages + stats.leaf_pages + stats.descriptor_pages) as u64 * PAGE;
    assert!(directory < 200 << 10, "{stats:?}");

    for range in set.iter() {
        for pfn in range.start / PAGE..range.end / PAGE {
            assert!(map.entry(pfn).is_some(), "pfn {pfn:#x}");
        }
        for pfn in [range.start / PAGE - 1, range.end / PAGE, (range.end + (1 << 30)) / PAGE] {
            assert!(map.entry(pfn).is_none(), "pfn {pfn:#x}");
        }
    }
}

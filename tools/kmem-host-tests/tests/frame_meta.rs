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
#[path = "frame_meta/old_ledger.rs"]
mod old_ledger;

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
}

/// The implementation the check-first tests judge.
fn subject<'a>(set: &'a mut RangeSet, _memory: &Memory) -> Result<impl Subject + 'a, ()> {
    let mut ledger = old_ledger::OldLedger::new(set);
    ledger.init_frame_ledger()?;
    Ok(ledger)
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

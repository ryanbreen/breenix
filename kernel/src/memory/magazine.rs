//! Per-CPU magazines and the depot in front of the slab caches (E1 plan,
//! 6.3 and 5.4a), after J. Bonwick and J. Adams, "Magazines and Vmem:
//! Extending the Slab Allocator to Many CPUs and Arbitrary Resources",
//! USENIX Annual Technical Conference 2001.
//!
//! Each CPU holds, for each size class, a loaded and a previous magazine.
//! Allocation pops from the loaded one; when it is empty and the previous
//! one holds rounds, the two swap; otherwise the CPU trades its empty
//! magazine for a filled one at the class's depot, and when the depot has
//! 0 filled magazines it fills a magazine from the slabs. Free is the mirror image. The fast path
//! takes only its own CPU's lock, which another CPU takes only to drain it.
//!
//! CPUs are found through a slot table sized at boot from the CPU count the
//! firmware describes, not from a constant. A slot starts null. A CPU whose
//! slot is null, or that cannot name itself yet, takes the shared path: the
//! class's slabs under their lock. A CPU publishes its own slot once its
//! per-CPU id is readable, and only after that does any CPU touch it.
//!
//! Each depot keeps Bonwick and Adams's working set: the fewest filled and
//! empty magazines it held during each 15-second interval were not needed,
//! and the next exchange after the interval returns them. The magazine size
//! starts at 15 rounds and grows a step when the depot lock was contended
//! during the interval.
//!
//! The operations here run with the calling CPU's interrupts masked; the
//! kernel's caller (`memory/kmem.rs`) masks them, so the CPU neither
//! migrates nor reenters its own lock from an interrupt handler.

use super::slab_cache::{Backing, CacheStats, Short, SlabCache, SpinLock, CLASSES, CLASS_COUNT};
use core::ptr::{self, NonNull};
use core::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};

/// The magazine sizes in rounds; a depot starts at the first.
pub const ROUNDS: [usize; 4] = [15, 31, 63, 127];
const HEADER: usize = 16;
/// The working set's update interval (Bonwick and Adams use 15 seconds).
pub const UPDATE_MS: u64 = 15_000;
/// Contended depot acquisitions within one interval that grow the magazine.
pub const CONTENTION_LIMIT: u64 = 16;
/// Surplus magazines of each kind one update returns, so an exchange's work
/// stays bounded; the rest stay unused and are counted again next interval.
const TRIM_BATCH: usize = 8;

const fn magazine_bytes(rounds: usize) -> usize {
    HEADER + rounds * core::mem::size_of::<*mut u8>()
}

/// A magazine: a header, then `capacity` object pointers.
#[repr(C)]
struct Magazine {
    next: *mut Magazine,
    rounds: u32,
    capacity: u32,
}

const _: () = assert!(core::mem::size_of::<Magazine>() == HEADER);

unsafe fn round(magazine: *mut Magazine, index: usize) -> *mut *mut u8 {
    unsafe { magazine.cast::<u8>().add(HEADER).cast::<*mut u8>().add(index) }
}

unsafe fn rounds(magazine: *mut Magazine) -> usize {
    if magazine.is_null() {
        0
    } else {
        unsafe { (*magazine).rounds as usize }
    }
}

unsafe fn has_room(magazine: *mut Magazine) -> bool {
    !magazine.is_null() && unsafe { (*magazine).rounds < (*magazine).capacity }
}

unsafe fn pop(magazine: *mut Magazine) -> NonNull<u8> {
    unsafe {
        (*magazine).rounds -= 1;
        NonNull::new_unchecked(round(magazine, (*magazine).rounds as usize).read())
    }
}

unsafe fn push(magazine: *mut Magazine, object: NonNull<u8>) {
    unsafe {
        round(magazine, (*magazine).rounds as usize).write(object.as_ptr());
        (*magazine).rounds += 1;
    }
}

/// A singly linked stack of magazines.
struct Stack {
    head: *mut Magazine,
    len: usize,
}

impl Stack {
    const fn new() -> Self {
        Self { head: ptr::null_mut(), len: 0 }
    }

    unsafe fn push(&mut self, magazine: *mut Magazine) {
        unsafe { (*magazine).next = self.head };
        self.head = magazine;
        self.len += 1;
    }

    unsafe fn pop(&mut self) -> *mut Magazine {
        let magazine = self.head;
        if !magazine.is_null() {
            self.head = unsafe { (*magazine).next };
            self.len -= 1;
        }
        magazine
    }
}

struct DepotState {
    filled: Stack,
    empty: Stack,
    /// The fewest filled and empty magazines held since the last update.
    filled_min: usize,
    empty_min: usize,
    updated_at: u64,
    contended_at: u64,
}

impl DepotState {
    unsafe fn pop_filled(&mut self) -> *mut Magazine {
        let magazine = unsafe { self.filled.pop() };
        self.filled_min = self.filled_min.min(self.filled.len);
        magazine
    }

    unsafe fn pop_empty(&mut self) -> *mut Magazine {
        let magazine = unsafe { self.empty.pop() };
        self.empty_min = self.empty_min.min(self.empty.len);
        magazine
    }

    /// At the first exchange after an interval: detach the working set's
    /// surplus, grow the magazine size (`size`, an index in `ROUNDS`) if the
    /// lock was contended, and start the next interval.
    unsafe fn update(&mut self, now: u64, contentions: u64, size: &AtomicUsize) -> Surplus {
        let mut surplus = Surplus::new();
        if now.wrapping_sub(self.updated_at) < UPDATE_MS {
            return surplus;
        }
        for _ in 0..self.filled_min.min(TRIM_BATCH) {
            unsafe { surplus.filled.push(self.filled.pop()) };
        }
        for _ in 0..self.empty_min.min(TRIM_BATCH) {
            unsafe { surplus.empty.push(self.empty.pop()) };
        }
        let current = size.load(Ordering::Relaxed);
        if contentions.wrapping_sub(self.contended_at) > CONTENTION_LIMIT && current + 1 < ROUNDS.len() {
            size.store(current + 1, Ordering::Relaxed);
        }
        self.contended_at = contentions;
        self.filled_min = self.filled.len;
        self.empty_min = self.empty.len;
        self.updated_at = now;
        surplus
    }

    /// Detach the filled and the empty stacks (the shrinker).
    fn take_all(&mut self) -> Surplus {
        let surplus = Surplus {
            filled: core::mem::replace(&mut self.filled, Stack::new()),
            empty: core::mem::replace(&mut self.empty, Stack::new()),
        };
        self.filled_min = 0;
        self.empty_min = 0;
        surplus
    }
}

/// Magazines detached from a depot, returned outside its lock.
struct Surplus {
    filled: Stack,
    empty: Stack,
}

impl Surplus {
    const fn new() -> Self {
        Self { filled: Stack::new(), empty: Stack::new() }
    }
}

/// One class's depot of filled and empty magazines.
pub struct Depot {
    state: SpinLock<DepotState>,
    /// Rounds in the depot's filled magazines.
    rounds: AtomicUsize,
    /// The index in `ROUNDS` of the size new magazines get; written under
    /// the lock.
    size: AtomicUsize,
}

impl Depot {
    const fn new() -> Self {
        Self {
            state: SpinLock::new(DepotState {
                filled: Stack::new(),
                empty: Stack::new(),
                filled_min: 0,
                empty_min: 0,
                updated_at: 0,
                contended_at: 0,
            }),
            rounds: AtomicUsize::new(0),
            size: AtomicUsize::new(0),
        }
    }

    unsafe fn push(&self, state: &mut DepotState, magazine: *mut Magazine) {
        let held = unsafe { rounds(magazine) };
        if held == 0 {
            unsafe { state.empty.push(magazine) };
        } else {
            self.rounds.fetch_add(held, Ordering::Relaxed);
            unsafe { state.filled.push(magazine) };
        }
    }
}

#[derive(Clone, Copy)]
struct Loaded {
    loaded: *mut Magazine,
    previous: *mut Magazine,
}

/// One CPU's magazines, a pair per class, under that CPU's lock.
pub struct CpuCaches {
    lock: SpinLock<[Loaded; CLASS_COUNT]>,
}

impl CpuCaches {
    pub const fn new() -> Self {
        Self {
            lock: SpinLock::new(
                [Loaded { loaded: ptr::null_mut(), previous: ptr::null_mut() }; CLASS_COUNT],
            ),
        }
    }

    /// Acquisitions of this CPU's lock, for the host model.
    pub fn acquisitions(&self) -> u64 {
        self.lock.acquisitions()
    }
}

impl Default for CpuCaches {
    fn default() -> Self {
        Self::new()
    }
}

/// One class's counts, as `/proc/breenix/kmem` reads them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClassStats {
    pub slabs: CacheStats,
    /// Rounds in the depot's filled magazines.
    pub depot_rounds: usize,
    /// Rounds in the CPUs' magazines, of the CPUs whose lock was free.
    pub cpu_rounds: usize,
    /// Rounds each new magazine of this class holds.
    pub magazine_rounds: usize,
}

/// The slab caches of each size class, their depots, the descriptor and
/// magazine caches, and the per-CPU slot table.
pub struct Kmem<B: Backing> {
    backing: B,
    classes: [SlabCache; CLASS_COUNT],
    depots: [Depot; CLASS_COUNT],
    descriptors: SlabCache,
    /// One cache per magazine size. These have no magazine layer of their
    /// own, which ends the recursion.
    magazines: [SlabCache; ROUNDS.len()],
    slots: AtomicPtr<AtomicPtr<CpuCaches>>,
    slot_count: AtomicUsize,
}

impl<B: Backing> Kmem<B> {
    pub const fn new(backing: B) -> Self {
        let mut classes = [const { SlabCache::new("", 16) }; CLASS_COUNT];
        let mut index = 0;
        while index < CLASS_COUNT {
            classes[index] = SlabCache::new("size_class", CLASSES[index]);
            index += 1;
        }
        let mut magazines = [const { SlabCache::new("", 16) }; ROUNDS.len()];
        let mut index = 0;
        while index < ROUNDS.len() {
            magazines[index] = SlabCache::new("magazine", magazine_bytes(ROUNDS[index]));
            index += 1;
        }
        Self {
            backing,
            classes,
            depots: [const { Depot::new() }; CLASS_COUNT],
            descriptors: SlabCache::descriptors(),
            magazines,
            slots: AtomicPtr::new(ptr::null_mut()),
            slot_count: AtomicUsize::new(0),
        }
    }

    pub fn backing(&self) -> &B {
        &self.backing
    }

    /// The descriptor cache, which named caches outside this set share.
    pub fn descriptors(&self) -> &SlabCache {
        &self.descriptors
    }

    /// Install the slot table, once: one null slot per CPU the firmware
    /// describes. Returns false if a table is already installed.
    pub fn install_slots(&self, table: &'static [AtomicPtr<CpuCaches>]) -> bool {
        if !self.slots.load(Ordering::Acquire).is_null() {
            return false;
        }
        self.slot_count.store(table.len(), Ordering::Relaxed);
        self.slots
            .compare_exchange(
                ptr::null_mut(),
                table.as_ptr() as *mut AtomicPtr<CpuCaches>,
                Ordering::Release,
                Ordering::Relaxed,
            )
            .is_ok()
    }

    pub fn slot_count(&self) -> usize {
        if self.slots.load(Ordering::Acquire).is_null() {
            0
        } else {
            self.slot_count.load(Ordering::Relaxed)
        }
    }

    fn slot(&self, cpu: usize) -> Option<&CpuCaches> {
        let table = self.slots.load(Ordering::Acquire);
        if table.is_null() || cpu >= self.slot_count.load(Ordering::Relaxed) {
            return None;
        }
        // SAFETY: the table has `slot_count` entries, and a published block
        // lives for the kernel's lifetime.
        unsafe { (*table.add(cpu)).load(Ordering::Acquire).as_ref() }
    }

    /// Publish `caches` as CPU `cpu`'s, with a release store. Called by that
    /// CPU once its per-CPU id is readable and before it enables interrupts.
    /// Returns false when the table has no slot for `cpu` or the slot is
    /// taken; the CPU then stays on the shared path.
    pub fn register(&self, cpu: usize, caches: &'static CpuCaches) -> bool {
        let table = self.slots.load(Ordering::Acquire);
        if table.is_null() || cpu >= self.slot_count.load(Ordering::Relaxed) {
            return false;
        }
        // SAFETY: as in `slot`.
        unsafe {
            (*table.add(cpu))
                .compare_exchange(
                    ptr::null_mut(),
                    caches as *const CpuCaches as *mut CpuCaches,
                    Ordering::Release,
                    Ordering::Relaxed,
                )
                .is_ok()
        }
    }

    /// One object of `class` for CPU `cpu`, which is absent for a CPU that
    /// cannot name itself. With `wait` false each lock is taken with one
    /// `try_lock`.
    pub fn alloc(&self, class: usize, cpu: Option<usize>, wait: bool) -> Result<NonNull<u8>, Short> {
        let Some(caches) = cpu.and_then(|cpu| self.slot(cpu)) else {
            return self.take_one(class, wait);
        };
        let mut pairs = caches.lock.lock_if(wait).ok_or(Short::Busy)?;
        let pair = &mut pairs[class];
        // SAFETY: the pair's magazines are this CPU's, under its lock.
        unsafe {
            if rounds(pair.loaded) > 0 {
                return Ok(pop(pair.loaded));
            }
            if rounds(pair.previous) > 0 {
                core::mem::swap(&mut pair.loaded, &mut pair.previous);
                return Ok(pop(pair.loaded));
            }
            self.refill(class, pair, wait)
        }
    }

    /// The loaded and previous magazines hold no rounds: trade at the depot,
    /// or fill a magazine from the slabs.
    unsafe fn refill(&self, class: usize, pair: &mut Loaded, wait: bool) -> Result<NonNull<u8>, Short> {
        let depot = &self.depots[class];
        let mut state = depot.state.lock_if(wait).ok_or(Short::Busy)?;
        let surplus = unsafe { state.update(self.backing.now_ms(), depot.state.contentions(), &depot.size) };
        let filled = unsafe { state.pop_filled() };
        if !filled.is_null() {
            depot.rounds.fetch_sub(unsafe { rounds(filled) }, Ordering::Relaxed);
            if !pair.previous.is_null() {
                unsafe { state.empty.push(pair.previous) };
            }
            pair.previous = pair.loaded;
            pair.loaded = filled;
        }
        let size = depot.size.load(Ordering::Relaxed);
        drop(state);
        unsafe { self.retire(class, surplus) };
        if !filled.is_null() {
            return Ok(unsafe { pop(pair.loaded) });
        }
        if pair.loaded.is_null() {
            core::mem::swap(&mut pair.loaded, &mut pair.previous);
        }
        if pair.loaded.is_null() {
            match self.new_magazine(size, wait) {
                Some(magazine) => pair.loaded = magazine,
                None => return self.take_one(class, wait),
            }
        }
        let magazine = pair.loaded;
        // SAFETY: the magazine is this CPU's and holds no rounds.
        unsafe {
            let capacity = (*magazine).capacity as usize;
            let out = core::slice::from_raw_parts_mut(round(magazine, 0), capacity);
            let taken = self.classes[class].take(out, wait, &self.backing, &self.descriptors)?;
            (*magazine).rounds = taken as u32;
            Ok(pop(magazine))
        }
    }

    fn take_one(&self, class: usize, wait: bool) -> Result<NonNull<u8>, Short> {
        let mut one = [ptr::null_mut(); 1];
        self.classes[class].take(&mut one, wait, &self.backing, &self.descriptors)?;
        // SAFETY: `take` filled the slot it reported.
        Ok(unsafe { NonNull::new_unchecked(one[0]) })
    }

    fn new_magazine(&self, size: usize, wait: bool) -> Option<*mut Magazine> {
        let mut one = [ptr::null_mut(); 1];
        self.magazines[size].take(&mut one, wait, &self.backing, &self.descriptors).ok()?;
        let magazine = one[0].cast::<Magazine>();
        // SAFETY: the object is a magazine-sized block of this cache.
        unsafe { magazine.write(Magazine { next: ptr::null_mut(), rounds: 0, capacity: ROUNDS[size] as u32 }) };
        Some(magazine)
    }

    unsafe fn free_magazine(&self, magazine: *mut Magazine) {
        let capacity = unsafe { (*magazine).capacity } as usize;
        let size = ROUNDS.iter().position(|&rounds| rounds == capacity).expect("magazine size");
        self.magazines[size].put(&[magazine.cast()], &self.backing, &self.descriptors);
    }

    /// Return detached magazines' rounds to the slabs, and the magazines to
    /// their caches.
    unsafe fn retire(&self, class: usize, mut surplus: Surplus) {
        loop {
            let magazine = unsafe { surplus.filled.pop() };
            if magazine.is_null() {
                break;
            }
            // SAFETY: a detached magazine is the caller's.
            unsafe {
                let held = rounds(magazine);
                self.depots[class].rounds.fetch_sub(held, Ordering::Relaxed);
                let objects = core::slice::from_raw_parts(round(magazine, 0), held);
                self.classes[class].put(objects, &self.backing, &self.descriptors);
                self.free_magazine(magazine);
            }
        }
        loop {
            let magazine = unsafe { surplus.empty.pop() };
            if magazine.is_null() {
                break;
            }
            unsafe { self.free_magazine(magazine) };
        }
    }

    /// Free an object of `class` on CPU `cpu`. The object is checked against
    /// its slab first, and a mismatch panics (6.2).
    pub fn free(&self, object: NonNull<u8>, class: usize, cpu: Option<usize>) {
        self.classes[class].check(object, &self.backing);
        let Some(caches) = cpu.and_then(|cpu| self.slot(cpu)) else {
            self.classes[class].put(&[object.as_ptr()], &self.backing, &self.descriptors);
            return;
        };
        let mut pairs = caches.lock.lock();
        let pair = &mut pairs[class];
        // SAFETY: the pair's magazines are this CPU's, under its lock.
        unsafe {
            if has_room(pair.loaded) {
                return push(pair.loaded, object);
            }
            if !pair.previous.is_null() && rounds(pair.previous) == 0 {
                core::mem::swap(&mut pair.loaded, &mut pair.previous);
                return push(pair.loaded, object);
            }
            // The loaded magazine is full or absent, and the previous one
            // holds rounds or is absent: trade for an empty magazine.
            let depot = &self.depots[class];
            let mut state = depot.state.lock();
            let surplus = state.update(self.backing.now_ms(), depot.state.contentions(), &depot.size);
            let mut empty = state.pop_empty();
            let size = depot.size.load(Ordering::Relaxed);
            drop(state);
            self.retire(class, surplus);
            if empty.is_null() {
                match self.new_magazine(size, true) {
                    Some(magazine) => empty = magazine,
                    None => {
                        self.classes[class].put(&[object.as_ptr()], &self.backing, &self.descriptors);
                        return;
                    }
                }
            }
            if !pair.previous.is_null() {
                let mut state = depot.state.lock();
                depot.push(&mut state, pair.previous);
            }
            pair.previous = pair.loaded;
            pair.loaded = empty;
            push(empty, object);
        }
    }

    /// Move CPU `cpu`'s magazines into the depots (7.2 steps 1 and 2). With
    /// `wait` false the CPU's lock and each depot's are taken with
    /// `try_lock`; a busy depot leaves that magazine with the CPU. Returns
    /// false when the CPU's lock was busy.
    pub fn drain(&self, cpu: usize, wait: bool) -> bool {
        let Some(caches) = self.slot(cpu) else {
            return true;
        };
        let Some(mut pairs) = caches.lock.lock_if(wait) else {
            return false;
        };
        for (class, pair) in pairs.iter_mut().enumerate() {
            for magazine in [&mut pair.loaded, &mut pair.previous] {
                if magazine.is_null() {
                    continue;
                }
                let depot = &self.depots[class];
                if let Some(mut state) = depot.state.lock_if(wait) {
                    // SAFETY: the magazine leaves this CPU for the depot.
                    unsafe { depot.push(&mut state, *magazine) };
                    *magazine = ptr::null_mut();
                }
            }
        }
        true
    }

    /// Return class `class`'s depot magazines to its slabs (7.2 step 3), if
    /// the depot's lock is free now. Slabs that empty go back to the page
    /// layer except the one each cache keeps.
    pub fn trim_depot(&self, class: usize) {
        if let Some(mut state) = self.depots[class].state.try_lock() {
            let surplus = state.take_all();
            drop(state);
            // SAFETY: the magazines were detached from the depot.
            unsafe { self.retire(class, surplus) };
        }
    }

    /// The slab shrinker (7.3, first): return each depot's magazines to the
    /// slabs and each empty slab to the page layer, the one kept per cache
    /// included, until `target` pages are back. A busy depot is skipped.
    /// Returns the pages given back.
    pub fn shrink(&self, target: usize) -> usize {
        let mut pages = 0;
        for class in 0..CLASS_COUNT {
            self.trim_depot(class);
            pages += self.classes[class].release_empty(&self.backing, &self.descriptors);
            if pages >= target {
                break;
            }
        }
        for cache in &self.magazines {
            pages += cache.release_empty(&self.backing, &self.descriptors);
        }
        pages + self.descriptors.release_empty(&self.backing, &self.descriptors)
    }

    /// Pages the shrinker could give back now, at most: the empty slabs, and
    /// the slabs the depots' rounds occupy.
    pub fn reclaimable(&self) -> usize {
        let mut pages = self.descriptors.stats().empty;
        for cache in &self.magazines {
            pages += cache.stats().empty;
        }
        for (cache, depot) in self.classes.iter().zip(&self.depots) {
            let stats = cache.stats();
            pages += stats.empty + depot.rounds.load(Ordering::Relaxed).div_ceil(stats.per_slab);
        }
        pages
    }

    pub fn class_stats(&self, class: usize) -> ClassStats {
        let depot = &self.depots[class];
        let mut cpu_rounds = 0;
        for cpu in 0..self.slot_count() {
            if let Some(pairs) = self.slot(cpu).and_then(|caches| caches.lock.try_lock()) {
                // SAFETY: the magazines are the CPU's, under its lock.
                cpu_rounds += unsafe { rounds(pairs[class].loaded) + rounds(pairs[class].previous) };
            }
        }
        ClassStats {
            slabs: self.classes[class].stats(),
            depot_rounds: depot.rounds.load(Ordering::Relaxed),
            cpu_rounds,
            magazine_rounds: ROUNDS[depot.size.load(Ordering::Relaxed)],
        }
    }

    /// Pages held by the descriptor and magazine caches.
    pub fn overhead_pages(&self) -> usize {
        self.descriptors.stats().slabs + self.magazines.iter().map(|cache| cache.stats().slabs).sum::<usize>()
    }

    /// Acquisitions of the locks CPUs share: each class's slab lock and
    /// depot lock, and the descriptor and magazine caches' locks.
    pub fn shared_acquisitions(&self) -> u64 {
        let classes: u64 = self.classes.iter().map(|cache| cache.lock_counts().0).sum();
        let depots: u64 = self.depots.iter().map(|depot| depot.state.acquisitions()).sum();
        let magazines: u64 = self.magazines.iter().map(|cache| cache.lock_counts().0).sum();
        classes + depots + magazines + self.descriptors.lock_counts().0
    }
}

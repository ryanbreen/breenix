//! Size-class arenas for malloc. Allocation uses the head of a per-class
//! partial list. The separate list of all arenas supports cache release and
//! provides the authoritative links for rebuilding the indexes after fork.
use core::ptr::{self, null_mut};
use core::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicU64, Ordering::*};
use libbreenix::syscall::{nr, raw};

pub(super) const ALIGNMENT: usize = 16;
pub(super) const HEADER: usize = 2 * size_of::<usize>();
const CLASSES: usize = 12; // 16, 32, ... 32768 bytes
const MAX_ARENA_BYTES: usize = 256 * 1024;
const PAGE_SIZE: usize = 4096;
const ARENA_TAG: usize = 1;
const WAITERS: u32 = 1 << 31;
const FUTEX_PRIVATE: u64 = 128;

#[repr(C, align(16))]
struct Arena {
    all: Links,
    partial: Links,
    // Live slots (including the continuation slots of a grown block).
    live: AtomicU64,
    // Pages that may still need discarding, and slots ever exposed to callers.
    touched: u64,
    used: u64,
    // A free reentering the owner defers its edit until the next acquisition.
    deferred: AtomicU64,
    class: usize,
}

#[derive(Default)]
struct Links {
    next: AtomicPtr<Arena>,
    prev: *mut Arena,
}

static OWNER: AtomicU32 = AtomicU32::new(0);
static PROCESS: AtomicU32 = AtomicU32::new(0);
static DEFERRED: AtomicBool = AtomicBool::new(false);
static ALL: [AtomicPtr<Arena>; CLASSES] = [const { AtomicPtr::new(null_mut()) }; CLASSES];
static PARTIAL: [AtomicPtr<Arena>; CLASSES] = [const { AtomicPtr::new(null_mut()) }; CLASSES];
static EMPTY: [AtomicPtr<Arena>; CLASSES] = [const { AtomicPtr::new(null_mut()) }; CLASSES];

fn futex(op: u64, value: u32) {
    unsafe {
        raw::syscall6(
            nr::FUTEX,
            OWNER.as_ptr() as u64,
            FUTEX_PRIVATE | op,
            value.into(),
            0,
            0,
            0,
        );
    }
}

pub(crate) struct Guard;

impl Guard {
    fn acquire() -> Option<Self> {
        let id = crate::malloc_thread_id();
        let Err(mut observed) = OWNER.compare_exchange(0, id, Acquire, Relaxed) else {
            return Some(Self);
        };
        // The public fork path repairs the cache in the child. This also
        // handles a direct fork syscall that inherited a held lock.
        let pid = crate::getpid() as u32;
        if PROCESS.load(Relaxed) != pid {
            unsafe { after_fork(pid) };
            return Self::acquire();
        }
        loop {
            if observed == 0 {
                match OWNER.compare_exchange(0, id | WAITERS, Acquire, Relaxed) {
                    Ok(_) => return Some(Self),
                    Err(other) => observed = other,
                }
            } else if observed & !WAITERS == id {
                // Same-thread reentry cannot wait for itself.
                return None;
            } else if observed & WAITERS == 0 {
                match OWNER.compare_exchange(observed, observed | WAITERS, Relaxed, Relaxed) {
                    Ok(_) => observed |= WAITERS,
                    Err(other) => observed = other,
                }
            } else {
                futex(0, observed);
                observed = OWNER.load(Relaxed);
            }
        }
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        if OWNER.swap(0, Release) & WAITERS != 0 {
            futex(1, 1);
        }
    }
}

fn initialize_process() {
    if PROCESS.load(Relaxed) == 0 {
        PROCESS.store(crate::getpid() as u32, Relaxed);
    }
}

pub(crate) fn before_fork() -> Option<Guard> {
    initialize_process();
    Guard::acquire()
}

/// Called by the sole surviving thread before child atfork callbacks. A raw
/// fork may have copied a half-edited index. Forward ALL links are published
/// in one store and remain valid throughout every edit.
pub(crate) unsafe fn after_fork(pid: u32) {
    if PROCESS.load(Relaxed) != pid {
        if OWNER.load(Relaxed) != 0 {
            rebuild_indexes();
        }
        PROCESS.store(pid, Relaxed);
        OWNER.store(0, Release);
    }
}

pub fn class(size: usize) -> Option<usize> {
    if size == 0 || size > capacity(CLASSES - 1) {
        return None;
    }
    Some(size.max(ALIGNMENT).next_power_of_two().trailing_zeros() as usize - 4)
}

pub fn capacity(class: usize) -> usize {
    ALIGNMENT << class
}

fn stride(class: usize) -> usize {
    HEADER + capacity(class)
}

fn slots(class: usize) -> usize {
    ((MAX_ARENA_BYTES - size_of::<Arena>()) / stride(class)).min(u64::BITS as usize)
}

fn length(class: usize) -> usize {
    size_of::<Arena>() + slots(class) * stride(class)
}

fn low_bits(n: usize) -> u64 {
    u64::MAX >> (u64::BITS as usize - n)
}

pub fn is_arena_tag(tag: usize) -> bool {
    tag & ARENA_TAG != 0
}

fn arena(tag: usize) -> *mut Arena {
    ptr::with_exposed_provenance_mut(tag & !ARENA_TAG)
}

fn run_slots(class: usize, size: usize) -> usize {
    (size.max(1) + HEADER).div_ceil(stride(class))
}

pub unsafe fn block_capacity(tag: usize, requested: usize) -> usize {
    let class = (*arena(tag)).class;
    run_slots(class, requested) * stride(class) - HEADER
}

fn all_links(p: *mut Arena) -> *mut Links {
    unsafe { &raw mut (*p).all }
}

fn partial_links(p: *mut Arena) -> *mut Links {
    unsafe { &raw mut (*p).partial }
}

unsafe fn push(head: &AtomicPtr<Arena>, arena: *mut Arena, links: fn(*mut Arena) -> *mut Links) {
    let next = head.load(Relaxed);
    (*links(arena)).next.store(next, Relaxed);
    (*links(arena)).prev = null_mut();
    head.store(arena, Release);
    if !next.is_null() {
        (*links(next)).prev = arena;
    }
}

unsafe fn remove(head: &AtomicPtr<Arena>, arena: *mut Arena, links: fn(*mut Arena) -> *mut Links) {
    let next = (*links(arena)).next.load(Relaxed);
    let prev = (*links(arena)).prev;
    if prev.is_null() {
        head.store(next, Release);
    } else {
        (*links(prev)).next.store(next, Release);
    }
    if !next.is_null() {
        (*links(next)).prev = prev;
    }
    *links(arena) = Links::default();
}

unsafe fn rebuild_indexes() {
    for class in 0..CLASSES {
        PARTIAL[class].store(null_mut(), Relaxed);
        EMPTY[class].store(null_mut(), Relaxed);
        let mut prev = null_mut();
        let mut arena = ALL[class].load(Acquire);
        while !arena.is_null() {
            (*arena).all.prev = prev;
            (*arena).partial = Links::default();
            let live = (*arena).live.load(Acquire);
            if live != low_bits(slots(class)) {
                push(&PARTIAL[class], arena, partial_links);
            }
            if live == 0 {
                EMPTY[class].store(arena, Relaxed);
            }
            prev = arena;
            arena = (*arena).all.next.load(Acquire);
        }
    }
}

unsafe fn apply_deferred() {
    if !DEFERRED.swap(false, Relaxed) {
        return;
    }
    for head in &ALL {
        let mut arena = head.load(Relaxed);
        while !arena.is_null() {
            let mask = (*arena).deferred.swap(0, Relaxed);
            if mask != 0 {
                release_slots(arena, mask);
            }
            arena = (*arena).all.next.load(Acquire);
        }
    }
}

// Keep the usual allocation entry point small. The zeroing path uses the
// same cache operation and can clear only the recycled slots.
pub unsafe fn take(class: usize, map: impl FnOnce(usize) -> *mut u8) -> Option<(*mut u8, usize)> {
    take_inner(class, map, 0)
}

pub unsafe fn take_zeroed(
    class: usize,
    map: impl FnOnce(usize) -> *mut u8,
    size: usize,
) -> Option<(*mut u8, usize)> {
    take_inner(class, map, size)
}

unsafe fn take_inner(
    class: usize,
    map: impl FnOnce(usize) -> *mut u8,
    clear: usize,
) -> Option<(*mut u8, usize)> {
    initialize_process();
    let guard = Guard::acquire()?;
    if DEFERRED.load(Relaxed) {
        apply_deferred();
    }
    let head = &PARTIAL[class];
    let mut arena = head.load(Relaxed);
    if arena.is_null() {
        arena = map(length(class)).cast();
        if arena.is_null() {
            return None;
        }
        arena.write(Arena {
            all: Links::default(),
            partial: Links::default(),
            live: AtomicU64::new(0),
            touched: 0,
            used: 0,
            deferred: AtomicU64::new(0),
            class,
        });
        push(&ALL[class], arena, all_links);
        push(head, arena, partial_links);
    }
    if arena == EMPTY[class].load(Relaxed) {
        EMPTY[class].store(null_mut(), Relaxed);
    }
    let index = (*arena).live.load(Relaxed).trailing_ones() as usize;
    let mask = 1 << index;
    let clear = if (*arena).used & mask != 0 { clear } else { 0 };
    claim_slots(arena, mask);
    let p = arena
        .cast::<u8>()
        .add(size_of::<Arena>() + index * stride(class) + HEADER);
    drop(guard);
    if clear != 0 {
        crate::string::memset(p, 0, clear);
    }
    Some((p, arena.expose_provenance() | ARENA_TAG))
}

unsafe fn claim_slots(arena: *mut Arena, mask: u64) {
    let class = (*arena).class;
    (*arena).used |= mask;
    (*arena).touched |= mask;
    let live = (*arena).live.load(Relaxed) | mask;
    (*arena).live.store(live, Release);
    if live == low_bits(slots(class)) {
        remove(&PARTIAL[class], arena, partial_links);
    }
}

unsafe fn release_slots(arena: *mut Arena, mask: u64) {
    let class = (*arena).class;
    let previous = (*arena).live.load(Relaxed);
    (*arena).live.store(previous & !mask, Release);
    if previous == low_bits(slots(class)) {
        push(&PARTIAL[class], arena, partial_links);
    }
}

fn slot_index(arena: *mut Arena, block: *mut u8, class: usize) -> usize {
    (block.addr() - arena.addr() - size_of::<Arena>() - HEADER) / stride(class)
}

pub unsafe fn put_run(
    tag: usize,
    block: *mut u8,
    size: usize,
    unmap: impl FnOnce(*mut u8, usize) -> bool,
) {
    let arena = arena(tag);
    let class = (*arena).class;
    let mask = low_bits(run_slots(class, size)) << slot_index(arena, block, class);
    let Some(_guard) = Guard::acquire() else {
        // Only the owner can reenter; preserve the slot until we can edit
        // both the bitmap and partial-list membership together.
        (*arena).deferred.fetch_or(mask, Relaxed);
        DEFERRED.store(true, Relaxed);
        return;
    };
    release_slots(arena, mask);
    if (*arena).live.load(Relaxed) == 0 {
        if EMPTY[class].load(Relaxed).is_null() {
            EMPTY[class].store(arena, Relaxed);
        } else {
            retire(arena, class, unmap);
        }
    }
}

pub unsafe fn resize(tag: usize, block: *mut u8, requested: *mut usize, size: usize) -> bool {
    let arena = arena(tag);
    let class = (*arena).class;
    let old_slots = run_slots(class, requested.read());
    let new_slots = run_slots(class, size);
    if old_slots == new_slots {
        requested.write(size);
        return true;
    }
    let index = slot_index(arena, block, class);
    if new_slots > slots(class) - index {
        return false;
    }
    let Some(_guard) = Guard::acquire() else {
        return false;
    };
    let change = (low_bits(old_slots) ^ low_bits(new_slots)) << index;
    if new_slots > old_slots {
        if (*arena).live.load(Relaxed) & change != 0 {
            return false;
        }
        claim_slots(arena, change);
        requested.write(size);
    } else {
        // A raw fork between these stores can retain a reserved tail, but
        // must never inherit a header spanning slots already released.
        requested.write(size);
        release_slots(arena, change);
    }
    true
}

unsafe fn retire(
    arena: *mut Arena,
    class: usize,
    unmap: impl FnOnce(*mut u8, usize) -> bool,
) -> bool {
    if EMPTY[class].load(Relaxed) == arena {
        EMPTY[class].store(null_mut(), Relaxed);
    }
    remove(&PARTIAL[class], arena, partial_links);
    remove(&ALL[class], arena, all_links);
    if unmap(arena.cast(), length(class)) {
        true
    } else {
        push(&ALL[class], arena, all_links);
        push(&PARTIAL[class], arena, partial_links);
        false
    }
}

/// Unmap empty arenas and discard pages that do not intersect a live slot.
pub unsafe fn drain(
    mut unmap: impl FnMut(*mut u8, usize) -> bool,
    mut discard: impl FnMut(*mut u8, usize) -> bool,
) -> usize {
    initialize_process();
    let Some(_guard) = Guard::acquire() else {
        return 0;
    };
    if DEFERRED.load(Relaxed) {
        apply_deferred();
    }
    let mut count = 0;
    for (class, head) in ALL.iter().enumerate() {
        let mut arena = head.load(Relaxed);
        while !arena.is_null() {
            let next = (*arena).all.next.load(Acquire);
            let live = (*arena).live.load(Acquire);
            if live == 0 {
                count += usize::from(retire(arena, class, &mut unmap));
            } else if (*arena).touched & !live != 0
                && discard_unused_pages(arena, class, live, &mut discard)
            {
                (*arena).touched = live;
            }
            arena = next;
        }
    }
    count
}

unsafe fn discard_unused_pages(
    arena: *mut Arena,
    class: usize,
    mut live: u64,
    mut discard: impl FnMut(*mut u8, usize) -> bool,
) -> bool {
    let mut discard_gap = |from: usize, to: usize| {
        let first_page = from.next_multiple_of(PAGE_SIZE);
        let end = to & !(PAGE_SIZE - 1);
        first_page >= end || discard(arena.cast::<u8>().add(first_page), end - first_page)
    };
    let mut gap_start = size_of::<Arena>();
    let mut success = true;
    while live != 0 {
        let index = live.trailing_zeros() as usize;
        let slot_start = size_of::<Arena>() + index * stride(class);
        success &= discard_gap(gap_start, slot_start);
        gap_start = slot_start + stride(class);
        live &= live - 1;
    }
    success & discard_gap(gap_start, length(class).next_multiple_of(PAGE_SIZE))
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::alloc::{alloc, dealloc, Layout};

    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    unsafe extern "C" {
        fn fork() -> i32;
        fn waitpid(pid: i32, status: *mut i32, flags: i32) -> i32;
        fn _exit(status: i32) -> !;
    }

    fn map(len: usize) -> *mut u8 {
        unsafe { alloc(Layout::from_size_align(len, 4096).unwrap()) }
    }

    fn unmap(ptr: *mut u8, len: usize) -> bool {
        unsafe { dealloc(ptr, Layout::from_size_align(len, 4096).unwrap()) };
        true
    }

    fn no_discard(_: *mut u8, _: usize) -> bool {
        true
    }

    fn no_mapping(_: usize) -> *mut u8 {
        ptr::null_mut()
    }

    unsafe fn put(tag: usize, block: *mut u8, unmap: impl FnOnce(*mut u8, usize) -> bool) {
        put_run(tag, block, 1, unmap);
    }

    #[test]
    fn reuse_is_bounded_and_preserves_live_blocks() {
        let _test = TEST_LOCK.lock().unwrap();
        let class = class(47).unwrap();
        assert_eq!(capacity(class), 64);
        unsafe {
            let (live, live_tag) = take(class, map).unwrap();
            assert!(is_arena_tag(live_tag));
            assert_eq!(block_capacity(live_tag, 47), 64);
            live.add(16).write(0xa5);
            let blocks: Vec<_> = (1..slots(class))
                .map(|_| take(class, no_mapping).unwrap())
                .collect();
            let (overflow, overflow_tag) = take(class, map).unwrap();
            assert_ne!(overflow_tag, live_tag);
            put(overflow_tag, overflow, unmap);
            for &(ptr, tag) in &blocks {
                put(tag, ptr, unmap);
            }
            assert_eq!(drain(unmap, no_discard), 1);
            assert_eq!(live.add(16).read(), 0xa5);
            let reused = take(class, no_mapping).unwrap();
            assert_eq!(reused, blocks[0]);
            put(reused.1, reused.0, unmap);
            put(live_tag, live, unmap);
            assert_eq!(drain(unmap, no_discard), 1);
            assert!(take(class, no_mapping).is_none());
        }
    }

    #[test]
    fn concurrent_reuse_never_hands_out_a_live_block_twice() {
        let _test = TEST_LOCK.lock().unwrap();
        std::thread::scope(|scope| {
            for index in 0..8 {
                scope.spawn(move || {
                    for _ in 0..10000 {
                        let (ptr, arena) = loop {
                            if let Some(block) = unsafe { take(7, map) } {
                                break block;
                            }
                            std::thread::yield_now();
                        };
                        let tag = index + 1;
                        unsafe {
                            ptr.add(8).cast::<u64>().write(tag);
                            std::thread::yield_now();
                            assert_eq!(ptr.add(8).cast::<u64>().read(), tag);
                            put(arena, ptr, unmap);
                        }
                    }
                });
            }
        });
        unsafe { drain(unmap, no_discard) };
    }

    #[test]
    fn reentrant_cache_access_falls_back_without_spinning() {
        let _test = TEST_LOCK.lock().unwrap();
        unsafe {
            let (ptr, tag) = take(0, map).unwrap();
            let guard = Guard::acquire().unwrap();
            assert!(take(0, no_mapping).is_none());
            put(tag, ptr, unmap);
            drop(guard);
            assert_eq!(take(0, no_mapping), Some((ptr, tag)));
            put(tag, ptr, unmap);
            assert_eq!(drain(unmap, no_discard), 1);
        }
    }

    #[test]
    fn raw_fork_recovers_the_lock_and_preserves_live_slots() {
        let _test = TEST_LOCK.lock().unwrap();
        let (live, tag) = unsafe { take(0, map) }.unwrap();
        unsafe { live.write(0xa5) };
        let guard = Guard::acquire().unwrap();
        let pid = unsafe { fork() };
        assert!(pid >= 0);
        if pid == 0 {
            let valid = unsafe { take(0, no_mapping) }.is_some_and(|(other, other_tag)| {
                other != live && other_tag == tag && unsafe { live.read() } == 0xa5
            });
            unsafe { _exit(if valid { 0 } else { 1 }) };
        }
        drop(guard);
        let mut status = 0;
        assert_eq!(unsafe { waitpid(pid, &mut status, 0) }, pid);
        assert_eq!(status, 0);
        unsafe {
            put(tag, live, unmap);
            assert_eq!(drain(unmap, no_discard), 1);
        }
    }
}

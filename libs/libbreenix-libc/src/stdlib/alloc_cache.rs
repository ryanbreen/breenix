//! Size-class arenas for malloc. Each mapping has a bitmap of live slots and an
//! intrusive link. Freeing a slot needs no allocation, including during contention.
use core::ptr;
use core::sync::atomic::{AtomicPtr, AtomicU64, Ordering};

pub(super) const ALIGNMENT: usize = 16;
pub(super) const HEADER: usize = 2 * core::mem::size_of::<usize>();
const CLASSES: usize = 12; // 16, 32, ... 32768 bytes
const MAX_ARENA_BYTES: usize = 256 * 1024;
const PAGE_SIZE: usize = 4096;
const LOCKED: u64 = 1 << 63;
const ARENA_TAG: usize = 1;

#[repr(C, align(16))]
struct Arena {
    next: AtomicPtr<Arena>,
    live: AtomicU64,
    touched: AtomicU64,
    class: usize,
}

static OWNER: AtomicU64 = AtomicU64::new(0);
static HEADS: [AtomicPtr<Arena>; CLASSES] = [const { AtomicPtr::new(ptr::null_mut()) }; CLASSES];

struct Guard(u64);
impl Guard {
    fn acquire() -> Option<Self> {
        // A raw fork can inherit the lock held by another thread. Publication
        // follows arena initialization, and unlinking precedes unmapping.
        // List and bitmap edits use a single atomic operation, so the
        // child can take over without discarding arenas that hold live blocks.
        // Breenix CLONE_VM members share getpid()'s thread-group identity.
        let pid = crate::getpid() as u32 as u64;
        loop {
            let old = OWNER.load(Ordering::Relaxed);
            if old == pid | LOCKED {
                return None;
            }
            if OWNER
                .compare_exchange_weak(old, pid | LOCKED, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                return Some(Self(pid));
            }
        }
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        OWNER.store(self.0, Ordering::Release);
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

pub fn is_arena_tag(tag: usize) -> bool {
    tag & ARENA_TAG != 0
}

fn arena(tag: usize) -> *mut Arena {
    ptr::with_exposed_provenance_mut(tag & !ARENA_TAG)
}

/// The caller holds a live slot in the arena.
pub unsafe fn block_capacity(tag: usize) -> usize {
    capacity((*arena(tag)).class)
}

pub unsafe fn take(class: usize, map: impl FnOnce(usize) -> *mut u8) -> Option<(*mut u8, usize)> {
    let _guard = Guard::acquire()?;
    let head = &HEADS[class];
    let mut arena = head.load(Ordering::Relaxed);
    let slot_count = slots(class);
    while !arena.is_null() {
        let bit = (*arena).live.load(Ordering::Acquire).trailing_ones() as usize;
        if bit < slot_count {
            // Under the guard only this thread can set bits. A concurrent
            // free can only clear other bits, so an atomic OR claims this one.
            let mask = 1 << bit;
            (*arena).touched.fetch_or(mask, Ordering::Relaxed);
            (*arena).live.fetch_or(mask, Ordering::AcqRel);
            return Some(block(arena, class, bit));
        }
        arena = (*arena).next.load(Ordering::Relaxed);
    }

    let arena = map(length(class)).cast::<Arena>();
    if arena.is_null() {
        return None;
    }
    arena.write(Arena {
        next: AtomicPtr::new(head.load(Ordering::Relaxed)),
        live: AtomicU64::new(1),
        touched: AtomicU64::new(1),
        class,
    });
    head.store(arena, Ordering::Release);
    Some(block(arena, class, 0))
}

unsafe fn block(arena: *mut Arena, class: usize, index: usize) -> (*mut u8, usize) {
    let ptr = arena
        .cast::<u8>()
        .add(size_of::<Arena>() + index * stride(class) + HEADER);
    (ptr, arena.expose_provenance() | ARENA_TAG)
}

/// Release a slot and keep at most the head arena of this class warm on the
/// uncontended path. A contended free leaves empty arenas for the next drain.
pub unsafe fn put(tag: usize, block: *mut u8, unmap: impl FnOnce(*mut u8, usize) -> bool) {
    let guard = Guard::acquire();
    let arena = arena(tag);
    let class = (*arena).class;
    let offset = block.addr() - arena.addr() - size_of::<Arena>() - HEADER;
    let bit = 1 << (offset / stride(class));
    let previous = (*arena).live.fetch_and(!bit, Ordering::AcqRel);
    // With no guard, a drain may unmap the arena as soon as we clear its last
    // bit. Do not read anything from it after the atomic operation.
    let Some(_guard) = guard else { return };
    if previous == bit && HEADS[class].load(Ordering::Relaxed) != arena {
        let mut link = &HEADS[class];
        let mut current = link.load(Ordering::Relaxed);
        while current != arena {
            link = &(*current).next;
            current = link.load(Ordering::Relaxed);
        }
        unlink(link, arena, class, unmap);
    }
}

unsafe fn unlink(
    link: &AtomicPtr<Arena>,
    arena: *mut Arena,
    class: usize,
    unmap: impl FnOnce(*mut u8, usize) -> bool,
) -> bool {
    let next = (*arena).next.load(Ordering::Relaxed);
    link.store(next, Ordering::Release);
    if unmap(arena.cast(), length(class)) {
        true
    } else {
        link.store(arena, Ordering::Release);
        false
    }
}

/// Unmap empty arenas and discard pages that do not intersect a live slot.
pub unsafe fn drain(
    mut unmap: impl FnMut(*mut u8, usize) -> bool,
    mut discard: impl FnMut(*mut u8, usize) -> bool,
) -> usize {
    let Some(_guard) = Guard::acquire() else {
        return 0;
    };
    let mut count = 0;
    for (class, head) in HEADS.iter().enumerate() {
        let mut link = head;
        loop {
            let arena = link.load(Ordering::Relaxed);
            if arena.is_null() {
                break;
            }
            let live = (*arena).live.load(Ordering::Acquire);
            if live == 0 {
                if unlink(link, arena, class, &mut unmap) {
                    count += 1;
                    continue;
                }
            } else if (*arena).touched.load(Ordering::Relaxed) & !live != 0
                && discard_unused_pages(arena, class, live, &mut discard)
            {
                // A concurrent free can only clear live bits. Keeping the
                // snapshot lets the next drain pick up those frees.
                (*arena).touched.store(live, Ordering::Relaxed);
            }
            link = &(*arena).next;
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

    #[test]
    fn reuse_is_bounded_and_preserves_live_blocks() {
        let _test = TEST_LOCK.lock().unwrap();
        let class = class(47).unwrap();
        assert_eq!(capacity(class), 64);
        unsafe {
            let (live, live_tag) = take(class, map).unwrap();
            assert!(is_arena_tag(live_tag));
            assert_eq!(block_capacity(live_tag), 64);
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

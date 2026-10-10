//! Bounded reuse of small malloc mappings. Larger and aligned allocations
//! continue to be unmapped on free. Reused blocks avoid mmap, munmap and page
//! faults; the process-ID query also makes raw-fork lock recovery safe.
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

const CLASSES: usize = 8; // 16, 32, ... 2048 bytes
const LIMIT: usize = 16; // At most 512 KiB of cached pages per process.
const LOCKED: u64 = 1 << 63;
const MARKER: usize = 1 << (usize::BITS - 1);
static OWNER: AtomicU64 = AtomicU64::new(0);
static HEADS: [AtomicUsize; CLASSES] = [const { AtomicUsize::new(0) }; CLASSES];
static COUNTS: [AtomicUsize; CLASSES] = [const { AtomicUsize::new(0) }; CLASSES];

struct Guard(u64);
impl Guard {
    fn acquire() -> Option<Self> {
        // A raw fork can copy a mutex held by a different thread. Identify the
        // process on acquisition, so a child can discard an inherited owner.
        // Breenix CLONE_VM members share getpid()'s thread-group identity.
        let pid = super::getpid() as u32 as u64;
        loop {
            let old = OWNER.load(Ordering::Acquire);
            if old & !LOCKED != pid || old & LOCKED == 0 {
                if OWNER
                    .compare_exchange_weak(old, pid | LOCKED, Ordering::Acquire, Ordering::Relaxed)
                    .is_ok()
                {
                    if old & !LOCKED != pid && old & LOCKED != 0 {
                        // The fork snapshot may be halfway through a list edit.
                        // Discard only the bounded free cache, not live blocks.
                        for index in 0..CLASSES {
                            HEADS[index].store(0, Ordering::Relaxed);
                            COUNTS[index].store(0, Ordering::Relaxed);
                        }
                    }
                    return Some(Self(pid));
                }
            } else {
                return None;
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
    if size == 0 || size > 2048 {
        return None;
    }
    Some(size.max(16).next_power_of_two().trailing_zeros() as usize - 4)
}

pub fn capacity(class: usize) -> usize {
    16 << class
}
pub fn marker(class: usize) -> usize {
    MARKER | class
}
pub fn marked_class(word: usize) -> Option<usize> {
    let class = word & !MARKER;
    (word & MARKER != 0 && class < CLASSES).then_some(class)
}

pub unsafe fn take(class: usize) -> *mut u8 {
    let Some(_guard) = Guard::acquire() else { return core::ptr::null_mut(); };
    let head = HEADS[class].load(Ordering::Relaxed) as *mut u8;
    if !head.is_null() {
        HEADS[class].store(*(head as *const usize), Ordering::Relaxed);
        COUNTS[class].fetch_sub(1, Ordering::Relaxed);
    }
    head
}

/// True when the mapping was retained; false when the caller must unmap it.
pub unsafe fn put(class: usize, ptr: *mut u8) -> bool {
    let Some(_guard) = Guard::acquire() else { return false; };
    if COUNTS[class].load(Ordering::Relaxed) >= LIMIT {
        return false;
    }
    *(ptr as *mut usize) = HEADS[class].load(Ordering::Relaxed);
    HEADS[class].store(ptr as usize, Ordering::Relaxed);
    COUNTS[class].fetch_add(1, Ordering::Relaxed);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    unsafe extern "C" {
        fn fork() -> i32;
        fn waitpid(pid: i32, status: *mut i32, flags: i32) -> i32;
        fn _exit(status: i32) -> !;
    }

    #[test]
    fn reuse_is_bounded_and_preserves_live_blocks() {
        let _test = TEST_LOCK.lock().unwrap();
        let class = class(47).unwrap();
        assert_eq!(capacity(class), 64);
        let mut blocks = [[0usize; 8]; LIMIT + 1];
        for block in &mut blocks[..LIMIT] {
            assert!(unsafe { put(class, block.as_mut_ptr().cast()) });
        }
        assert!(!unsafe { put(class, blocks[LIMIT].as_mut_ptr().cast()) });
        let mut seen = std::collections::HashSet::new();
        for _ in 0..LIMIT {
            let block = unsafe { take(class) };
            assert!(!block.is_null());
            assert!(seen.insert(block as usize));
            unsafe { block.add(16).write(0xa5) };
        }
        assert!(unsafe { take(class) }.is_null());
        assert!(blocks[..LIMIT]
            .iter()
            .all(|b| unsafe { (b.as_ptr().cast::<u8>()).add(16).read() == 0xa5 }));
    }

    #[test]
    fn concurrent_reuse_never_hands_out_a_live_block_twice() {
        let _test = TEST_LOCK.lock().unwrap();
        std::thread::scope(|scope| {
            for index in 0..8 {
                scope.spawn(move || {
                    for _ in 0..10000 {
                        let mut ptr = unsafe { take(7) };
                        if ptr.is_null() {
                            ptr = Box::into_raw(Box::new([0u64; 256])).cast();
                        }
                        let tag = index + 1;
                        unsafe {
                            (ptr.add(8) as *mut u64).write(tag);
                            std::thread::yield_now();
                            assert_eq!((ptr.add(8) as *const u64).read(), tag);
                            if !put(7, ptr) {
                                drop(Box::from_raw(ptr.cast::<[u64; 256]>()));
                            }
                        }
                    }
                });
            }
        });
        loop {
            let ptr = unsafe { take(7) };
            if ptr.is_null() {
                break;
            }
            unsafe {
                drop(Box::from_raw(ptr.cast::<[u64; 256]>()));
            }
        }
    }

    #[test]
    fn reentrant_cache_access_falls_back_without_spinning() {
        let _test = TEST_LOCK.lock().unwrap();
        let guard = Guard::acquire().unwrap();
        let mut block = [0usize; 2];
        assert!(unsafe { take(0) }.is_null());
        assert!(!unsafe { put(0, block.as_mut_ptr().cast()) });
        drop(guard);
    }

    #[test]
    fn fork_discards_a_partial_free_list_without_waiting_for_parent() {
        let _test = TEST_LOCK.lock().unwrap();
        let guard = Guard::acquire().unwrap();
        // A fork may see this inconsistent intermediate state under the lock.
        COUNTS[0].store(1, Ordering::Relaxed);
        HEADS[0].store(1, Ordering::Relaxed);
        let pid = unsafe { fork() };
        assert!(pid >= 0);
        if pid == 0 {
            let ptr = unsafe { take(0) };
            unsafe { _exit(if ptr.is_null() { 0 } else { 1 }) };
        }
        HEADS[0].store(0, Ordering::Relaxed);
        COUNTS[0].store(0, Ordering::Relaxed);
        drop(guard);
        let mut status = 0;
        assert_eq!(unsafe { waitpid(pid, &mut status, 0) }, pid);
        assert_eq!(status, 0);
    }
}

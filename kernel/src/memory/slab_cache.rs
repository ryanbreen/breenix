//! Slab caches (E1 plan, 6.2): object caches over single-page slabs, after
//! J. Bonwick, "The Slab Allocator: An Object-Caching Kernel Memory
//! Allocator", USENIX Summer 1994.
//!
//! Each slab is one 4 KiB page reached through the direct map, so no slab
//! needs contiguous frames. A slab's bookkeeping is off-page, in a descriptor
//! from the descriptor cache, so the whole page holds objects. The descriptor
//! cache is self-hosting: each of its slabs holds its own descriptor in its
//! first object. The page layer records each page's descriptor (`FrameMeta`'s
//! slab word, 4.3), so a free finds its slab from the address in O(1).
//!
//! The module has no kernel dependency. A `Backing` supplies the pages, the
//! per-page slab record and the clock: the kernel's is frames through the
//! direct map (`memory/kmem.rs`), the host tests' a simulated page layer.
//! The per-CPU magazines in front of these caches are `magazine.rs`.

use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::ptr::{self, NonNull};
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

pub const PAGE: usize = 4096;

/// The size classes in bytes (6.2): 16 to 160 in 16-byte steps, then above
/// 128 the largest multiple of 16 that fits a whole number of objects in a
/// page, and each power of two to 4096.
pub const CLASSES: [usize; 25] = [
    16, 32, 48, 64, 80, 96, 112, 128, 144, 160, 192, 224, 256, 288, 336, 400, 448, 512, 576, 672,
    816, 1024, 1360, 2048, 4096,
];
pub const CLASS_COUNT: usize = CLASSES.len();
const STEP: usize = 16;
const STEPPED: usize = 160;

/// The class that serves a request of `size` bytes aligned to `align`; a
/// request above a page has no class (6.1, row 1). Above 16 bytes of
/// alignment the class is the smallest power of two at least
/// max(size, align): objects sit at multiples of their size from a
/// page-aligned base, so such a class aligns each object to its size.
#[inline]
pub fn class_of(size: usize, align: usize) -> Option<usize> {
    if size > PAGE || align > PAGE {
        return None;
    }
    if align <= STEP {
        if size <= STEPPED {
            return Some(size.max(1).div_ceil(STEP) - 1);
        }
        return Some(CLASSES.partition_point(|&class| class < size));
    }
    let need = size.max(align).next_power_of_two();
    Some(CLASSES.partition_point(|&class| class < need))
}

/// A spin lock that counts its acquisitions and how many of them had to
/// wait. The counts are what the magazine layer's size growth reads (6.3)
/// and what the host model counts. The caller masks interrupts for the
/// hold: each allocator lock is taken with this CPU's interrupts masked.
pub struct SpinLock<T> {
    held: AtomicBool,
    acquired: AtomicU64,
    contended: AtomicU64,
    value: UnsafeCell<T>,
}

// SAFETY: `held` gives one holder at a time access to `value`. The values
// are allocator bookkeeping whose raw pointers address allocator-owned
// memory, which any CPU may touch while holding the lock.
unsafe impl<T> Sync for SpinLock<T> {}
unsafe impl<T> Send for SpinLock<T> {}

pub struct Guard<'a, T> {
    lock: &'a SpinLock<T>,
}

impl<T> SpinLock<T> {
    pub const fn new(value: T) -> Self {
        Self {
            held: AtomicBool::new(false),
            acquired: AtomicU64::new(0),
            contended: AtomicU64::new(0),
            value: UnsafeCell::new(value),
        }
    }

    #[inline]
    pub fn try_lock(&self) -> Option<Guard<'_, T>> {
        if self.held.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
            return None;
        }
        // The holder alone writes the count.
        self.acquired.store(self.acquired.load(Ordering::Relaxed) + 1, Ordering::Relaxed);
        Some(Guard { lock: self })
    }

    #[inline]
    pub fn lock(&self) -> Guard<'_, T> {
        if let Some(guard) = self.try_lock() {
            return guard;
        }
        self.contended.fetch_add(1, Ordering::Relaxed);
        loop {
            while self.held.load(Ordering::Relaxed) {
                core::hint::spin_loop();
            }
            if let Some(guard) = self.try_lock() {
                return guard;
            }
        }
    }

    /// `lock` when `wait`, else one `try_lock`.
    #[inline]
    pub fn lock_if(&self, wait: bool) -> Option<Guard<'_, T>> {
        if wait {
            Some(self.lock())
        } else {
            self.try_lock()
        }
    }

    pub fn acquisitions(&self) -> u64 {
        self.acquired.load(Ordering::Relaxed)
    }

    /// Acquisitions that found the lock held and waited for it.
    pub fn contentions(&self) -> u64 {
        self.contended.load(Ordering::Relaxed)
    }
}

impl<T> Deref for Guard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: the guard is the lock's one holder.
        unsafe { &*self.lock.value.get() }
    }
}

impl<T> DerefMut for Guard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: the guard is the lock's one holder.
        unsafe { &mut *self.lock.value.get() }
    }
}

impl<T> Drop for Guard<'_, T> {
    fn drop(&mut self) {
        self.lock.held.store(false, Ordering::Release);
    }
}

/// What the page layer underneath the caches provides.
pub trait Backing {
    /// One page, mapped writable at a page-aligned kernel address.
    fn take_page(&self) -> Option<NonNull<u8>>;
    /// Return a page `take_page` gave, after its slab record is cleared.
    fn give_page(&self, page: NonNull<u8>);
    /// Record `slab` as the descriptor of `page`; null clears the record.
    fn set_slab(&self, page: NonNull<u8>, slab: *mut Slab);
    /// The descriptor recorded for `page`, or null.
    fn slab(&self, page: NonNull<u8>) -> *mut Slab;
    /// Milliseconds on a monotonic clock, for the depot's working set.
    fn now_ms(&self) -> u64;
}

/// Why a cache could not hand out an object.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Short {
    /// A lock was held and the attempt did not wait for it.
    Busy,
    /// No free object, and the page layer gave no page.
    NoPage,
}

const PARTIAL: u32 = 0;
const FULL: u32 = 1;
const EMPTY: u32 = 2;

/// A slab's descriptor (6.2): its cache, its page, the free objects threaded
/// through their own first words, the in-use count and its list links.
#[repr(C)]
pub struct Slab {
    cache: *const SlabCache,
    base: *mut u8,
    free: *mut u8,
    inuse: u32,
    capacity: u32,
    prev: *mut Slab,
    next: *mut Slab,
    on: u32,
}

/// A descriptor's object size in the descriptor cache.
const DESCRIPTOR_BYTES: usize = 64;
const _: () = assert!(core::mem::size_of::<Slab>() <= DESCRIPTOR_BYTES);

struct List {
    head: *mut Slab,
    len: usize,
}

impl List {
    const fn new() -> Self {
        Self { head: ptr::null_mut(), len: 0 }
    }

    unsafe fn link(&mut self, slab: *mut Slab, on: u32) {
        unsafe {
            (*slab).prev = ptr::null_mut();
            (*slab).next = self.head;
            (*slab).on = on;
            if !self.head.is_null() {
                (*self.head).prev = slab;
            }
        }
        self.head = slab;
        self.len += 1;
    }

    unsafe fn remove(&mut self, slab: *mut Slab) {
        unsafe {
            let (prev, next) = ((*slab).prev, (*slab).next);
            if prev.is_null() {
                self.head = next;
            } else {
                (*prev).next = next;
            }
            if !next.is_null() {
                (*next).prev = prev;
            }
        }
        self.len -= 1;
    }
}

pub struct Lists {
    partial: List,
    full: List,
    empty: List,
}

/// The counts of one cache, as `/proc/breenix/kmem` and the shrinker read them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CacheStats {
    pub size: usize,
    pub per_slab: usize,
    /// Slabs, and so pages, the cache holds.
    pub slabs: usize,
    /// Objects the slab layer has handed out: in use, or in a magazine.
    pub objects: usize,
    /// Slabs with no object handed out (at most one is kept).
    pub empty: usize,
}

/// One object cache: slabs of `size`-byte objects under one lock.
pub struct SlabCache {
    name: &'static str,
    size: usize,
    per_slab: usize,
    /// Objects of each slab that the slab's own descriptor occupies: 1 for
    /// the self-hosting descriptor cache, 0 otherwise.
    reserved: u32,
    lists: SpinLock<Lists>,
    slabs: AtomicUsize,
    objects: AtomicUsize,
    empty: AtomicUsize,
}

impl SlabCache {
    /// A cache of `size`-byte objects; `size` is rounded up to 16 and must
    /// fit a page.
    pub const fn new(name: &'static str, size: usize) -> Self {
        let size = size.next_multiple_of(STEP);
        assert!(size <= PAGE);
        Self::with(name, size, 0)
    }

    /// The descriptor cache, whose slabs hold their own descriptors.
    pub const fn descriptors() -> Self {
        Self::with("slab_descriptors", DESCRIPTOR_BYTES, 1)
    }

    const fn with(name: &'static str, size: usize, reserved: u32) -> Self {
        Self {
            name,
            size,
            per_slab: PAGE / size,
            reserved,
            lists: SpinLock::new(Lists { partial: List::new(), full: List::new(), empty: List::new() }),
            slabs: AtomicUsize::new(0),
            objects: AtomicUsize::new(0),
            empty: AtomicUsize::new(0),
        }
    }

    pub fn stats(&self) -> CacheStats {
        CacheStats {
            size: self.size,
            per_slab: self.per_slab,
            slabs: self.slabs.load(Ordering::Relaxed),
            objects: self.objects.load(Ordering::Relaxed),
            empty: self.empty.load(Ordering::Relaxed),
        }
    }

    /// The cache's lock, for the host model's acquisition counts.
    pub fn lock_counts(&self) -> (u64, u64) {
        (self.lists.acquisitions(), self.lists.contentions())
    }

    /// Take up to `out.len()` objects into `out`, growing by a page at a time.
    /// Returns how many it took, at least one. With `wait` false each lock is
    /// taken with one `try_lock`.
    pub fn take<B: Backing>(
        &self,
        out: &mut [*mut u8],
        wait: bool,
        backing: &B,
        descriptors: &SlabCache,
    ) -> Result<usize, Short> {
        let mut lists = self.lists.lock_if(wait).ok_or(Short::Busy)?;
        let mut taken = 0;
        let mut short = Short::NoPage;
        while taken < out.len() {
            let slab = if !lists.partial.head.is_null() {
                lists.partial.head
            } else if !lists.empty.head.is_null() {
                let slab = lists.empty.head;
                // SAFETY: the slab is on the empty list, which the lock guards.
                unsafe {
                    lists.empty.remove(slab);
                    lists.partial.link(slab, PARTIAL);
                }
                self.empty.fetch_sub(1, Ordering::Relaxed);
                slab
            } else {
                match self.grow(wait, backing, descriptors) {
                    Ok(slab) => {
                        // SAFETY: a new slab is on no list.
                        unsafe { lists.partial.link(slab, PARTIAL) };
                        slab
                    }
                    Err(reason) => {
                        short = reason;
                        break;
                    }
                }
            };
            // SAFETY: the slab is on the partial list, which the lock guards,
            // and its free objects are owned by the cache.
            unsafe {
                while taken < out.len() && !(*slab).free.is_null() {
                    let object = (*slab).free;
                    (*slab).free = object.cast::<*mut u8>().read();
                    (*slab).inuse += 1;
                    out[taken] = object;
                    taken += 1;
                }
                if (*slab).free.is_null() {
                    lists.partial.remove(slab);
                    lists.full.link(slab, FULL);
                }
            }
        }
        if taken == 0 {
            return Err(short);
        }
        self.objects.fetch_add(taken, Ordering::Relaxed);
        Ok(taken)
    }

    /// A slab over a new page. The caller holds this cache's lock.
    fn grow<B: Backing>(&self, wait: bool, backing: &B, descriptors: &SlabCache) -> Result<*mut Slab, Short> {
        let page = backing.take_page().ok_or(Short::NoPage)?;
        let slab: *mut Slab = if self.reserved == 1 {
            page.as_ptr().cast()
        } else {
            let mut one = [ptr::null_mut(); 1];
            if let Err(reason) = descriptors.take(&mut one, wait, backing, descriptors) {
                backing.give_page(page);
                return Err(reason);
            }
            one[0].cast()
        };
        let base = page.as_ptr();
        // SAFETY: the page and the descriptor are this cache's alone.
        unsafe {
            let mut free = ptr::null_mut();
            for index in (self.reserved as usize..self.per_slab).rev() {
                let object = base.add(index * self.size);
                object.cast::<*mut u8>().write(free);
                free = object;
            }
            slab.write(Slab {
                cache: self,
                base,
                free,
                inuse: self.reserved,
                capacity: self.per_slab as u32,
                prev: ptr::null_mut(),
                next: ptr::null_mut(),
                on: PARTIAL,
            });
        }
        backing.set_slab(page, slab);
        self.slabs.fetch_add(1, Ordering::Relaxed);
        Ok(slab)
    }

    /// Check that `object` is an object of this cache: its page's slab is
    /// this cache's, and it starts an object of that slab. A mismatch is
    /// memory corruption, not an allocation failure, and panics with both
    /// values. The check takes no lock, so a panic holds 0 allocator locks.
    pub fn check<B: Backing>(&self, object: NonNull<u8>, backing: &B) {
        let address = object.as_ptr() as usize;
        let slab = NonNull::new((address & !(PAGE - 1)) as *mut u8).map_or(ptr::null_mut(), |page| backing.slab(page));
        if slab.is_null() {
            panic!("slab: free of {:#x} to cache {} ({} bytes), whose page holds no slab", address, self.name, self.size);
        }
        // SAFETY: a recorded descriptor stays valid while its slab holds a
        // live object, and a live object is what the caller frees.
        let (cache, base, capacity) = unsafe { ((*slab).cache, (*slab).base, (*slab).capacity) };
        if !ptr::eq(cache, self) {
            // SAFETY: as above.
            let (name, size) = unsafe { ((*cache).name, (*cache).size) };
            panic!(
                "slab: free of {:#x} to cache {} ({} bytes), but its slab belongs to cache {} ({} bytes)",
                address, self.name, self.size, name, size
            );
        }
        let offset = address - base as usize;
        let index = offset / self.size;
        if offset % self.size != 0 || index < self.reserved as usize || index >= capacity as usize {
            panic!("slab: free of {:#x} to cache {} ({} bytes) is not the start of an object", address, self.name, self.size);
        }
    }

    /// Return checked objects to their slabs. An emptied slab is kept if
    /// the cache has no empty slab, and goes back to the page layer if it has
    /// one.
    pub fn put<B: Backing>(&self, objects: &[*mut u8], backing: &B, descriptors: &SlabCache) {
        if objects.is_empty() {
            return;
        }
        let mut lists = self.lists.lock();
        for &object in objects {
            let page = object as usize & !(PAGE - 1);
            // SAFETY: the caller checked the object, so its page records a
            // live slab of this cache, which the lock guards.
            unsafe {
                let slab = backing.slab(NonNull::new_unchecked(page as *mut u8));
                object.cast::<*mut u8>().write((*slab).free);
                (*slab).free = object;
                (*slab).inuse -= 1;
                if (*slab).on == FULL {
                    lists.full.remove(slab);
                    lists.partial.link(slab, PARTIAL);
                }
                if (*slab).inuse == self.reserved {
                    lists.partial.remove(slab);
                    if lists.empty.len == 0 {
                        lists.empty.link(slab, EMPTY);
                        self.empty.fetch_add(1, Ordering::Relaxed);
                    } else {
                        self.release(slab, backing, descriptors);
                    }
                }
            }
        }
        self.objects.fetch_sub(objects.len(), Ordering::Relaxed);
    }

    /// Give an empty slab's page back, and its descriptor. The caller holds
    /// this cache's lock and has unlinked the slab.
    unsafe fn release<B: Backing>(&self, slab: *mut Slab, backing: &B, descriptors: &SlabCache) {
        // SAFETY: the slab is unlinked and empty, so it is the caller's.
        let page = unsafe { NonNull::new_unchecked((*slab).base) };
        backing.set_slab(page, ptr::null_mut());
        if self.reserved == 0 {
            descriptors.put(&[slab.cast()], backing, descriptors);
        }
        backing.give_page(page);
        self.slabs.fetch_sub(1, Ordering::Relaxed);
    }

    /// Give each empty slab back, the one kept included (the shrinker, 6.2),
    /// if the cache's lock is free now. Returns the pages given back.
    pub fn release_empty<B: Backing>(&self, backing: &B, descriptors: &SlabCache) -> usize {
        let Some(mut lists) = self.lists.try_lock() else {
            return 0;
        };
        let mut pages = 0;
        while !lists.empty.head.is_null() {
            let slab = lists.empty.head;
            // SAFETY: the slab is on the empty list, which the lock guards.
            unsafe {
                lists.empty.remove(slab);
                self.release(slab, backing, descriptors);
            }
            self.empty.fetch_sub(1, Ordering::Relaxed);
            pages += 1;
        }
        pages
    }
}

//! Named caches for fixed-size kernel objects (E1 plan, 6.2).
//!
//! A named cache has a fixed object size and its own slabs from the slab
//! layer (`slab_cache.rs`), with no count limit. An object larger than a
//! page cannot sit in a single-page slab, so a cache of such objects takes
//! them from the allocator's larger-request layer instead, with the same
//! interface. The caches have no magazine layer: their objects are made once
//! per process, at fork and exec.
//!
//! Two caches are provided:
//! - `FD_TABLE_SLAB`: for `[Option<FileDescriptor>; 256]`
//! - `SIGNAL_HANDLERS_SLAB`: for `SignalTable`
//!
//! `SlabBox` returns an object to the cache it came from, or to the global
//! allocator for one made with `from_box`.

use alloc::boxed::Box;
use core::alloc::Layout;
use core::ops::{Deref, DerefMut};
use core::ptr::{self, NonNull};
use core::sync::atomic::{AtomicUsize, Ordering};

use super::kmem;
use super::slab_cache::{self, PAGE};

// ---------------------------------------------------------------------------
// SlabCache: a named cache
// ---------------------------------------------------------------------------

enum Objects {
    /// Objects up to a page, in the cache's own slabs.
    Slabs(slab_cache::SlabCache),
    /// Objects above a page, from the allocator with this layout.
    Large(Layout),
}

pub struct SlabCache {
    name: &'static str,
    obj_size: usize,
    objects: Objects,
    allocated: AtomicUsize,
}

/// Statistics for a named cache
pub struct SlabStats {
    pub name: &'static str,
    pub obj_size: usize,
    /// Objects the cache holds room for now: its slabs' objects, or for a
    /// cache above a page, its live objects.
    pub capacity: usize,
    pub allocated: usize,
    pub free: usize,
}

impl SlabCache {
    /// A cache of `obj_size`-byte objects aligned to `align`.
    pub const fn new(name: &'static str, obj_size: usize, align: usize) -> Self {
        // Slab objects sit at multiples of their 16-byte-rounded size.
        assert!(align <= 16);
        let objects = if obj_size <= PAGE {
            Objects::Slabs(slab_cache::SlabCache::new(name, obj_size))
        } else {
            match Layout::from_size_align(obj_size, align) {
                Ok(layout) => Objects::Large(layout),
                Err(_) => panic!("named cache layout"),
            }
        };
        SlabCache { name, obj_size, objects, allocated: AtomicUsize::new(0) }
    }

    /// Allocate an object from this cache.
    ///
    /// Returns a pointer to zeroed memory, if memory could be had.
    pub fn alloc(&self) -> Option<*mut u8> {
        let object = match &self.objects {
            Objects::Slabs(cache) => kmem::named_take(cache)?,
            Objects::Large(layout) => kmem::try_alloc(*layout, kmem::Ctx::Normal).ok()?,
        };
        // SAFETY: the object is `obj_size` bytes and the caller's alone.
        unsafe { ptr::write_bytes(object.as_ptr(), 0, self.obj_size) };
        self.allocated.fetch_add(1, Ordering::Relaxed);
        Some(object.as_ptr())
    }

    /// Return an object to this cache.
    ///
    /// # Safety
    ///
    /// `ptr` must have been returned by a previous call to `self.alloc()`.
    pub unsafe fn dealloc(&self, ptr: *mut u8) {
        let Some(object) = NonNull::new(ptr) else {
            return;
        };
        match &self.objects {
            Objects::Slabs(cache) => kmem::named_put(cache, object),
            // SAFETY: the object came from the allocator with this layout.
            Objects::Large(layout) => unsafe { alloc::alloc::dealloc(ptr, *layout) },
        }
        self.allocated.fetch_sub(1, Ordering::Relaxed);
    }

    /// Get statistics for this cache.
    pub fn stats(&self) -> SlabStats {
        let allocated = self.allocated.load(Ordering::Relaxed);
        let capacity = match &self.objects {
            Objects::Slabs(cache) => {
                let stats = cache.stats();
                stats.slabs * stats.per_slab
            }
            Objects::Large(_) => allocated,
        };
        SlabStats {
            name: self.name,
            obj_size: self.obj_size,
            capacity,
            allocated,
            free: capacity.saturating_sub(allocated),
        }
    }

    fn reclaimable(&self) -> usize {
        match &self.objects {
            Objects::Slabs(cache) => cache.stats().empty,
            Objects::Large(_) => 0,
        }
    }

    fn release_empty(&self) -> usize {
        match &self.objects {
            Objects::Slabs(cache) => kmem::named_release_empty(cache),
            Objects::Large(_) => 0,
        }
    }
}

// ---------------------------------------------------------------------------
// SlabBox<T>: smart pointer that returns memory to slab (or global heap)
// ---------------------------------------------------------------------------

/// A smart pointer that deallocates to a slab cache on drop.
///
/// If `slab` is `Some`, the memory was allocated from that slab and will be
/// returned there. If `slab` is `None`, the memory was allocated from the
/// global heap via `Box` and will be freed normally.
pub struct SlabBox<T: ?Sized> {
    ptr: NonNull<T>,
    slab: Option<&'static SlabCache>,
}

// SAFETY: SlabBox owns its data exclusively, like Box.
unsafe impl<T: ?Sized + Send> Send for SlabBox<T> {}
unsafe impl<T: ?Sized + Sync> Sync for SlabBox<T> {}

impl<T> SlabBox<T> {
    /// Wrap a slab-allocated pointer.
    ///
    /// # Safety
    ///
    /// `ptr` must point to a valid, initialized `T` that was allocated from `slab`.
    pub unsafe fn from_slab(ptr: *mut T, slab: &'static SlabCache) -> Self {
        SlabBox {
            ptr: NonNull::new_unchecked(ptr),
            slab: Some(slab),
        }
    }
}

impl<T: ?Sized> SlabBox<T> {
    /// Wrap a heap-allocated `Box` (slab = None, so Drop frees via global allocator).
    pub fn from_box(b: Box<T>) -> Self {
        let raw = Box::into_raw(b);
        SlabBox {
            // SAFETY: Box::into_raw always returns a non-null pointer.
            ptr: unsafe { NonNull::new_unchecked(raw) },
            slab: None,
        }
    }
}

impl<T, const N: usize> SlabBox<[T; N]> {
    /// An array on the global heap whose slot `i` holds `init(i)`, written in
    /// place. `Box::new` of an array value builds the whole array in the
    /// caller's frame first, and a 256-descriptor table copied there overflows
    /// a kernel stack.
    pub fn heap_from_fn(mut init: impl FnMut(usize) -> T) -> Self {
        let mut boxed = Box::<[T; N]>::new_uninit();
        let dst = boxed.as_mut_ptr() as *mut T;
        for i in 0..N {
            // SAFETY: `dst` addresses the box's N slots; each is written once.
            unsafe { dst.add(i).write(init(i)) };
        }
        // SAFETY: the loop above initialized every slot.
        SlabBox::from_box(unsafe { boxed.assume_init() })
    }
}

impl<T: ?Sized> Deref for SlabBox<T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T {
        unsafe { self.ptr.as_ref() }
    }
}

impl<T: ?Sized> DerefMut for SlabBox<T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        unsafe { self.ptr.as_mut() }
    }
}

impl<T: ?Sized> Drop for SlabBox<T> {
    fn drop(&mut self) {
        unsafe {
            // Run the destructor for T
            ptr::drop_in_place(self.ptr.as_ptr());

            if let Some(slab) = self.slab {
                // Return to slab
                slab.dealloc(self.ptr.as_ptr() as *mut u8);
            } else {
                // Reconstruct Box and let it free via global allocator
                let _ = Box::from_raw(self.ptr.as_ptr());
            }
        }
    }
}

impl<T: Clone> Clone for SlabBox<[T]> {
    fn clone(&self) -> Self {
        // Try to allocate from the same slab if we came from one
        if let Some(slab) = self.slab {
            if let Some(raw) = slab.alloc() {
                let src: &[T] = self;
                let dst = raw as *mut T;
                for i in 0..src.len() {
                    unsafe {
                        ptr::write(dst.add(i), src[i].clone());
                    }
                }
                // Reconstruct the fat pointer with the same length
                let fat: *mut [T] = ptr::slice_from_raw_parts_mut(dst, src.len());
                return SlabBox {
                    ptr: unsafe { NonNull::new_unchecked(fat) },
                    slab: Some(slab),
                };
            }
        }
        // Fall back to heap
        let boxed: Box<[T]> = self.deref().into();
        SlabBox::from_box(boxed)
    }
}

/// Clone for fixed-size arrays
impl<T: Clone, const N: usize> Clone for SlabBox<[T; N]> {
    fn clone(&self) -> Self {
        if let Some(slab) = self.slab {
            if let Some(raw) = slab.alloc() {
                let src: &[T; N] = self;
                let dst = raw as *mut T;
                for i in 0..N {
                    unsafe {
                        ptr::write(dst.add(i), src[i].clone());
                    }
                }
                return unsafe { SlabBox::from_slab(raw as *mut [T; N], slab) };
            }
        }
        // Fall back to heap
        SlabBox::heap_from_fn(|i| self[i].clone())
    }
}

/// Clone for the signal table (SignalState::fork)
impl Clone for SlabBox<SignalTable> {
    fn clone(&self) -> Self {
        if let Some(slab) = self.slab {
            if let Some(raw) = slab.alloc() {
                let dst = raw as *mut SignalTable;
                // SAFETY: the slab hands out blocks sized and aligned for a
                // SignalTable, which is plain data, copied in place.
                unsafe {
                    ptr::copy_nonoverlapping(self.ptr.as_ptr() as *const SignalTable, dst, 1);
                    return SlabBox::from_slab(dst, slab);
                }
            }
        }
        // Fall back to heap
        SlabBox::from_box(Box::new((**self).clone()))
    }
}

// ---------------------------------------------------------------------------
// Static cache declarations
// ---------------------------------------------------------------------------

use crate::ipc::fd::{FileDescriptor, INITIAL_FDS};
use crate::signal::types::SignalTable;
use core::mem::{align_of, size_of};

type FdArray = [Option<FileDescriptor>; INITIAL_FDS];

/// Cache for `[Option<FileDescriptor>; 256]`.
/// Used by `FdTable::new()` and `FdTable::clone()` (fork).
pub static FD_TABLE_SLAB: SlabCache =
    SlabCache::new("fd_table", size_of::<FdArray>(), align_of::<FdArray>());

/// Cache for `SignalTable`.
/// Used by `SignalState::default()` and `SignalState::fork()`.
pub static SIGNAL_HANDLERS_SLAB: SlabCache =
    SlabCache::new("signal_handlers", size_of::<SignalTable>(), align_of::<SignalTable>());

fn named() -> [&'static SlabCache; 2] {
    [&FD_TABLE_SLAB, &SIGNAL_HANDLERS_SLAB]
}

/// Pages the named caches could give back now: their empty slabs.
pub(super) fn reclaimable() -> usize {
    named().iter().map(|cache| cache.reclaimable()).sum()
}

/// Give back the named caches' empty slabs, for the slab shrinker.
pub(super) fn release_empty() -> usize {
    named().iter().map(|cache| cache.release_empty()).sum()
}

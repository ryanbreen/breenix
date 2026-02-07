//! Slab allocator for fixed-size kernel objects
//!
//! Provides O(1) allocation and deallocation for frequently created/destroyed
//! fixed-size objects. Uses bitmap-based free tracking following the pattern
//! from `kernel_stack.rs`.
//!
//! Two static caches are provided:
//! - `FD_TABLE_SLAB`: for `[Option<FileDescriptor>; 256]` (~6 KiB each, 64 slots)
//! - `SIGNAL_HANDLERS_SLAB`: for `[SignalAction; 64]` (~2 KiB each, 64 slots)
//!
//! Objects that cannot be served from the slab fall back to the global heap
//! allocator transparently via `SlabBox`.

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::alloc::Layout;
use core::ops::{Deref, DerefMut};
use core::ptr::{self, NonNull};
use spin::Mutex;

// ---------------------------------------------------------------------------
// SlabCache: bitmap-based object pool
// ---------------------------------------------------------------------------

struct SlabCacheInner {
    /// Heap-allocated backing buffer
    storage: *mut u8,
    /// Per-object size (8-byte aligned)
    obj_size: usize,
    /// Maximum number of objects
    capacity: usize,
    /// 1 = allocated, 0 = free
    bitmap: Vec<u64>,
    /// Count of live objects
    allocated: usize,
    /// Whether the cache has been initialized
    initialized: bool,
}

// SAFETY: The storage pointer is heap-allocated and only accessed under the
// mutex lock. We need Send for the Mutex<SlabCacheInner>.
unsafe impl Send for SlabCacheInner {}

pub struct SlabCache {
    inner: Mutex<SlabCacheInner>,
    name: &'static str,
}

/// Statistics for a slab cache
pub struct SlabStats {
    pub name: &'static str,
    pub obj_size: usize,
    pub capacity: usize,
    pub allocated: usize,
    pub free: usize,
}

impl SlabCache {
    /// Create an uninitialized slab cache for static declaration.
    ///
    /// Both `Vec::new()` and `Mutex::new()` are const, so this works in statics.
    pub const fn uninit(name: &'static str) -> Self {
        SlabCache {
            inner: Mutex::new(SlabCacheInner {
                storage: ptr::null_mut(),
                obj_size: 0,
                capacity: 0,
                bitmap: Vec::new(),
                allocated: 0,
                initialized: false,
            }),
            name,
        }
    }

    /// Initialize the slab cache, allocating backing storage from the global heap.
    ///
    /// `obj_size` is the size of each object (will be rounded up to 8-byte alignment).
    /// `capacity` is the maximum number of objects.
    pub fn init(&self, obj_size: usize, capacity: usize) {
        let obj_size_aligned = (obj_size + 7) & !7;
        let total_bytes = obj_size_aligned * capacity;
        let bitmap_words = (capacity + 63) / 64;

        // Allocate backing storage from the global heap
        let layout = Layout::from_size_align(total_bytes, 8)
            .expect("slab: invalid layout");
        let storage = unsafe { alloc::alloc::alloc_zeroed(layout) };
        if storage.is_null() {
            panic!("slab: failed to allocate {} bytes for cache '{}'", total_bytes, self.name);
        }

        let mut inner = self.inner.lock();
        inner.storage = storage;
        inner.obj_size = obj_size_aligned;
        inner.capacity = capacity;
        inner.bitmap = alloc::vec![0u64; bitmap_words];
        inner.allocated = 0;
        inner.initialized = true;

        log::info!(
            "Slab cache '{}' initialized: {} slots x {} bytes = {} KiB",
            self.name,
            capacity,
            obj_size_aligned,
            total_bytes / 1024
        );
    }

    /// Allocate a slot from this cache.
    ///
    /// Returns a pointer to zeroed memory, or `None` if the cache is full
    /// or not yet initialized.
    pub fn alloc(&self) -> Option<*mut u8> {
        let mut inner = self.inner.lock();
        if !inner.initialized {
            return None;
        }

        // Cache fields before mutable borrow of bitmap
        let capacity = inner.capacity;
        let obj_size = inner.obj_size;
        let storage = inner.storage;

        // Scan bitmap for a free slot
        for (word_idx, word) in inner.bitmap.iter_mut().enumerate() {
            if *word != u64::MAX {
                for bit_idx in 0..64 {
                    let global_idx = word_idx * 64 + bit_idx;
                    if global_idx >= capacity {
                        return None;
                    }
                    if (*word & (1u64 << bit_idx)) == 0 {
                        // Mark as allocated
                        *word |= 1u64 << bit_idx;
                        inner.allocated += 1;

                        let offset = global_idx * obj_size;
                        let ptr = unsafe { storage.add(offset) };
                        // Zero the slot (defense in depth)
                        unsafe { ptr::write_bytes(ptr, 0, obj_size); }
                        return Some(ptr);
                    }
                }
            }
        }
        None
    }

    /// Deallocate a slot back to this cache.
    ///
    /// # Safety
    ///
    /// `ptr` must have been returned by a previous call to `self.alloc()`.
    pub unsafe fn dealloc(&self, ptr: *mut u8) {
        let mut inner = self.inner.lock();
        debug_assert!(inner.initialized, "slab dealloc on uninitialized cache");

        let offset = (ptr as usize).wrapping_sub(inner.storage as usize);
        let slot = offset / inner.obj_size;

        debug_assert!(
            slot < inner.capacity,
            "slab dealloc: pointer {:#x} out of range for cache '{}'",
            ptr as usize,
            self.name
        );
        debug_assert_eq!(
            offset % inner.obj_size,
            0,
            "slab dealloc: misaligned pointer for cache '{}'",
            self.name
        );

        let word_idx = slot / 64;
        let bit_idx = slot % 64;
        debug_assert!(
            (inner.bitmap[word_idx] & (1u64 << bit_idx)) != 0,
            "slab dealloc: double free in cache '{}'",
            self.name
        );

        inner.bitmap[word_idx] &= !(1u64 << bit_idx);
        inner.allocated -= 1;
    }

    /// Get statistics for this cache.
    pub fn stats(&self) -> SlabStats {
        let inner = self.inner.lock();
        SlabStats {
            name: self.name,
            obj_size: inner.obj_size,
            capacity: inner.capacity,
            allocated: inner.allocated,
            free: inner.capacity.saturating_sub(inner.allocated),
        }
    }
}

// SAFETY: SlabCache uses a Mutex internally.
unsafe impl Sync for SlabCache {}

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

/// Clone for sized types (used by SignalState handlers array)
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
        SlabBox::from_box(Box::new((**self).clone()))
    }
}

// ---------------------------------------------------------------------------
// Static cache declarations and initialization
// ---------------------------------------------------------------------------

use crate::ipc::fd::FileDescriptor;
use crate::signal::types::SignalAction;

/// Slab cache for `[Option<FileDescriptor>; 256]` (~6 KiB each).
/// Used by `FdTable::new()` and `FdTable::clone()` (fork).
pub static FD_TABLE_SLAB: SlabCache = SlabCache::uninit("fd_table");

/// Slab cache for `[SignalAction; 64]` (~2 KiB each).
/// Used by `SignalState::default()` and `SignalState::fork()`.
pub static SIGNAL_HANDLERS_SLAB: SlabCache = SlabCache::uninit("signal_handlers");

/// Initialize all slab caches.
///
/// Must be called after the global heap allocator is initialized.
pub fn init() {
    use crate::ipc::fd::MAX_FDS;
    use core::mem::size_of;

    FD_TABLE_SLAB.init(
        size_of::<[Option<FileDescriptor>; MAX_FDS]>(),
        64,
    );
    SIGNAL_HANDLERS_SLAB.init(
        size_of::<[SignalAction; 64]>(),
        64,
    );
}

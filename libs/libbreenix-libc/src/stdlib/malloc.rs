//! malloc, free, calloc, realloc, aligned_alloc and posix_memalign (ISO C17 7.22.3,
//! POSIX.1-2024).
//!
//! Every block is its own anonymous mapping, except that blocks of up to 2 KiB are
//! recycled through alloc_cache. A 16-byte header sits in front of each block: the
//! requested size, then either alloc_cache's class marker, the mapping's base address
//! (for over-aligned blocks), or 0 (a plain mapping that starts at the header).
//! Results are 16-byte aligned, which covers max_align_t on both architectures. Every
//! failure sets errno to ENOMEM.

use super::alloc_cache;
use super::{set_errno, EINVAL, ENOMEM};
use crate::{mmap, munmap, MAP_ANONYMOUS, MAP_FAILED, MAP_PRIVATE, PROT_READ, PROT_WRITE};

const HEADER: usize = 16;

/// No object may be larger than PTRDIFF_MAX; refusing larger requests here also keeps
/// impossible lengths away from the kernel.
const LIMIT: usize = isize::MAX as usize - 4096;

unsafe fn header(ptr: *mut u8) -> *mut usize {
    ptr.sub(HEADER) as *mut usize
}

unsafe fn map(len: usize) -> *mut u8 {
    let p = mmap(core::ptr::null_mut(), len, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if p == MAP_FAILED { core::ptr::null_mut() } else { p }
}

#[no_mangle]
pub unsafe extern "C" fn malloc(size: usize) -> *mut u8 {
    if size > LIMIT {
        set_errno(ENOMEM);
        return core::ptr::null_mut();
    }
    // malloc(0) returns a unique block that may be passed to free.
    let class = alloc_cache::class(size.max(1));
    if let Some(class) = class {
        let ptr = alloc_cache::take(class);
        if !ptr.is_null() {
            *header(ptr) = size;
            return ptr;
        }
    }
    let capacity = class.map_or(size, alloc_cache::capacity);
    let base = map(capacity + HEADER);
    if base.is_null() {
        set_errno(ENOMEM);
        return base;
    }
    *(base as *mut usize) = size;
    *(base as *mut usize).add(1) = class.map_or(0, alloc_cache::marker);
    base.add(HEADER)
}

/// Unmap the blocks the allocation cache keeps for reuse; returns how many.
/// The cache holds free memory: a process that measures what it still uses (the
/// suite runner's memory record) releases it first. Breenix-specific.
#[no_mangle]
pub unsafe extern "C" fn __breenix_malloc_release_cached() -> usize {
    alloc_cache::drain(|class, ptr| {
        munmap(header(ptr) as *mut u8, alloc_cache::capacity(class) + HEADER);
    })
}

#[no_mangle]
pub unsafe extern "C" fn free(ptr: *mut u8) {
    if ptr.is_null() {
        return;
    }
    let h = header(ptr);
    let size = *h;
    let tag = *h.add(1);
    if let Some(class) = alloc_cache::marked_class(tag) {
        if !alloc_cache::put(class, ptr) {
            munmap(h as *mut u8, alloc_cache::capacity(class) + HEADER);
        }
    } else if tag != 0 && tag <= h as usize {
        // An over-aligned block: `tag` holds the mapping base, whose first
        // word records its full length, including unused trailing alignment room.
        munmap(tag as *mut u8, *(tag as *const usize));
    } else {
        munmap(h as *mut u8, size + HEADER);
    }
}

#[no_mangle]
pub unsafe extern "C" fn calloc(n: usize, size: usize) -> *mut u8 {
    let Some(total) = n.checked_mul(size) else {
        set_errno(ENOMEM);
        return core::ptr::null_mut();
    };
    let p = malloc(total);
    if !p.is_null() {
        // Recycled blocks keep their old contents.
        crate::string::memset(p, 0, total);
    }
    p
}

/// A size of 0 is treated as a request for a minimal block, so realloc(p, 0) frees p
/// and returns a unique pointer. A failed realloc leaves the block untouched.
#[no_mangle]
pub unsafe extern "C" fn realloc(ptr: *mut u8, size: usize) -> *mut u8 {
    if ptr.is_null() {
        return malloc(size);
    }
    if size > LIMIT {
        set_errno(ENOMEM);
        return core::ptr::null_mut();
    }
    let h = header(ptr);
    let old = *h;
    if alloc_cache::marked_class(*h.add(1)).is_some_and(|c| size.max(1) <= alloc_cache::capacity(c)) {
        *h = size;
        return ptr;
    }
    let new = malloc(size);
    if !new.is_null() {
        crate::string::memcpy(new, ptr, old.min(size));
        free(ptr);
    }
    new
}

/// The block for an alignment above 16: a mapping with room to move the user pointer up
/// to the alignment, with the header in front of it recording the mapping's base.
unsafe fn aligned(alignment: usize, size: usize) -> *mut u8 {
    if alignment <= HEADER {
        return malloc(size);
    }
    let Some(len) = alignment.checked_add(size).and_then(|n| n.checked_add(2 * HEADER)).filter(|&n| n <= LIMIT) else {
        set_errno(ENOMEM);
        return core::ptr::null_mut();
    };
    let base = map(len);
    if base.is_null() {
        set_errno(ENOMEM);
        return base;
    }
    // Store the full mapping length before the movable header; even the earliest
    // aligned pointer leaves this word outside that header.
    *(base as *mut usize) = len;
    let user = (base as usize + 2 * HEADER + alignment - 1) & !(alignment - 1);
    let h = (user - HEADER) as *mut usize;
    *h = size;
    *h.add(1) = base as usize;
    user as *mut u8
}

#[no_mangle]
pub unsafe extern "C" fn posix_memalign(out: *mut *mut u8, alignment: usize, size: usize) -> i32 {
    if !alignment.is_power_of_two() || alignment < core::mem::size_of::<usize>() {
        return EINVAL;
    }
    // posix_memalign reports through its return value and leaves errno alone.
    let saved = super::errno();
    let p = aligned(alignment, size);
    set_errno(saved);
    if p.is_null() {
        return ENOMEM;
    }
    *out = p;
    0
}

#[no_mangle]
pub unsafe extern "C" fn aligned_alloc(alignment: usize, size: usize) -> *mut u8 {
    if !alignment.is_power_of_two() {
        set_errno(EINVAL);
        return core::ptr::null_mut();
    }
    aligned(alignment, size)
}

#[no_mangle]
pub unsafe extern "C" fn memalign(alignment: usize, size: usize) -> *mut u8 {
    aligned_alloc(alignment, size)
}

/// realloc for n elements of `size` bytes, failing with ENOMEM when n * size overflows.
#[no_mangle]
pub unsafe extern "C" fn reallocarray(ptr: *mut u8, n: usize, size: usize) -> *mut u8 {
    match n.checked_mul(size) {
        Some(total) => realloc(ptr, total),
        None => {
            set_errno(ENOMEM);
            core::ptr::null_mut()
        }
    }
}

/// The bytes usable at `ptr`: the recycled block's capacity, or the size requested for a
/// mapping of its own.
#[no_mangle]
pub unsafe extern "C" fn malloc_usable_size(ptr: *mut u8) -> usize {
    if ptr.is_null() {
        return 0;
    }
    let h = header(ptr);
    match alloc_cache::marked_class(*h.add(1)) {
        Some(class) => alloc_cache::capacity(class),
        None => *h,
    }
}

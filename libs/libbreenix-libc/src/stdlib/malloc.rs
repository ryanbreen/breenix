//! malloc, free, calloc, realloc, aligned_alloc and posix_memalign (ISO C17 7.22.3,
//! POSIX.1-2024).
//!
//! Small and mid-size blocks share size-class arenas. Large or over-aligned
//! blocks have their own mappings. A 16-byte header records the requested size
//! and a tag: an arena pointer with its low bit set, an over-aligned mapping's
//! base, or `0` for a mapping starting at the header.

use super::alloc_cache::{self, ALIGNMENT, HEADER};
use super::{set_errno, EINVAL, ENOMEM};
use crate::{mmap, munmap, MAP_ANONYMOUS, MAP_FAILED, MAP_PRIVATE, PROT_READ, PROT_WRITE};

/// No object may be larger than PTRDIFF_MAX; refusing larger requests here also keeps
/// impossible lengths away from the kernel.
const LIMIT: usize = isize::MAX as usize - 4096;

unsafe fn header(ptr: *mut u8) -> *mut usize {
    ptr.sub(HEADER) as *mut usize
}

unsafe fn map(len: usize) -> *mut u8 {
    let p = mmap(
        core::ptr::null_mut(),
        len,
        PROT_READ | PROT_WRITE,
        MAP_PRIVATE | MAP_ANONYMOUS,
        -1,
        0,
    );
    if p == MAP_FAILED {
        core::ptr::null_mut()
    } else {
        p
    }
}

#[no_mangle]
pub unsafe extern "C" fn malloc(size: usize) -> *mut u8 {
    if size > LIMIT {
        set_errno(ENOMEM);
        return core::ptr::null_mut();
    }
    // malloc(0) returns a unique block that may be passed to free.
    if let Some(class) = alloc_cache::class(size.max(1)) {
        if let Some((ptr, tag)) = alloc_cache::take(class, |len| map(len)) {
            let h = header(ptr);
            *h = size;
            *h.add(1) = tag;
            return ptr;
        }
    }
    let base = map(size.max(1) + HEADER);
    if base.is_null() {
        set_errno(ENOMEM);
        return base;
    }
    *(base as *mut usize) = size;
    *(base as *mut usize).add(1) = 0;
    base.add(HEADER)
}

/// Release free arena pages before the suite runner measures resident memory.
/// Returns the number of mappings released. Breenix-specific.
#[no_mangle]
pub unsafe extern "C" fn __breenix_malloc_release_cached() -> usize {
    const MADV_DONTNEED: u64 = 4;
    alloc_cache::drain(
        |ptr, len| unmap(ptr, len),
        |ptr, len| {
            libbreenix::raw::syscall3(
                libbreenix::syscall::nr::MADVISE,
                ptr as u64,
                len as u64,
                MADV_DONTNEED,
            ) == 0
        },
    )
}

unsafe fn unmap(ptr: *mut u8, len: usize) -> bool {
    let saved = super::errno();
    let result = munmap(ptr, len);
    set_errno(saved);
    result == 0
}

#[no_mangle]
pub unsafe extern "C" fn free(ptr: *mut u8) {
    if ptr.is_null() {
        return;
    }
    let h = header(ptr);
    let tag = *h.add(1);
    if alloc_cache::is_arena_tag(tag) {
        alloc_cache::put(tag, ptr, |base, len| unmap(base, len));
    } else if tag != 0 {
        // An over-aligned block: the mapping base stores the full length.
        let base = core::ptr::with_exposed_provenance_mut::<usize>(tag);
        unmap(base.cast(), *base);
    } else {
        unmap(h.cast(), (*h).max(1) + HEADER);
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
    let tag = *h.add(1);
    if alloc_cache::is_arena_tag(tag) && size.max(1) <= alloc_cache::block_capacity(tag) {
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
    if alignment <= ALIGNMENT {
        return malloc(size);
    }
    let Some(len) = alignment
        .checked_add(size)
        .and_then(|n| n.checked_add(2 * HEADER))
        .filter(|&n| n <= LIMIT)
    else {
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
    let start = base.add(2 * HEADER);
    let user = start.add(start.align_offset(alignment));
    let h = header(user);
    *h = size;
    *h.add(1) = base.expose_provenance();
    user
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
    let tag = *h.add(1);
    if alloc_cache::is_arena_tag(tag) {
        alloc_cache::block_capacity(tag)
    } else {
        *h
    }
}

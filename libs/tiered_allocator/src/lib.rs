//! This is a ported version of https://github.com/gz/rust-slabmalloc
//! to integrate with the memory infrastructure in phillop's kernel.
//!
//! A slab allocator implementation for small objects
//! (< architecture page size).
//!
//! The organization is as follows (top-down):
//!
//!  * A `ZoneAllocator` manages many `SlabAllocator` and can
//!    satisfy requests for different allocation sizes.
//!  * A `SlabAllocator` allocates objects of exactly one size.
//!    It holds its data in a SlabList.
//!  * A `SlabPage` contains allocated objects and associated meta-data.
//!  * A `SlabPageProvider` is provided by the client and used by the
//!    SlabAllocator to allocate SlabPages.
//!
#![feature(allocator)]
#![allow(unused_features, dead_code, unused_variables)]
#![feature(const_fn, prelude_import, test, raw, ptr_as_ref, core_prelude, core_slice_ext, libc)]
#![no_std]
#![allocator]

use core::mem;
use core::ptr;
use core::fmt;

extern crate spin;
use spin::Mutex;

pub const HEAP_START: usize = 0o_000_001_000_000_0000;
pub const HEAP_SIZE: usize = 100 * 1024; // 100 KiB

/// Align downwards. Returns the greatest x with alignment `align`
/// so that x <= addr. The alignment must be a power of 2.
pub fn align_down(addr: usize, align: usize) -> usize {
    if align.is_power_of_two() {
        addr & !(align - 1)
    } else if align == 0 {
        addr
    } else {
        panic!("`align` must be a power of 2");
    }
}

/// Align upwards. Returns the smallest x with alignment `align`
/// so that x >= addr. The alignment must be a power of 2.
pub fn align_up(addr: usize, align: usize) -> usize {
    align_down(addr + align - 1, align)
}

#[derive(Debug)]
struct BumpAllocator {
    heap_start: usize,
    heap_size: usize,
    next: usize,
}

impl BumpAllocator {
    /// Create a new allocator, which uses the memory in the
    /// range [heap_start, heap_start + heap_size).
    const fn new(heap_start: usize, heap_size: usize) -> BumpAllocator {
        BumpAllocator {
            heap_start: heap_start,
            heap_size: heap_size,
            next: heap_start,
        }
    }

    /// Allocates a block of memory with the given size and alignment.
    fn allocate(&mut self, size: usize, align: usize) -> Option<*mut u8> {
        let alloc_start = align_up(self.next, align);
        let alloc_end = alloc_start + size;

        if alloc_end <= self.heap_start + self.heap_size {
            self.next = alloc_end;
            Some(alloc_start as *mut u8)
        } else {
            None
        }
    }
}

static BUMP_ALLOCATOR: Mutex<BumpAllocator> = Mutex::new(
    BumpAllocator::new(HEAP_START, HEAP_SIZE));

static mut SLAB_ALLOCATE: Option<fn(usize, usize) -> *mut u8> = None;
static mut SLAB_DEALLOCATE: Option<fn()> = None;

pub fn init(allocate: fn(usize, usize) -> *mut u8) {
  unsafe {
    SLAB_ALLOCATE = Some(allocate);
  }
}

#[no_mangle]
pub extern fn __rust_allocate(size: usize, align: usize) -> *mut u8 {
  unsafe {
    if SLAB_ALLOCATE.is_none() {
      bootstrap_allocate(size, align)
    } else {
      slab_allocate(size, align)
    }
  }
}

fn bootstrap_allocate(size: usize, align: usize) -> *mut u8 {
  BUMP_ALLOCATOR.lock().allocate(size, align).expect("out of memory")
}

fn slab_allocate(size: usize, align: usize) -> *mut u8 {
  unsafe {
    SLAB_ALLOCATE.expect("invalid allocate")(size, align)
  }
}

#[no_mangle]
pub extern fn __rust_deallocate(ptr: *mut u8, size: usize, align: usize) {
  
}

#[no_mangle]
pub extern fn __rust_usable_size(size: usize, _align: usize) -> usize {
  size
}

#[no_mangle]
pub extern fn __rust_reallocate_inplace(_ptr: *mut u8, size: usize,
    _new_size: usize, _align: usize) -> usize
{
  size
}

#[no_mangle]
pub extern fn __rust_reallocate(ptr: *mut u8, size: usize, new_size: usize,
                                align: usize) -> *mut u8 {
  use core::{ptr, cmp};

  // from: https://github.com/rust-lang/rust/blob/
  //     c66d2380a810c9a2b3dbb4f93a830b101ee49cc2/
  //     src/liballoc_system/lib.rs#L98-L101

  unsafe {
    if ptr as usize >= HEAP_START && ptr as usize <= HEAP_START + (HEAP_SIZE * 8) {
      let new_ptr = bootstrap_allocate(new_size, align);
      ptr::copy(ptr, new_ptr, cmp::min(size, new_size));
      new_ptr
    } else {
      let new_ptr = __rust_allocate(new_size, align);
      ptr::copy(ptr, new_ptr, cmp::min(size, new_size));
      __rust_deallocate(ptr, size, align);
      new_ptr
    }
  }
}

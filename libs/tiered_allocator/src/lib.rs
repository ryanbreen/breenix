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
#![feature(const_fn, allocator_api, alloc, global_allocator, ptr_internals, unique)]
#![no_std]

extern crate alloc;
extern crate spin;

#[macro_use]
extern crate lazy_static;

use heap::Heap;

use alloc::alloc::{GlobalAlloc, Layout};

use spin::Mutex;

mod heap;
mod hole;

pub const HEAP_START: usize = 0o_000_001_000_000_0000;
pub const HEAP_SIZE: usize = 1024 * 1024; // 1MB

lazy_static! {
    static ref HEAP: Mutex<Heap> = Mutex::new(unsafe {
        Heap::new(HEAP_START, HEAP_SIZE)
    });
}

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

static mut SLAB_ALLOCATE: Option<fn(usize, usize) -> *mut u8> = None;
static mut SLAB_DEALLOCATE: Option<fn()> = None;

pub fn init(allocate: fn(usize, usize) -> *mut u8) {
    unsafe {
        SLAB_ALLOCATE = Some(allocate);
    }
}

pub struct Allocator;

unsafe impl GlobalAlloc for & Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if SLAB_ALLOCATE.is_none() {
            bootstrap_allocate(layout.size(), layout.align())
        } else {
            slab_allocate(layout.size(), layout.align())
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        
    }
}

//Our allocator static
#[global_allocator]
static GLOBAL_ALLOC: Allocator = Allocator;

pub static mut BOOTSTRAP_ALLOCS: usize = 0;
pub static mut BOOTSTRAP_ALLOC_SIZE: usize = 0;

fn bootstrap_allocate(size: usize, align: usize) -> *mut u8 {
    // HEAP.lock().allocate_first_fit(size, align).expect("out of bootstrap memory")
    unsafe {
        BOOTSTRAP_ALLOCS += 1;
        BOOTSTRAP_ALLOC_SIZE += size;

        if size > 128000 {
            BOOTSTRAP_ALLOC_SIZE += 0;
        }
    }

    HEAP.lock().allocate_first_fit(size, align)
}

unsafe fn slab_allocate(size: usize, align: usize) -> *mut u8 {
    SLAB_ALLOCATE.expect("invalid allocate")(size, align)
}

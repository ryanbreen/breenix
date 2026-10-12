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
#![no_std]

extern crate alloc;
extern crate spin;

#[macro_use]
extern crate lazy_static;

use crate::heap::Heap;

use alloc::alloc::{GlobalAlloc, Layout};

use core::sync::atomic::{AtomicUsize, Ordering};
use spin::Mutex;

pub mod heap;
mod hole;

pub const HEAP_START: usize = 0o_000_001_000_000_0000;
pub const HEAP_SIZE: usize = 1024 * 1024; // 1MB

lazy_static! {
    static ref HEAP: Mutex<Heap> = Mutex::new(unsafe { Heap::new(HEAP_START, HEAP_SIZE) });
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
static mut SLAB_DEALLOCATE: Option<unsafe fn(*mut u8, usize, usize)> = None;

/// Install paired slab allocation and deallocation callbacks.
///
/// # Safety
/// Call before concurrent allocations begin. Callbacks must accept the same layouts.
pub unsafe fn init(
    allocate: fn(usize, usize) -> *mut u8,
    deallocate: unsafe fn(*mut u8, usize, usize),
) {
    unsafe {
        SLAB_ALLOCATE = Some(allocate);
        SLAB_DEALLOCATE = Some(deallocate);
    }
}

pub struct Allocator;

unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if core::ptr::addr_of!(SLAB_ALLOCATE).read().is_none() {
            bootstrap_allocate(layout.size(), layout.align())
        } else {
            slab_allocate(layout.size(), layout.align())
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if (ptr as usize) >= HEAP_START && (ptr as usize) < HEAP_START + HEAP_SIZE {
            HEAP.lock().deallocate(ptr, layout.size(), layout.align());
        } else {
            core::ptr::addr_of!(SLAB_DEALLOCATE)
                .read()
                .expect("slab deallocator not initialized")(
                ptr, layout.size(), layout.align()
            );
        }
    }
}

// The fixed bootstrap address is mapped by the kernel, never by a host test process.
#[cfg(any(target_os = "none", target_os = "breenix"))]
#[global_allocator]
static GLOBAL_ALLOC: Allocator = Allocator;

pub static BOOTSTRAP_ALLOCS: AtomicUsize = AtomicUsize::new(0);
pub static BOOTSTRAP_ALLOC_SIZE: AtomicUsize = AtomicUsize::new(0);

fn bootstrap_allocate(size: usize, align: usize) -> *mut u8 {
    BOOTSTRAP_ALLOCS.fetch_add(1, Ordering::Relaxed);
    BOOTSTRAP_ALLOC_SIZE.fetch_add(size, Ordering::Relaxed);

    HEAP.lock().allocate_first_fit(size, align)
}

unsafe fn slab_allocate(size: usize, align: usize) -> *mut u8 {
    core::ptr::addr_of!(SLAB_ALLOCATE)
        .read()
        .expect("invalid allocate")(size, align)
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use core::sync::atomic::{AtomicBool, Ordering};

    static FREED: AtomicBool = AtomicBool::new(false);

    fn allocate(size: usize, align: usize) -> *mut u8 {
        unsafe { std::alloc::alloc(Layout::from_size_align(size, align).unwrap()) }
    }

    unsafe fn deallocate(ptr: *mut u8, size: usize, align: usize) {
        assert_eq!((size, align), (48, 16));
        std::alloc::dealloc(ptr, Layout::from_size_align(size, align).unwrap());
        FREED.store(true, Ordering::SeqCst);
    }

    #[test]
    fn slab_callbacks_allocate_and_deallocate() {
        let allocator = Allocator;
        let layout = Layout::from_size_align(48, 16).unwrap();
        unsafe {
            init(allocate, deallocate);
            let ptr = allocator.alloc(layout);
            assert!(!ptr.is_null());
            assert_eq!(ptr as usize % 16, 0);
            ptr.write_bytes(0xAA, layout.size());
            allocator.dealloc(ptr, layout);
        }
        assert!(FREED.load(Ordering::SeqCst));
    }
}

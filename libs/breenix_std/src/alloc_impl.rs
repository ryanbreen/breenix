//! Global memory allocator for Breenix userspace programs.
//!
//! This module provides a simple bump allocator that uses the brk() syscall
//! to extend the heap. It implements the GlobalAlloc trait, allowing Rust's
//! alloc crate (Vec, Box, String, etc.) to work in userspace.
//!
//! # Design
//!
//! The allocator uses a simple bump allocation strategy:
//! - Allocations extend the heap via brk()
//! - Deallocations are no-ops (memory is not reclaimed)
//! - This is simple and fast, but wastes memory for long-running programs
//!
//! For production use, consider implementing a proper allocator (e.g., dlmalloc,
//! linked-list allocator, or buddy allocator).

use core::alloc::{GlobalAlloc, Layout};
use core::sync::atomic::{AtomicU64, Ordering};
use libbreenix::memory::{brk, get_brk};

/// Simple bump allocator for Breenix userspace.
///
/// This allocator extends the heap via brk() for each allocation.
/// Deallocations are no-ops - memory is not reclaimed.
pub struct BreenixAllocator {
    /// Current heap position (atomic for thread safety placeholder)
    heap_pos: AtomicU64,
    /// Whether the allocator has been initialized
    initialized: AtomicU64,
}

impl BreenixAllocator {
    /// Create a new uninitialized allocator.
    pub const fn new() -> Self {
        Self {
            heap_pos: AtomicU64::new(0),
            initialized: AtomicU64::new(0),
        }
    }

    /// Initialize the allocator by querying the current program break.
    fn init(&self) {
        if self.initialized.load(Ordering::Relaxed) == 0 {
            let current_brk = get_brk();
            self.heap_pos.store(current_brk, Ordering::Relaxed);
            self.initialized.store(1, Ordering::Relaxed);
        }
    }
}

unsafe impl GlobalAlloc for BreenixAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        self.init();

        let size = layout.size();
        let align = layout.align();

        // Get current heap position
        let current = self.heap_pos.load(Ordering::Relaxed);

        // Align the allocation
        let aligned = (current + align as u64 - 1) & !(align as u64 - 1);
        let new_end = aligned + size as u64;

        // Extend the heap
        let new_brk = brk(new_end);
        if new_brk < new_end {
            // Allocation failed
            return core::ptr::null_mut();
        }

        // Update heap position
        self.heap_pos.store(new_end, Ordering::Relaxed);

        aligned as *mut u8
    }

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {
        // Bump allocator doesn't reclaim memory
        // This is intentional for simplicity
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // Simple realloc: allocate new, copy, don't free old
        let new_layout = Layout::from_size_align_unchecked(new_size, layout.align());
        let new_ptr = self.alloc(new_layout);
        if !new_ptr.is_null() {
            core::ptr::copy_nonoverlapping(ptr, new_ptr, core::cmp::min(layout.size(), new_size));
        }
        new_ptr
    }
}

/// The global allocator instance.
#[global_allocator]
pub static ALLOCATOR: BreenixAllocator = BreenixAllocator::new();

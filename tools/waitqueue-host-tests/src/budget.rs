//! A global allocator that fails once a per-thread allocation budget is
//! spent, so out-of-memory paths run the real `try_reserve` failure.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

std::thread_local! {
    static LEFT: Cell<Option<usize>> = const { Cell::new(None) };
}

fn allowed() -> bool {
    LEFT.try_with(|left| match left.get() {
        None => true,
        Some(0) => false,
        Some(n) => {
            left.set(Some(n - 1));
            true
        }
    })
    .unwrap_or(true)
}

struct BudgetAllocator;

unsafe impl GlobalAlloc for BudgetAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if allowed() {
            System.alloc(layout)
        } else {
            core::ptr::null_mut()
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout)
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if allowed() {
            System.realloc(ptr, layout, size)
        } else {
            core::ptr::null_mut()
        }
    }
}

#[global_allocator]
static ALLOCATOR: BudgetAllocator = BudgetAllocator;

/// Run `call` with at most `n` allocations allowed on this thread.
pub fn with_budget<T>(n: usize, call: impl FnOnce() -> T) -> T {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            LEFT.set(None);
        }
    }
    LEFT.set(Some(n));
    let _reset = Reset;
    call()
}

//! Memory management syscall wrappers

use crate::syscall::{nr, raw};

/// Change the program break (heap end).
///
/// # Arguments
/// * `addr` - New program break address, or 0 to query current break
///
/// # Returns
/// Current program break on success, or unchanged break on error.
///
/// # Example
/// ```rust,ignore
/// use libbreenix::memory::brk;
///
/// // Query current break
/// let current = brk(0);
///
/// // Allocate 4KB
/// let new_break = brk(current + 4096);
/// if new_break == current + 4096 {
///     // Allocation succeeded
/// }
/// ```
#[inline]
pub fn brk(addr: u64) -> u64 {
    unsafe { raw::syscall1(nr::BRK, addr) }
}

/// Get the current program break.
#[inline]
pub fn get_brk() -> u64 {
    brk(0)
}

/// Allocate memory by extending the program break.
///
/// This is a simple bump allocator - it extends the heap by `size` bytes
/// and returns a pointer to the start of the new allocation.
///
/// # Arguments
/// * `size` - Number of bytes to allocate
///
/// # Returns
/// Pointer to allocated memory, or null on failure.
///
/// # Safety
/// The returned memory is uninitialized.
#[inline]
pub fn sbrk(size: usize) -> *mut u8 {
    let current = get_brk();
    let new_break = brk(current + size as u64);
    if new_break == current + size as u64 {
        current as *mut u8
    } else {
        core::ptr::null_mut()
    }
}

// Future syscalls (not yet implemented in kernel):
// pub fn mmap(...) -> *mut u8
// pub fn munmap(...) -> i64
// pub fn mprotect(...) -> i64

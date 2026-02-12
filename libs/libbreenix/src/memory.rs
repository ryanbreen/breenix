//! Memory management syscall wrappers
//!
//! This module provides both POSIX-named syscall wrappers (brk, mmap, munmap, mprotect) and
//! Rust convenience functions (sbrk, get_brk). Both layers coexist for flexibility.

use crate::error::Error;
use crate::syscall::{nr, raw};

/// Change the program break (heap end).
///
/// # Arguments
/// * `addr` - New program break address, or 0 to query current break
///
/// # Returns
/// Current program break on success, or unchanged break on error.
///
/// Note: brk on Linux returns the new break on success, or the current (unchanged)
/// break on error. It does not use the standard negative-errno convention, so we
/// keep the raw u64 return here.
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
    // Check that the new break is >= requested (kernel may page-align up)
    // On failure, kernel returns the old break which will be < current + size
    if new_break >= current + size as u64 {
        current as *mut u8
    } else {
        core::ptr::null_mut()
    }
}

// mmap/munmap syscalls (Phase 9)

/// Protection flags for mmap
pub const PROT_NONE: i32 = 0;
pub const PROT_READ: i32 = 1;
pub const PROT_WRITE: i32 = 2;
pub const PROT_EXEC: i32 = 4;

/// Map flags for mmap
pub const MAP_SHARED: i32 = 0x01;
pub const MAP_PRIVATE: i32 = 0x02;
pub const MAP_FIXED: i32 = 0x10;
pub const MAP_ANONYMOUS: i32 = 0x20;

/// Error return value for mmap (kept for backward compatibility)
pub const MAP_FAILED: *mut u8 = usize::MAX as *mut u8;

/// Map memory into the process address space.
///
/// # Arguments
/// * `addr` - Hint address (null for kernel to choose)
/// * `length` - Size of mapping
/// * `prot` - Protection flags (PROT_READ, PROT_WRITE, PROT_EXEC)
/// * `flags` - Mapping flags (MAP_PRIVATE, MAP_ANONYMOUS, etc.)
/// * `fd` - File descriptor (-1 for anonymous)
/// * `offset` - File offset
///
/// # Returns
/// `Ok(pointer)` to the mapped region on success, `Err(Error)` on failure.
///
/// # Example
/// ```rust,ignore
/// use libbreenix::memory::{mmap, PROT_READ, PROT_WRITE, MAP_PRIVATE, MAP_ANONYMOUS};
/// use core::ptr::null_mut;
///
/// let ptr = mmap(
///     null_mut(),
///     4096,
///     PROT_READ | PROT_WRITE,
///     MAP_PRIVATE | MAP_ANONYMOUS,
///     -1,
///     0,
/// ).unwrap();
/// ```
#[inline]
pub fn mmap(
    addr: *mut u8,
    length: usize,
    prot: i32,
    flags: i32,
    fd: i32,
    offset: i64,
) -> Result<*mut u8, Error> {
    let result = unsafe {
        raw::syscall6(
            nr::MMAP,
            addr as u64,
            length as u64,
            prot as u64,
            flags as u64,
            fd as u64,
            offset as u64,
        )
    };

    Error::from_syscall(result as i64).map(|v| v as *mut u8)
}

/// Unmap memory from the process address space.
///
/// # Arguments
/// * `addr` - Address of mapping to unmap
/// * `length` - Size of mapping
///
/// # Returns
/// `Ok(())` on success, `Err(Error)` on failure.
///
/// # Example
/// ```rust,ignore
/// use libbreenix::memory::munmap;
///
/// munmap(ptr, 4096).unwrap();
/// ```
#[inline]
pub fn munmap(addr: *mut u8, length: usize) -> Result<(), Error> {
    let result = unsafe {
        raw::syscall2(nr::MUNMAP, addr as u64, length as u64)
    };
    Error::from_syscall(result as i64).map(|_| ())
}

/// Change protection of a memory region.
///
/// # Arguments
/// * `addr` - Start address (must be page-aligned)
/// * `length` - Size of region
/// * `prot` - New protection flags (PROT_READ, PROT_WRITE, PROT_EXEC)
///
/// # Returns
/// `Ok(())` on success, `Err(Error)` on failure.
///
/// # Example
/// ```rust,ignore
/// use libbreenix::memory::{mmap, mprotect, PROT_READ, PROT_WRITE, MAP_PRIVATE, MAP_ANONYMOUS};
/// use core::ptr::null_mut;
///
/// // Map read-write
/// let ptr = mmap(null_mut(), 4096, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0).unwrap();
///
/// // Change to read-only
/// mprotect(ptr, 4096, PROT_READ).unwrap();
/// ```
#[inline]
pub fn mprotect(addr: *mut u8, length: usize, prot: i32) -> Result<(), Error> {
    let result = unsafe {
        raw::syscall3(nr::MPROTECT, addr as u64, length as u64, prot as u64)
    };
    Error::from_syscall(result as i64).map(|_| ())
}

/// Copy-on-Write statistics
///
/// This structure is returned by the cow_stats() syscall and contains
/// counters for various CoW events in the kernel.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct CowStats {
    /// Total number of CoW page faults handled
    pub total_faults: u64,
    /// Faults handled via process manager (normal path)
    pub manager_path: u64,
    /// Faults handled via direct page table manipulation (lock-held path)
    pub direct_path: u64,
    /// Pages that were actually copied (frame was shared)
    pub pages_copied: u64,
    /// Pages made writable without copy (sole owner optimization)
    pub sole_owner_opt: u64,
}

/// Get Copy-on-Write statistics from the kernel.
///
/// This is a testing/debugging syscall that returns the current CoW counters.
/// Use this to verify that the sole-owner optimization and page copying are
/// working as expected.
///
/// # Returns
/// `Ok(CowStats)` on success, `Err(Error)` on error.
///
/// # Example
/// ```rust,ignore
/// use libbreenix::memory::cow_stats;
///
/// let stats = cow_stats().unwrap();
/// // stats.total_faults, stats.sole_owner_opt, etc.
/// ```
#[inline]
pub fn cow_stats() -> Result<CowStats, Error> {
    let mut stats = CowStats::default();
    let result = unsafe {
        raw::syscall1(nr::COW_STATS, &mut stats as *mut CowStats as u64)
    };
    Error::from_syscall(result as i64).map(|_| stats)
}

/// Enable or disable OOM simulation for testing.
///
/// When OOM simulation is enabled, all frame allocations will fail, causing
/// Copy-on-Write page faults to fail. Processes that trigger a CoW fault
/// during OOM simulation will be terminated with SIGSEGV (exit code -11).
///
/// This is used to test that the kernel gracefully handles memory exhaustion
/// during CoW page faults.
///
/// # Arguments
/// * `enable` - true to enable OOM simulation, false to disable
///
/// # Returns
/// `Ok(())` on success, `Err(Error)` if the testing feature is not compiled into the kernel.
///
/// # Safety
/// Only enable OOM simulation briefly for testing! Extended OOM simulation will
/// crash the kernel because it affects ALL frame allocations.
#[inline]
pub fn simulate_oom(enable: bool) -> Result<(), Error> {
    let result = unsafe {
        raw::syscall1(nr::SIMULATE_OOM, if enable { 1 } else { 0 })
    };
    Error::from_syscall(result as i64).map(|_| ())
}

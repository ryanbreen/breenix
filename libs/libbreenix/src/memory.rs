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

/// Error return value for mmap
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
/// Pointer to mapped region, or MAP_FAILED on error
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
/// );
/// ```
#[inline]
pub fn mmap(
    addr: *mut u8,
    length: usize,
    prot: i32,
    flags: i32,
    fd: i32,
    offset: i64,
) -> *mut u8 {
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

    // Check for error (negative values)
    if (result as i64) < 0 {
        MAP_FAILED
    } else {
        result as *mut u8
    }
}

/// Unmap memory from the process address space.
///
/// # Arguments
/// * `addr` - Address of mapping to unmap
/// * `length` - Size of mapping
///
/// # Returns
/// 0 on success, -1 on error
///
/// # Example
/// ```rust,ignore
/// use libbreenix::memory::munmap;
///
/// let result = munmap(ptr, 4096);
/// if result == 0 {
///     // Success
/// }
/// ```
#[inline]
pub fn munmap(addr: *mut u8, length: usize) -> i32 {
    let result = unsafe {
        raw::syscall2(nr::MUNMAP, addr as u64, length as u64)
    };
    result as i32
}

/// Change protection of a memory region.
///
/// # Arguments
/// * `addr` - Start address (must be page-aligned)
/// * `length` - Size of region
/// * `prot` - New protection flags (PROT_READ, PROT_WRITE, PROT_EXEC)
///
/// # Returns
/// 0 on success, -1 on error
///
/// # Example
/// ```rust,ignore
/// use libbreenix::memory::{mmap, mprotect, PROT_READ, PROT_WRITE, MAP_PRIVATE, MAP_ANONYMOUS};
/// use core::ptr::null_mut;
///
/// // Map read-write
/// let ptr = mmap(null_mut(), 4096, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
///
/// // Change to read-only
/// let result = mprotect(ptr, 4096, PROT_READ);
/// if result == 0 {
///     // Success - memory is now read-only
/// }
/// ```
#[inline]
pub fn mprotect(addr: *mut u8, length: usize, prot: i32) -> i32 {
    let result = unsafe {
        raw::syscall3(nr::MPROTECT, addr as u64, length as u64, prot as u64)
    };
    result as i32
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
/// Some(CowStats) on success, None on error.
///
/// # Example
/// ```rust,ignore
/// use libbreenix::memory::cow_stats;
///
/// if let Some(stats) = cow_stats() {
///     println!("Total CoW faults: {}", stats.total_faults);
///     println!("Sole owner optimizations: {}", stats.sole_owner_opt);
/// }
/// ```
#[inline]
pub fn cow_stats() -> Option<CowStats> {
    let mut stats = CowStats::default();
    let result = unsafe {
        raw::syscall1(nr::COW_STATS, &mut stats as *mut CowStats as u64)
    };
    if (result as i64) < 0 {
        None
    } else {
        Some(stats)
    }
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
/// 0 on success, -ENOSYS if the testing feature is not compiled into the kernel.
///
/// # Safety
/// Only enable OOM simulation briefly for testing! Extended OOM simulation will
/// crash the kernel because it affects ALL frame allocations.
///
/// # Example
/// ```rust,ignore
/// use libbreenix::memory::simulate_oom;
/// use libbreenix::process::{fork, exit, waitpid, wifexited, wexitstatus};
///
/// // Enable OOM simulation
/// simulate_oom(true);
///
/// let pid = fork();
/// if pid == 0 {
///     // Child process
///     // This write will trigger a CoW fault that fails due to OOM
///     // The child will be killed with SIGSEGV
///     let ptr = some_shared_memory;
///     *ptr = 42; // Triggers CoW fault -> SIGSEGV
///     // Never reaches here
/// } else {
///     // Parent process
///     simulate_oom(false); // Disable OOM simulation
///     let mut status = 0;
///     waitpid(pid, &mut status, 0);
///     // Child should have been killed with SIGSEGV (exit code -11)
///     assert!(!wifexited(status)); // Killed by signal, not normal exit
/// }
/// ```
#[inline]
pub fn simulate_oom(enable: bool) -> i32 {
    let result = unsafe {
        raw::syscall1(nr::SIMULATE_OOM, if enable { 1 } else { 0 })
    };
    result as i32
}

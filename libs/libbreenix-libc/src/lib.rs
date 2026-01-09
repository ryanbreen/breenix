//! libbreenix-libc: C ABI wrappers for Breenix syscalls
//!
//! This crate provides C-compatible function signatures that wrap the existing
//! libbreenix syscall library. This enables:
//!
//! 1. Rust std support via `-Z build-std` (std expects libc functions)
//! 2. Future C program support
//!
//! # Architecture
//!
//! ```text
//! Rust std / C programs
//!         |
//!         v
//! libbreenix-libc (this crate) - C ABI wrappers
//!         |
//!         v
//! libbreenix - Rust syscall wrappers
//!         |
//!         v
//! Breenix kernel (int 0x80 syscalls)
//! ```
//!
//! # Phase 1 Functions (Minimal std support)
//!
//! - I/O: write, read, close
//! - Process: exit, _exit, getpid
//! - Memory: mmap, munmap
//! - Error: __errno_location

#![no_std]

use core::slice;

// =============================================================================
// Panic Handler
// =============================================================================

/// Panic handler for no_std environment
///
/// Since this is a libc implementation, we just loop forever on panic.
/// In the future, this could call abort() to terminate the process.
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    // In a real libc, we'd call abort() here
    // For now, just loop forever
    loop {
        core::hint::spin_loop();
    }
}

// =============================================================================
// Error Handling
// =============================================================================

/// Thread-local errno storage
///
/// Note: This is a simple static for now. Thread-local storage (TLS) will be
/// implemented in Phase 4 when we add threading support.
static mut ERRNO: i32 = 0;

/// Returns a pointer to the thread-local errno variable.
///
/// This is the standard libc interface for accessing errno. Rust std and
/// other code that uses errno will call this function.
#[no_mangle]
pub extern "C" fn __errno_location() -> *mut i32 {
    // Return a mutable pointer to the static errno.
    // This is the standard libc interface - errno is designed to be accessed this way.
    // Note: Single-threaded for now (Phase 4 will add proper TLS)
    core::ptr::addr_of_mut!(ERRNO)
}

/// Set errno from a negative syscall return value
///
/// Syscalls return negative errno on error. This helper converts the return
/// value to the positive errno and stores it.
#[inline]
fn set_errno_from_result(result: i64) -> i32 {
    if result < 0 {
        let errno_val = (-result) as i32;
        unsafe {
            ERRNO = errno_val;
        }
        errno_val
    } else {
        0
    }
}

// =============================================================================
// I/O Functions
// =============================================================================

/// Write bytes to a file descriptor.
///
/// # Arguments
/// * `fd` - File descriptor to write to
/// * `buf` - Buffer containing data to write
/// * `count` - Number of bytes to write
///
/// # Returns
/// Number of bytes written on success, -1 on error (sets errno).
///
/// # Safety
/// Caller must ensure `buf` points to at least `count` valid bytes.
#[no_mangle]
pub unsafe extern "C" fn write(fd: i32, buf: *const u8, count: usize) -> isize {
    if buf.is_null() && count > 0 {
        ERRNO = libbreenix::Errno::EFAULT as i32;
        return -1;
    }

    // Convert fd from C's i32 to libbreenix's Fd (u64)
    let fd_u64 = fd as u64;

    // Create a slice from the raw pointer
    let slice = if count == 0 {
        &[]
    } else {
        slice::from_raw_parts(buf, count)
    };

    let result = libbreenix::io::write(fd_u64, slice);

    if result < 0 {
        set_errno_from_result(result);
        -1
    } else {
        result as isize
    }
}

/// Read bytes from a file descriptor.
///
/// # Arguments
/// * `fd` - File descriptor to read from
/// * `buf` - Buffer to read data into
/// * `count` - Maximum number of bytes to read
///
/// # Returns
/// Number of bytes read on success, -1 on error (sets errno).
///
/// # Safety
/// Caller must ensure `buf` points to at least `count` bytes of writable memory.
#[no_mangle]
pub unsafe extern "C" fn read(fd: i32, buf: *mut u8, count: usize) -> isize {
    if buf.is_null() && count > 0 {
        ERRNO = libbreenix::Errno::EFAULT as i32;
        return -1;
    }

    // Convert fd from C's i32 to libbreenix's Fd (u64)
    let fd_u64 = fd as u64;

    // Create a mutable slice from the raw pointer
    let slice = if count == 0 {
        &mut []
    } else {
        slice::from_raw_parts_mut(buf, count)
    };

    let result = libbreenix::io::read(fd_u64, slice);

    if result < 0 {
        set_errno_from_result(result);
        -1
    } else {
        result as isize
    }
}

/// Close a file descriptor.
///
/// # Arguments
/// * `fd` - File descriptor to close
///
/// # Returns
/// 0 on success, -1 on error (sets errno).
#[no_mangle]
pub extern "C" fn close(fd: i32) -> i32 {
    // Convert fd from C's i32 to libbreenix's Fd (u64)
    let fd_u64 = fd as u64;

    let result = libbreenix::io::close(fd_u64);

    if result < 0 {
        set_errno_from_result(result);
        -1
    } else {
        0
    }
}

/// Duplicate a file descriptor.
///
/// # Arguments
/// * `oldfd` - File descriptor to duplicate
///
/// # Returns
/// New file descriptor on success, -1 on error (sets errno).
#[no_mangle]
pub extern "C" fn dup(oldfd: i32) -> i32 {
    let fd_u64 = oldfd as u64;
    let result = libbreenix::io::dup(fd_u64);

    if result < 0 {
        set_errno_from_result(result);
        -1
    } else {
        result as i32
    }
}

/// Create a pipe.
///
/// # Arguments
/// * `pipefd` - Array of two ints to receive the file descriptors
///              pipefd[0] = read end, pipefd[1] = write end
///
/// # Returns
/// 0 on success, -1 on error (sets errno).
///
/// # Safety
/// Caller must ensure pipefd points to an array of at least 2 ints.
#[no_mangle]
pub unsafe extern "C" fn pipe(pipefd: *mut i32) -> i32 {
    if pipefd.is_null() {
        ERRNO = EFAULT;
        return -1;
    }

    let mut fds: [i32; 2] = [0, 0];
    let result = libbreenix::io::pipe(&mut fds);

    if result < 0 {
        set_errno_from_result(result);
        -1
    } else {
        *pipefd = fds[0];
        *pipefd.add(1) = fds[1];
        0
    }
}

// =============================================================================
// Process Control
// =============================================================================

/// Terminate the calling process.
///
/// # Arguments
/// * `status` - Exit status code
///
/// # Returns
/// This function never returns.
#[no_mangle]
pub extern "C" fn exit(status: i32) -> ! {
    libbreenix::process::exit(status)
}

/// Terminate the calling process immediately.
///
/// This is the same as exit() but is the "raw" syscall version that
/// doesn't run atexit handlers (we don't have those yet anyway).
///
/// # Arguments
/// * `status` - Exit status code
///
/// # Returns
/// This function never returns.
#[no_mangle]
pub extern "C" fn _exit(status: i32) -> ! {
    libbreenix::process::exit(status)
}

/// Get the process ID of the calling process.
///
/// # Returns
/// The process ID (always succeeds).
#[no_mangle]
pub extern "C" fn getpid() -> i32 {
    libbreenix::process::getpid() as i32
}

/// Get the thread ID of the calling thread.
///
/// # Returns
/// The thread ID (always succeeds).
#[no_mangle]
pub extern "C" fn gettid() -> i32 {
    libbreenix::process::gettid() as i32
}

// =============================================================================
// Memory Management
// =============================================================================

/// Map memory into the process address space.
///
/// # Arguments
/// * `addr` - Hint address (NULL for kernel to choose)
/// * `len` - Size of mapping
/// * `prot` - Protection flags (PROT_READ, PROT_WRITE, PROT_EXEC)
/// * `flags` - Mapping flags (MAP_PRIVATE, MAP_ANONYMOUS, etc.)
/// * `fd` - File descriptor (-1 for anonymous)
/// * `offset` - File offset
///
/// # Returns
/// Pointer to mapped region on success, MAP_FAILED (-1 as pointer) on error.
///
/// # Safety
/// This function is inherently unsafe as it manipulates virtual memory.
#[no_mangle]
pub unsafe extern "C" fn mmap(
    addr: *mut u8,
    len: usize,
    prot: i32,
    flags: i32,
    fd: i32,
    offset: i64,
) -> *mut u8 {
    // Call raw syscall directly to get the actual error code
    let result = libbreenix::raw::syscall6(
        9, // MMAP syscall number
        addr as u64,
        len as u64,
        prot as u64,
        flags as u64,
        fd as u64,
        offset as u64,
    );

    // Check for error (negative values indicate -errno)
    let result_signed = result as i64;
    if result_signed < 0 && result_signed >= -4096 {
        // Error: result is -errno, convert to positive errno
        ERRNO = (-result_signed) as i32;
        libbreenix::memory::MAP_FAILED
    } else {
        result as *mut u8
    }
}

/// Unmap memory from the process address space.
///
/// # Arguments
/// * `addr` - Address of mapping to unmap
/// * `len` - Size of mapping
///
/// # Returns
/// 0 on success, -1 on error (sets errno).
///
/// # Safety
/// Caller must ensure the memory region is valid to unmap.
#[no_mangle]
pub unsafe extern "C" fn munmap(addr: *mut u8, len: usize) -> i32 {
    let result = libbreenix::memory::munmap(addr, len);

    if result < 0 {
        set_errno_from_result(result as i64);
        -1
    } else {
        0
    }
}

/// Change protection on a region of memory.
///
/// # Arguments
/// * `addr` - Start address (must be page-aligned)
/// * `len` - Size of region
/// * `prot` - New protection flags (PROT_READ, PROT_WRITE, PROT_EXEC)
///
/// # Returns
/// 0 on success, -1 on error (sets errno).
///
/// # Safety
/// Caller must ensure the memory region is valid.
#[no_mangle]
pub unsafe extern "C" fn mprotect(addr: *mut u8, len: usize, prot: i32) -> i32 {
    let result = libbreenix::memory::mprotect(addr, len, prot);

    if result < 0 {
        set_errno_from_result(result as i64);
        -1
    } else {
        0
    }
}

/// Change the program break (heap end).
///
/// # Arguments
/// * `addr` - New program break address, or NULL/0 to query current break
///
/// # Returns
/// New program break on success, -1 cast to pointer on error.
///
/// # Safety
/// This function manipulates the heap boundary.
#[no_mangle]
pub unsafe extern "C" fn brk(addr: *mut u8) -> i32 {
    let result = libbreenix::memory::brk(addr as u64);

    // brk returns the new break on success, or the old break on failure
    // In C, brk returns 0 on success, -1 on error
    // But this is actually sbrk-like behavior, so we return success always
    // since libbreenix::memory::brk always returns the current break
    if result == 0 && !addr.is_null() {
        // Request failed (break didn't change to requested value)
        ERRNO = libbreenix::Errno::ENOMEM as i32;
        -1
    } else {
        0
    }
}

/// Allocate memory by moving the program break.
///
/// # Arguments
/// * `increment` - Number of bytes to add to the program break.
///
/// # Returns
/// Previous program break on success, -1 cast to pointer on error.
///
/// # Notes
/// - If `increment` is 0, returns the current program break.
/// - Negative increments are NOT currently supported (returns error with EINVAL).
///   This limitation exists because the underlying libbreenix::memory::sbrk
///   only supports heap expansion, not shrinking. Most allocators use mmap/munmap
///   for memory management anyway, so this is typically not an issue.
///
/// # Safety
/// This function manipulates the heap boundary.
#[no_mangle]
pub unsafe extern "C" fn sbrk(increment: isize) -> *mut u8 {
    if increment == 0 {
        // Query current break
        return libbreenix::memory::get_brk() as *mut u8;
    }

    if increment < 0 {
        // Negative increments (heap shrinking) are not supported.
        // The underlying libbreenix::memory::sbrk only handles positive increments.
        // Return error with EINVAL to indicate invalid argument.
        ERRNO = libbreenix::Errno::EINVAL as i32;
        return usize::MAX as *mut u8; // Return -1 as pointer (MAP_FAILED equivalent)
    }

    // Safe cast: we've verified increment >= 0 above
    let result = libbreenix::memory::sbrk(increment as usize);

    if result.is_null() {
        ERRNO = libbreenix::Errno::ENOMEM as i32;
        usize::MAX as *mut u8 // Return -1 as pointer
    } else {
        result
    }
}

// =============================================================================
// Memory Constants (re-exported for convenience)
// =============================================================================

/// Protection: page cannot be accessed
pub const PROT_NONE: i32 = libbreenix::memory::PROT_NONE;
/// Protection: page can be read
pub const PROT_READ: i32 = libbreenix::memory::PROT_READ;
/// Protection: page can be written
pub const PROT_WRITE: i32 = libbreenix::memory::PROT_WRITE;
/// Protection: page can be executed
pub const PROT_EXEC: i32 = libbreenix::memory::PROT_EXEC;

/// Mapping: share changes
pub const MAP_SHARED: i32 = libbreenix::memory::MAP_SHARED;
/// Mapping: changes are private
pub const MAP_PRIVATE: i32 = libbreenix::memory::MAP_PRIVATE;
/// Mapping: place at exact address
pub const MAP_FIXED: i32 = libbreenix::memory::MAP_FIXED;
/// Mapping: not backed by file
pub const MAP_ANONYMOUS: i32 = libbreenix::memory::MAP_ANONYMOUS;

/// Error return value for mmap
pub const MAP_FAILED: *mut u8 = usize::MAX as *mut u8;

// =============================================================================
// Errno Constants (re-exported for C compatibility)
// =============================================================================

// These match Linux errno values
pub const EPERM: i32 = 1;
pub const ENOENT: i32 = 2;
pub const ESRCH: i32 = 3;
pub const EINTR: i32 = 4;
pub const EIO: i32 = 5;
pub const ENXIO: i32 = 6;
pub const E2BIG: i32 = 7;
pub const ENOEXEC: i32 = 8;
pub const EBADF: i32 = 9;
pub const ECHILD: i32 = 10;
pub const EAGAIN: i32 = 11;
pub const ENOMEM: i32 = 12;
pub const EACCES: i32 = 13;
pub const EFAULT: i32 = 14;
pub const ENOTBLK: i32 = 15;
pub const EBUSY: i32 = 16;
pub const EEXIST: i32 = 17;
pub const EXDEV: i32 = 18;
pub const ENODEV: i32 = 19;
pub const ENOTDIR: i32 = 20;
pub const EISDIR: i32 = 21;
pub const EINVAL: i32 = 22;
pub const ENFILE: i32 = 23;
pub const EMFILE: i32 = 24;
pub const ENOTTY: i32 = 25;
pub const ETXTBSY: i32 = 26;
pub const EFBIG: i32 = 27;
pub const ENOSPC: i32 = 28;
pub const ESPIPE: i32 = 29;
pub const EROFS: i32 = 30;
pub const EMLINK: i32 = 31;
pub const EPIPE: i32 = 32;
pub const ENOSYS: i32 = 38;

// =============================================================================
// C Runtime Startup (_start entry point)
// =============================================================================

/// The _start entry point for Rust programs using std.
///
/// This is the first code that runs when a program starts. On Linux/Unix,
/// the kernel sets up the stack with argc, argv, and envp, then jumps to _start.
///
/// Stack layout at entry:
/// ```text
/// [top of stack]
/// NULL
/// envp[n]
/// ...
/// envp[0]
/// NULL
/// argv[argc-1]
/// ...
/// argv[0]
/// argc
/// [rsp points here]
/// ```
///
/// We need to:
/// 1. Extract argc, argv, envp from the stack
/// 2. Set up any required runtime state
/// 3. Call main (via Rust's lang_start)
///
/// For Rust std programs, we call `main` directly since Rust's lang_start
/// handles the rest. The #[lang = "start"] attribute on main takes care of this.
#[no_mangle]
pub extern "C" fn _start() -> ! {
    extern "C" {
        fn main(argc: isize, argv: *const *const u8) -> isize;
    }

    unsafe {
        // Get argc from stack (first value at rsp when _start is called)
        // The kernel places: [rsp] = argc, [rsp+8] = argv[0], etc.
        //
        // Note: In the actual entry, the stack layout depends on the kernel.
        // For now, we use a simple approach: argc=0, argv=null pointer.
        // A proper implementation would read from the stack set up by the kernel.
        //
        // TODO: Properly extract argc/argv from the stack.
        // For minimal hello world testing, we can proceed with argc=0.
        let argc: isize = 0;
        let argv: *const *const u8 = core::ptr::null();

        let ret = main(argc, argv);
        exit(ret as i32);
    }
}

/// Abort function - required by various runtime components
#[no_mangle]
pub extern "C" fn abort() -> ! {
    // Exit with a failure code
    exit(134) // 128 + SIGABRT (6) = 134
}

/// strlen - required by Rust's CString and other string operations
#[no_mangle]
pub unsafe extern "C" fn strlen(s: *const u8) -> usize {
    let mut len = 0;
    while *s.add(len) != 0 {
        len += 1;
    }
    len
}

/// memcmp - required for various comparisons
#[no_mangle]
pub unsafe extern "C" fn memcmp(s1: *const u8, s2: *const u8, n: usize) -> i32 {
    for i in 0..n {
        let a = *s1.add(i);
        let b = *s2.add(i);
        if a != b {
            return a as i32 - b as i32;
        }
    }
    0
}

/// getenv - get environment variable (stub - always returns NULL)
#[no_mangle]
pub extern "C" fn getenv(_name: *const u8) -> *mut u8 {
    core::ptr::null_mut()
}

/// Get random bytes from the kernel.
///
/// Currently returns ENOSYS as the kernel doesn't have an entropy source.
/// Programs requiring randomness will fail explicitly rather than getting
/// predictable "random" values.
#[no_mangle]
pub unsafe extern "C" fn getrandom(_buf: *mut u8, _buflen: usize, _flags: u32) -> isize {
    ERRNO = ENOSYS;
    -1
}

// =============================================================================
// Memory Allocation Functions
// =============================================================================

/// Header size for allocation tracking.
/// Stores: [size: usize (8 bytes)][padding: 8 bytes for alignment]
/// Total: 16 bytes to maintain 16-byte alignment for returned pointers.
const ALLOC_HEADER_SIZE: usize = 16;

/// Get the size of an allocation from its header.
///
/// # Safety
/// Caller must ensure `ptr` was returned by malloc/realloc and is not null.
#[inline]
unsafe fn get_alloc_size(ptr: *mut u8) -> usize {
    let header = ptr.sub(ALLOC_HEADER_SIZE);
    *(header as *const usize)
}

/// malloc - allocate memory with size tracking
///
/// Allocates `size` bytes and stores the allocation size in a header
/// before the returned pointer. This enables realloc to know how many
/// bytes to copy when growing/shrinking allocations.
#[no_mangle]
pub unsafe extern "C" fn malloc(size: usize) -> *mut u8 {
    if size == 0 {
        return core::ptr::null_mut();
    }

    // Allocate extra space for the header
    let total_size = size + ALLOC_HEADER_SIZE;
    let ptr = mmap(
        core::ptr::null_mut(),
        total_size,
        PROT_READ | PROT_WRITE,
        MAP_PRIVATE | MAP_ANONYMOUS,
        -1,
        0,
    );

    if ptr == MAP_FAILED {
        core::ptr::null_mut()
    } else {
        // Store size in header (offset 0)
        *(ptr as *mut usize) = size;
        // Zero the second field (offset 8) to distinguish from posix_memalign allocations
        // posix_memalign stores base_ptr here, which is non-zero and <= header
        *((ptr as *mut usize).add(1)) = 0;
        // Return pointer after header
        ptr.add(ALLOC_HEADER_SIZE)
    }
}

/// free - deallocate memory
///
/// Frees memory allocated by malloc/realloc/posix_memalign by reading metadata
/// from the header and calling munmap on the full allocation.
///
/// Header layout (16 bytes before user pointer):
/// - For malloc/realloc: [size: usize][unused: usize]
///   - munmap starts at header, size = size + ALLOC_HEADER_SIZE
/// - For posix_memalign with alignment > 16: [size: usize][base_ptr: usize]
///   - munmap starts at base_ptr, size = (ptr - base_ptr) + size
///
/// We detect posix_memalign allocations by checking if base_ptr_field != 0 and
/// points to an address before or at the header. For regular malloc, the second
/// field is unused (typically 0 or garbage, but < header address).
#[no_mangle]
pub unsafe extern "C" fn free(ptr: *mut u8) {
    if ptr.is_null() {
        return;
    }
    let header = ptr.sub(ALLOC_HEADER_SIZE);
    let size = *(header as *const usize);
    let base_ptr_field = *((header as *const usize).add(1));

    // Detect if this is a posix_memalign allocation:
    // - base_ptr_field will be a valid address pointing to the start of the mmap
    // - For regular malloc, base_ptr_field is typically 0 or garbage
    // - For posix_memalign, base_ptr_field points to an address <= header
    //
    // We check if base_ptr_field looks like a valid pointer (non-zero and <= header)
    let header_addr = header as usize;
    if base_ptr_field != 0 && base_ptr_field <= header_addr {
        // posix_memalign allocation: munmap from base_ptr
        // The total size is from base_ptr to ptr + size
        let base_ptr = base_ptr_field as *mut u8;
        let total_size = (ptr as usize - base_ptr_field) + size;
        munmap(base_ptr, total_size);
    } else {
        // Regular malloc/realloc allocation: munmap from header
        let munmap_size = size + ALLOC_HEADER_SIZE;
        munmap(header, munmap_size);
    }
}

/// realloc - resize memory allocation with proper data preservation
///
/// Copies min(old_size, new_size) bytes to avoid reading beyond the
/// original allocation's bounds (which would be undefined behavior).
#[no_mangle]
pub unsafe extern "C" fn realloc(ptr: *mut u8, size: usize) -> *mut u8 {
    if ptr.is_null() {
        return malloc(size);
    }
    if size == 0 {
        free(ptr);
        return core::ptr::null_mut();
    }

    let old_size = get_alloc_size(ptr);
    let new_ptr = malloc(size);

    if !new_ptr.is_null() {
        // Copy min(old_size, new_size) bytes - safe bounds
        let copy_size = core::cmp::min(old_size, size);
        core::ptr::copy_nonoverlapping(ptr, new_ptr, copy_size);
        free(ptr);
    }
    new_ptr
}

/// posix_memalign - allocate aligned memory
///
/// Uses the same header scheme as malloc for consistency with free().
/// For alignments larger than ALLOC_HEADER_SIZE, we allocate extra space
/// and align the returned pointer while ensuring the header is accessible.
#[no_mangle]
pub unsafe extern "C" fn posix_memalign(
    memptr: *mut *mut u8,
    alignment: usize,
    size: usize,
) -> i32 {
    if alignment == 0 || (alignment & (alignment - 1)) != 0 {
        return EINVAL;
    }
    if alignment < core::mem::size_of::<*mut u8>() {
        return EINVAL;
    }

    // For alignments <= ALLOC_HEADER_SIZE (16 bytes), malloc already works
    // since mmap returns page-aligned memory and our header is 16 bytes.
    if alignment <= ALLOC_HEADER_SIZE {
        let ptr = malloc(size);
        if ptr.is_null() {
            return ENOMEM;
        }
        *memptr = ptr;
        return 0;
    }

    // For larger alignments, we need to allocate extra space to ensure
    // we can align the user pointer while keeping the header accessible.
    // We need: header (16 bytes) + padding for alignment + size
    let total_size = ALLOC_HEADER_SIZE + alignment + size;
    let base_ptr = mmap(
        core::ptr::null_mut(),
        total_size,
        PROT_READ | PROT_WRITE,
        MAP_PRIVATE | MAP_ANONYMOUS,
        -1,
        0,
    );

    if base_ptr == MAP_FAILED {
        return ENOMEM;
    }

    // Find an aligned address that leaves room for the header before it
    // Start after the header and find the next aligned address
    let after_header = base_ptr.add(ALLOC_HEADER_SIZE) as usize;
    let aligned_addr = (after_header + alignment - 1) & !(alignment - 1);
    let user_ptr = aligned_addr as *mut u8;

    // Store metadata in the 16 bytes before the user pointer:
    // - At offset 0: size (user-requested size) - for realloc to know how much data to copy
    // - At offset 8: base_ptr as usize - for free to know where the mmap started
    //
    // We need to store base_ptr (not just total_size) because the header may be
    // at a different address than base_ptr due to alignment padding.
    let header = user_ptr.sub(ALLOC_HEADER_SIZE);
    *(header as *mut usize) = size;
    // Store base_ptr for munmap - free will calculate total_size from user_ptr position
    *((header as *mut usize).add(1)) = base_ptr as usize;

    *memptr = user_ptr;
    0
}

// =============================================================================
// Syscall and Synchronization Functions
// =============================================================================

/// pause - suspend until signal (stub implementation)
#[no_mangle]
pub extern "C" fn pause() -> i32 {
    // Simple busy wait - a real implementation would use a syscall
    loop {
        core::hint::spin_loop();
    }
}

/// syscall - generic syscall interface
///
/// This provides a way for std to make syscalls. We implement the common
/// syscalls that std uses directly.
#[no_mangle]
pub unsafe extern "C" fn syscall(num: i64, _a1: i64, _a2: i64, _a3: i64, _a4: i64, _a5: i64, _a6: i64) -> i64 {
    // Linux x86_64 syscall numbers
    const SYS_FUTEX: i64 = 202;
    const SYS_GETRANDOM: i64 = 318;

    match num {
        SYS_FUTEX => {
            // futex(uaddr, op, val, timeout, uaddr2, val3)
            // For basic usage, we just return 0 (success)
            0
        }
        SYS_GETRANDOM => {
            // Return ENOSYS - no entropy source available
            -(ENOSYS as i64)
        }
        _ => {
            // Unknown syscall - return ENOSYS
            -(ENOSYS as i64)
        }
    }
}

/// writev - write multiple buffers
#[no_mangle]
pub unsafe extern "C" fn writev(fd: i32, iov: *const Iovec, iovcnt: i32) -> isize {
    let mut total: isize = 0;
    for i in 0..iovcnt as usize {
        let vec = &*iov.add(i);
        let result = write(fd, vec.iov_base as *const u8, vec.iov_len);
        if result < 0 {
            return result;
        }
        total += result;
    }
    total
}

/// Iovec structure for scatter/gather I/O
#[repr(C)]
pub struct Iovec {
    pub iov_base: *mut u8,
    pub iov_len: usize,
}

// =============================================================================
// Thread-Local Storage Functions (Stubs)
// =============================================================================

/// pthread_key_create - create a thread-local key
#[no_mangle]
pub unsafe extern "C" fn pthread_key_create(
    key: *mut u32,
    _destructor: Option<unsafe extern "C" fn(*mut u8)>,
) -> i32 {
    static mut NEXT_KEY: u32 = 0;
    *key = NEXT_KEY;
    NEXT_KEY += 1;
    0 // Success
}

/// pthread_key_delete - delete a thread-local key
#[no_mangle]
pub extern "C" fn pthread_key_delete(_key: u32) -> i32 {
    0 // Success (no-op for now)
}

/// pthread_getspecific - get thread-local value
#[no_mangle]
pub extern "C" fn pthread_getspecific(_key: u32) -> *mut u8 {
    // For single-threaded, just return null
    core::ptr::null_mut()
}

/// pthread_setspecific - set thread-local value
#[no_mangle]
pub extern "C" fn pthread_setspecific(_key: u32, _value: *const u8) -> i32 {
    0 // Success (no-op for now)
}

/// pthread_self - get current thread ID
#[no_mangle]
pub extern "C" fn pthread_self() -> usize {
    1 // Main thread ID
}

/// pthread_getattr_np - get thread attributes
#[no_mangle]
pub extern "C" fn pthread_getattr_np(_thread: usize, _attr: *mut u8) -> i32 {
    0 // Success
}

/// pthread_attr_init - initialize thread attributes
#[no_mangle]
pub extern "C" fn pthread_attr_init(_attr: *mut u8) -> i32 {
    0 // Success
}

/// pthread_attr_destroy - destroy thread attributes
#[no_mangle]
pub extern "C" fn pthread_attr_destroy(_attr: *mut u8) -> i32 {
    0 // Success
}

/// pthread_attr_getstack - get stack attributes
#[no_mangle]
pub unsafe extern "C" fn pthread_attr_getstack(
    _attr: *const u8,
    stackaddr: *mut *mut u8,
    stacksize: *mut usize,
) -> i32 {
    // Return a default stack info
    *stackaddr = 0x7fff0000_00000000_u64 as *mut u8; // High memory address
    *stacksize = 8 * 1024 * 1024; // 8 MB stack
    0 // Success
}

// =============================================================================
// File and I/O Operations
// =============================================================================

/// poll - wait for events on file descriptors (stub)
#[no_mangle]
pub extern "C" fn poll(_fds: *mut u8, _nfds: usize, _timeout: i32) -> i32 {
    0 // No events
}

/// fcntl - file control (stub)
#[no_mangle]
pub extern "C" fn fcntl(_fd: i32, _cmd: i32, _arg: u64) -> i32 {
    0 // Success
}

/// open - open a file (stub)
#[no_mangle]
pub extern "C" fn open(_path: *const u8, _flags: i32, _mode: u32) -> i32 {
    -(ENOSYS as i32) // Not implemented
}

// =============================================================================
// Signal Handling (Stubs)
// =============================================================================

/// signal - set signal handler (stub)
#[no_mangle]
pub extern "C" fn signal(_signum: i32, _handler: usize) -> usize {
    0 // SIG_DFL
}

/// sigaction - examine and change signal action (stub)
#[no_mangle]
pub unsafe extern "C" fn sigaction(_signum: i32, _act: *const u8, _oldact: *mut u8) -> i32 {
    0 // Success
}

/// sigaltstack - set/get signal stack context (stub)
#[no_mangle]
pub unsafe extern "C" fn sigaltstack(_ss: *const u8, _old_ss: *mut u8) -> i32 {
    0 // Success
}

// =============================================================================
// System Information
// =============================================================================

/// sysconf - get system configuration values
#[no_mangle]
pub extern "C" fn sysconf(name: i32) -> i64 {
    const _SC_PAGESIZE: i32 = 30;
    const _SC_NPROCESSORS_ONLN: i32 = 84;
    const _SC_NPROCESSORS_CONF: i32 = 83;
    const _SC_GETPW_R_SIZE_MAX: i32 = 70;
    const _SC_GETGR_R_SIZE_MAX: i32 = 69;

    match name {
        _SC_PAGESIZE => 4096,
        _SC_NPROCESSORS_ONLN | _SC_NPROCESSORS_CONF => 1,
        _SC_GETPW_R_SIZE_MAX | _SC_GETGR_R_SIZE_MAX => 1024,
        _ => -1, // Unknown
    }
}

/// __xpg_strerror_r - convert error number to string (XPG version)
#[no_mangle]
pub unsafe extern "C" fn __xpg_strerror_r(errnum: i32, buf: *mut u8, buflen: usize) -> i32 {
    let msg: &[u8] = match errnum {
        0 => b"Success\0",
        1 => b"Operation not permitted\0",
        2 => b"No such file or directory\0",
        _ => b"Unknown error\0",
    };
    let copy_len = core::cmp::min(msg.len() - 1, buflen - 1);
    core::ptr::copy_nonoverlapping(msg.as_ptr(), buf, copy_len);
    *buf.add(copy_len) = 0;
    0 // Success
}

/// getauxval - get auxiliary vector value (stub)
#[no_mangle]
pub extern "C" fn getauxval(type_: u64) -> u64 {
    const AT_PAGESZ: u64 = 6;
    const AT_HWCAP: u64 = 16;
    const AT_HWCAP2: u64 = 26;

    match type_ {
        AT_PAGESZ => 4096,
        AT_HWCAP | AT_HWCAP2 => 0, // No special hardware capabilities
        _ => 0,
    }
}

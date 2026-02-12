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

#![no_std]

use libbreenix::types::Fd;
use libbreenix::error::Error;
use core::slice;

// =============================================================================
// Panic Handler
// =============================================================================

/// Panic handler for no_std environment
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
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
#[no_mangle]
pub static mut ERRNO: i32 = 0;

/// Returns a pointer to the thread-local errno variable.
#[no_mangle]
pub extern "C" fn __errno_location() -> *mut i32 {
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

/// Helper: convert a negative-errno result to C convention (-1 on error, sets errno)
/// Returns the result as-is if non-negative, or -1 with errno set if negative.
#[inline]
fn syscall_result_to_c_int(result: i64) -> i32 {
    if result < 0 {
        set_errno_from_result(result);
        -1
    } else {
        result as i32
    }
}

/// Helper: convert a negative-errno result to C convention for ssize_t returns
#[inline]
fn syscall_result_to_c_ssize(result: i64) -> isize {
    if result < 0 {
        set_errno_from_result(result);
        -1
    } else {
        result as isize
    }
}

/// Extract raw errno value from an Error
#[inline]
fn error_to_errno(e: &Error) -> i32 {
    match e {
        Error::Os(errno) => *errno as i32,
    }
}

/// Set errno from an Error and return -1
#[inline]
fn set_errno_from_error(e: Error) -> i32 {
    unsafe { ERRNO = error_to_errno(&e); }
    -1
}

/// Convert Result<usize, Error> to C ssize_t convention
#[inline]
fn result_usize_to_c_ssize(result: Result<usize, Error>) -> isize {
    match result {
        Ok(v) => v as isize,
        Err(e) => {
            set_errno_from_error(e);
            -1
        }
    }
}

/// Convert Result<(), Error> to C int convention (0 success, -1 error)
#[inline]
fn result_unit_to_c_int(result: Result<(), Error>) -> i32 {
    match result {
        Ok(()) => 0,
        Err(e) => set_errno_from_error(e),
    }
}

/// Convert Result<Fd, Error> to C int convention (fd on success, -1 error)
#[inline]
fn result_fd_to_c_int(result: Result<Fd, Error>) -> i32 {
    match result {
        Ok(fd) => fd.raw() as i32,
        Err(e) => set_errno_from_error(e),
    }
}

/// Convert Result<i64, Error> to C int convention
#[inline]
fn result_i64_to_c_int(result: Result<i64, Error>) -> i32 {
    match result {
        Ok(v) => v as i32,
        Err(e) => set_errno_from_error(e),
    }
}

// =============================================================================
// I/O Functions
// =============================================================================

/// Write bytes to a file descriptor.
#[no_mangle]
pub unsafe extern "C" fn write(fd: i32, buf: *const u8, count: usize) -> isize {
    if buf.is_null() && count > 0 {
        ERRNO = libbreenix::Errno::EFAULT as i32;
        return -1;
    }

    let fd_val = Fd::from_raw(fd as u64);
    let slice = if count == 0 {
        &[]
    } else {
        slice::from_raw_parts(buf, count)
    };

    let result = libbreenix::io::write(fd_val, slice);
    result_usize_to_c_ssize(result)
}

/// Read bytes from a file descriptor.
#[no_mangle]
pub unsafe extern "C" fn read(fd: i32, buf: *mut u8, count: usize) -> isize {
    if buf.is_null() && count > 0 {
        ERRNO = libbreenix::Errno::EFAULT as i32;
        return -1;
    }

    let fd_val = Fd::from_raw(fd as u64);
    let slice = if count == 0 {
        &mut []
    } else {
        slice::from_raw_parts_mut(buf, count)
    };

    let result = libbreenix::io::read(fd_val, slice);
    result_usize_to_c_ssize(result)
}

/// Close a file descriptor.
#[no_mangle]
pub extern "C" fn close(fd: i32) -> i32 {
    let fd_val = Fd::from_raw(fd as u64);
    let result = libbreenix::io::close(fd_val);
    result_unit_to_c_int(result)
}

/// Duplicate a file descriptor.
#[no_mangle]
pub extern "C" fn dup(oldfd: i32) -> i32 {
    let fd_val = Fd::from_raw(oldfd as u64);
    let result = libbreenix::io::dup(fd_val);
    result_fd_to_c_int(result)
}

/// Duplicate a file descriptor to a specific number.
#[no_mangle]
pub extern "C" fn dup2(oldfd: i32, newfd: i32) -> i32 {
    let result = libbreenix::io::dup2(Fd::from_raw(oldfd as u64), Fd::from_raw(newfd as u64));
    result_fd_to_c_int(result)
}

/// Create a pipe.
#[no_mangle]
pub unsafe extern "C" fn pipe(pipefd: *mut i32) -> i32 {
    if pipefd.is_null() {
        ERRNO = EFAULT;
        return -1;
    }

    match libbreenix::io::pipe() {
        Ok((fd0, fd1)) => {
            *pipefd = fd0.raw() as i32;
            *pipefd.add(1) = fd1.raw() as i32;
            0
        }
        Err(e) => set_errno_from_error(e),
    }
}

/// Create a pipe with flags.
#[no_mangle]
pub unsafe extern "C" fn pipe2(pipefd: *mut i32, flags: i32) -> i32 {
    if pipefd.is_null() {
        ERRNO = EFAULT;
        return -1;
    }

    match libbreenix::io::pipe2(flags) {
        Ok((fd0, fd1)) => {
            *pipefd = fd0.raw() as i32;
            *pipefd.add(1) = fd1.raw() as i32;
            0
        }
        Err(e) => set_errno_from_error(e),
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
// File I/O Functions
// =============================================================================

/// open - open a file
#[no_mangle]
pub unsafe extern "C" fn open(path: *const u8, flags: i32, mode: u32) -> i32 {
    if path.is_null() {
        ERRNO = EFAULT;
        return -1;
    }

    let result = libbreenix::raw::syscall3(
        libbreenix::syscall::nr::OPEN,
        path as u64,
        flags as u64,
        mode as u64,
    ) as i64;
    syscall_result_to_c_int(result)
}

/// openat - open a file relative to a directory fd
///
/// If dirfd is AT_FDCWD (-100), delegates to open() with the given path.
/// Otherwise, returns -ENOSYS (not yet supported).
#[no_mangle]
pub unsafe extern "C" fn openat(dirfd: i32, path: *const u8, flags: i32, mode: u32) -> i32 {
    if path.is_null() {
        ERRNO = EFAULT;
        return -1;
    }

    // AT_FDCWD = -100: use current working directory (same as open)
    if dirfd == -100 {
        return open(path, flags, mode);
    }

    // Non-AT_FDCWD dirfd not supported yet
    ERRNO = ENOSYS;
    -1
}

/// fstat - get file status by fd
#[no_mangle]
pub unsafe extern "C" fn fstat(fd: i32, buf: *mut u8) -> i32 {
    if buf.is_null() {
        ERRNO = EFAULT;
        return -1;
    }

    let result = libbreenix::raw::syscall2(
        libbreenix::syscall::nr::FSTAT,
        fd as u64,
        buf as u64,
    ) as i64;
    syscall_result_to_c_int(result)
}

/// stat - get file status by path
///
/// Implemented as open() + fstat() + close().
#[no_mangle]
pub unsafe extern "C" fn stat(path: *const u8, buf: *mut u8) -> i32 {
    if path.is_null() || buf.is_null() {
        ERRNO = EFAULT;
        return -1;
    }

    let fd = open(path, 0 /* O_RDONLY */, 0);
    if fd < 0 {
        return -1; // errno already set by open
    }

    let result = fstat(fd, buf);
    close(fd);
    result
}

/// lstat - get file status by path (no symlink follow)
///
/// Same as stat since we don't have symlink resolution yet.
#[no_mangle]
pub unsafe extern "C" fn lstat(path: *const u8, buf: *mut u8) -> i32 {
    stat(path, buf)
}

/// fstat64 - same as fstat (64-bit is native on x86_64)
#[no_mangle]
pub unsafe extern "C" fn fstat64(fd: i32, buf: *mut u8) -> i32 {
    fstat(fd, buf)
}

/// stat64 - same as stat (64-bit is native on x86_64)
#[no_mangle]
pub unsafe extern "C" fn stat64(path: *const u8, buf: *mut u8) -> i32 {
    stat(path, buf)
}

/// lstat64 - same as lstat (64-bit is native on x86_64)
#[no_mangle]
pub unsafe extern "C" fn lstat64(path: *const u8, buf: *mut u8) -> i32 {
    lstat(path, buf)
}

/// lseek - reposition read/write file offset
#[no_mangle]
pub unsafe extern "C" fn lseek(fd: i32, offset: i64, whence: i32) -> i64 {
    let result = libbreenix::raw::syscall3(
        libbreenix::syscall::nr::LSEEK,
        fd as u64,
        offset as u64,
        whence as u64,
    ) as i64;

    if result < 0 {
        set_errno_from_result(result);
        -1
    } else {
        result
    }
}

/// lseek64 - same as lseek (64-bit is native on x86_64)
#[no_mangle]
pub unsafe extern "C" fn lseek64(fd: i32, offset: i64, whence: i32) -> i64 {
    lseek(fd, offset, whence)
}

/// readlink - read the target of a symbolic link
#[no_mangle]
pub unsafe extern "C" fn readlink(path: *const u8, buf: *mut u8, bufsiz: usize) -> isize {
    if path.is_null() || buf.is_null() {
        ERRNO = EFAULT;
        return -1;
    }

    let result = libbreenix::raw::syscall3(
        libbreenix::syscall::nr::READLINK,
        path as u64,
        buf as u64,
        bufsiz as u64,
    ) as i64;
    syscall_result_to_c_ssize(result)
}

/// unlink - remove a file
#[no_mangle]
pub unsafe extern "C" fn unlink(path: *const u8) -> i32 {
    if path.is_null() {
        ERRNO = EFAULT;
        return -1;
    }

    let result = libbreenix::raw::syscall1(
        libbreenix::syscall::nr::UNLINK,
        path as u64,
    ) as i64;
    syscall_result_to_c_int(result)
}

/// rename - rename a file
#[no_mangle]
pub unsafe extern "C" fn rename(oldpath: *const u8, newpath: *const u8) -> i32 {
    if oldpath.is_null() || newpath.is_null() {
        ERRNO = EFAULT;
        return -1;
    }

    let result = libbreenix::raw::syscall2(
        libbreenix::syscall::nr::RENAME,
        oldpath as u64,
        newpath as u64,
    ) as i64;
    syscall_result_to_c_int(result)
}

/// mkdir - create a directory
#[no_mangle]
pub unsafe extern "C" fn mkdir(path: *const u8, mode: u32) -> i32 {
    if path.is_null() {
        ERRNO = EFAULT;
        return -1;
    }

    let result = libbreenix::raw::syscall2(
        libbreenix::syscall::nr::MKDIR,
        path as u64,
        mode as u64,
    ) as i64;
    syscall_result_to_c_int(result)
}

/// rmdir - remove a directory
#[no_mangle]
pub unsafe extern "C" fn rmdir(path: *const u8) -> i32 {
    if path.is_null() {
        ERRNO = EFAULT;
        return -1;
    }

    let result = libbreenix::raw::syscall1(
        libbreenix::syscall::nr::RMDIR,
        path as u64,
    ) as i64;
    syscall_result_to_c_int(result)
}

/// link - create a hard link
#[no_mangle]
pub unsafe extern "C" fn link(oldpath: *const u8, newpath: *const u8) -> i32 {
    if oldpath.is_null() || newpath.is_null() {
        ERRNO = EFAULT;
        return -1;
    }

    let result = libbreenix::raw::syscall2(
        libbreenix::syscall::nr::LINK,
        oldpath as u64,
        newpath as u64,
    ) as i64;
    syscall_result_to_c_int(result)
}

/// linkat - create a hard link relative to directory file descriptors
///
/// We ignore dirfd and flags, treating paths as absolute (Breenix doesn't
/// support AT_FDCWD/AT_ operations yet). This is sufficient for std::fs::hard_link.
#[no_mangle]
pub unsafe extern "C" fn linkat(
    _olddirfd: i32,
    oldpath: *const u8,
    _newdirfd: i32,
    newpath: *const u8,
    _flags: i32,
) -> i32 {
    link(oldpath, newpath)
}

/// symlink - create a symbolic link
#[no_mangle]
pub unsafe extern "C" fn symlink(target: *const u8, linkpath: *const u8) -> i32 {
    if target.is_null() || linkpath.is_null() {
        ERRNO = EFAULT;
        return -1;
    }

    let result = libbreenix::raw::syscall2(
        libbreenix::syscall::nr::SYMLINK,
        target as u64,
        linkpath as u64,
    ) as i64;
    syscall_result_to_c_int(result)
}

/// access - check user's permissions for a file
#[no_mangle]
pub unsafe extern "C" fn access(path: *const u8, mode: i32) -> i32 {
    if path.is_null() {
        ERRNO = EFAULT;
        return -1;
    }

    let result = libbreenix::raw::syscall2(
        libbreenix::syscall::nr::ACCESS,
        path as u64,
        mode as u64,
    ) as i64;
    syscall_result_to_c_int(result)
}

/// getcwd - get current working directory
///
/// Returns buf on success, NULL on error (sets errno).
#[no_mangle]
pub unsafe extern "C" fn getcwd(buf: *mut u8, size: usize) -> *mut u8 {
    if buf.is_null() || size == 0 {
        ERRNO = EINVAL;
        return core::ptr::null_mut();
    }

    let result = libbreenix::raw::syscall2(
        libbreenix::syscall::nr::GETCWD,
        buf as u64,
        size as u64,
    ) as i64;

    if result < 0 {
        set_errno_from_result(result);
        core::ptr::null_mut()
    } else {
        buf
    }
}

/// chdir - change working directory
#[no_mangle]
pub unsafe extern "C" fn chdir(path: *const u8) -> i32 {
    if path.is_null() {
        ERRNO = EFAULT;
        return -1;
    }

    let result = libbreenix::raw::syscall1(
        libbreenix::syscall::nr::CHDIR,
        path as u64,
    ) as i64;
    syscall_result_to_c_int(result)
}

/// isatty - test whether a file descriptor refers to a terminal
///
/// Implemented by attempting an ioctl(TIOCGWINSZ). If it succeeds, the fd
/// is a terminal. If it fails with ENOTTY, it is not.
#[no_mangle]
pub unsafe extern "C" fn isatty(fd: i32) -> i32 {
    // TIOCGWINSZ = 0x5413
    // winsize struct is 8 bytes (2x u16 rows, cols, 2x u16 xpixel, ypixel)
    let mut winsize: [u8; 8] = [0; 8];
    let result = libbreenix::raw::syscall3(
        libbreenix::syscall::nr::IOCTL,
        fd as u64,
        0x5413, // TIOCGWINSZ
        winsize.as_mut_ptr() as u64,
    ) as i64;

    if result < 0 {
        set_errno_from_result(result);
        0 // Not a terminal
    } else {
        1 // Is a terminal
    }
}

/// ioctl - device control
#[no_mangle]
pub unsafe extern "C" fn ioctl(fd: i32, request: u64, arg: u64) -> i32 {
    let result = libbreenix::raw::syscall3(
        libbreenix::syscall::nr::IOCTL,
        fd as u64,
        request,
        arg,
    ) as i64;
    syscall_result_to_c_int(result)
}

/// getdents64 - get directory entries
#[no_mangle]
pub unsafe extern "C" fn getdents64(fd: i32, buf: *mut u8, count: usize) -> isize {
    if buf.is_null() {
        ERRNO = EFAULT;
        return -1;
    }

    let result = libbreenix::raw::syscall3(
        libbreenix::syscall::nr::GETDENTS64,
        fd as u64,
        buf as u64,
        count as u64,
    ) as i64;
    syscall_result_to_c_ssize(result)
}

/// ftruncate - truncate a file to a specified length
#[no_mangle]
pub unsafe extern "C" fn ftruncate(_fd: i32, _length: i64) -> i32 {
    ERRNO = ENOSYS;
    -1
}

/// ftruncate64 - same as ftruncate on 64-bit
#[no_mangle]
pub unsafe extern "C" fn ftruncate64(fd: i32, length: i64) -> i32 {
    ftruncate(fd, length)
}

/// fsync - synchronize file state with storage
#[no_mangle]
pub extern "C" fn fsync(_fd: i32) -> i32 {
    0 // No-op for now
}

/// fdatasync - synchronize file data with storage
#[no_mangle]
pub extern "C" fn fdatasync(_fd: i32) -> i32 {
    0 // No-op for now
}

/// fchmod - change file mode bits (by fd)
#[no_mangle]
pub unsafe extern "C" fn fchmod(_fd: i32, _mode: u32) -> i32 {
    ERRNO = ENOSYS;
    -1
}

/// fchown - change file owner/group (by fd)
#[no_mangle]
pub unsafe extern "C" fn fchown(_fd: i32, _owner: u32, _group: u32) -> i32 {
    ERRNO = ENOSYS;
    -1
}

/// chmod - change file mode bits (by path)
#[no_mangle]
pub unsafe extern "C" fn chmod(_path: *const u8, _mode: u32) -> i32 {
    ERRNO = ENOSYS;
    -1
}

/// chown - change file owner/group (by path)
#[no_mangle]
pub unsafe extern "C" fn chown(_path: *const u8, _owner: u32, _group: u32) -> i32 {
    ERRNO = ENOSYS;
    -1
}

/// utimes - change file access and modification times
#[no_mangle]
pub unsafe extern "C" fn utimes(_path: *const u8, _times: *const u8) -> i32 {
    ERRNO = ENOSYS;
    -1
}

/// fcntl - file control
#[no_mangle]
pub unsafe extern "C" fn fcntl(fd: i32, cmd: i32, arg: u64) -> i32 {
    let result = libbreenix::io::fcntl(Fd::from_raw(fd as u64), cmd, arg as i64);
    result_i64_to_c_int(result)
}

// =============================================================================
// Process Control
// =============================================================================

/// Terminate the calling process.
#[no_mangle]
pub extern "C" fn exit(status: i32) -> ! {
    libbreenix::process::exit(status)
}

/// Terminate the calling process immediately.
#[no_mangle]
pub extern "C" fn _exit(status: i32) -> ! {
    libbreenix::process::exit(status)
}

/// Terminate all threads in the current process group.
///
/// For now this is equivalent to exit() since we are single-threaded per process.
#[no_mangle]
pub extern "C" fn exit_group(status: i32) -> ! {
    libbreenix::process::exit(status)
}

/// set_tid_address - Store TID address for thread exit notification.
///
/// Returns the caller's thread ID.
#[no_mangle]
pub extern "C" fn set_tid_address(tidptr: *mut i32) -> i32 {
    let result = unsafe {
        libbreenix::syscall::raw::syscall1(
            libbreenix::syscall::nr::SET_TID_ADDRESS,
            tidptr as u64,
        )
    } as i64;
    syscall_result_to_c_int(result)
}

/// Get the process ID of the calling process.
#[no_mangle]
pub extern "C" fn getpid() -> i32 {
    match libbreenix::process::getpid() { Ok(pid) => pid.raw() as i32, Err(_) => -1, }
}

/// Get the thread ID of the calling thread.
#[no_mangle]
pub extern "C" fn gettid() -> i32 {
    match libbreenix::process::gettid() { Ok(tid) => tid.raw() as i32, Err(_) => -1, }
}

/// Get the parent process ID.
#[no_mangle]
pub extern "C" fn getppid() -> i32 {
    let result = unsafe {
        libbreenix::syscall::raw::syscall0(libbreenix::syscall::nr::GETPPID)
    } as i64;
    syscall_result_to_c_int(result)
}

/// Get real user ID.
#[no_mangle]
pub extern "C" fn getuid() -> u32 {
    0 // root - single-user system
}

/// Get effective user ID.
#[no_mangle]
pub extern "C" fn geteuid() -> u32 {
    0 // root
}

/// Get real group ID.
#[no_mangle]
pub extern "C" fn getgid() -> u32 {
    0 // root group
}

/// Get effective group ID.
#[no_mangle]
pub extern "C" fn getegid() -> u32 {
    0 // root group
}

/// setpgid - set process group ID
#[no_mangle]
pub extern "C" fn setpgid(pid: i32, pgid: i32) -> i32 {
    result_unit_to_c_int(libbreenix::process::setpgid(pid, pgid))
}

/// fork - create a child process
#[no_mangle]
pub extern "C" fn fork() -> i32 {
    match libbreenix::process::fork() {
        Ok(libbreenix::process::ForkResult::Child) => 0,
        Ok(libbreenix::process::ForkResult::Parent(pid)) => pid.raw() as i32,
        Err(e) => set_errno_from_error(e),
    }
}

/// execve - execute a program
///
/// Wires to libbreenix::process::execv (envp is ignored for now).
#[no_mangle]
pub unsafe extern "C" fn execve(
    path: *const u8,
    argv: *const *const u8,
    _envp: *const *const u8,
) -> i32 {
    if path.is_null() {
        ERRNO = EFAULT;
        return -1;
    }

    let result = libbreenix::raw::syscall2(
        libbreenix::syscall::nr::EXEC,
        path as u64,
        argv as u64,
    ) as i64;
    // execve should not return on success
    syscall_result_to_c_int(result)
}

/// waitpid - wait for a child process
#[no_mangle]
pub unsafe extern "C" fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32 {
    match libbreenix::process::waitpid(pid, status, options) {
        Ok(pid) => pid.raw() as i32,
        Err(e) => set_errno_from_error(e),
    }
}

/// kill - send signal to a process
#[no_mangle]
pub extern "C" fn kill(pid: i32, sig: i32) -> i32 {
    result_unit_to_c_int(libbreenix::signal::kill(pid, sig))
}

/// raise - send a signal to the calling process
#[no_mangle]
pub extern "C" fn raise(sig: i32) -> i32 {
    kill(getpid(), sig)
}

// =============================================================================
// Memory Management
// =============================================================================

/// Map memory into the process address space.
#[no_mangle]
pub unsafe extern "C" fn mmap(
    addr: *mut u8,
    len: usize,
    prot: i32,
    flags: i32,
    fd: i32,
    offset: i64,
) -> *mut u8 {
    let result = libbreenix::raw::syscall6(
        9, // MMAP syscall number
        addr as u64,
        len as u64,
        prot as u64,
        flags as u64,
        fd as u64,
        offset as u64,
    );

    let result_signed = result as i64;
    if result_signed < 0 && result_signed >= -4096 {
        ERRNO = (-result_signed) as i32;
        libbreenix::memory::MAP_FAILED
    } else {
        result as *mut u8
    }
}

/// Unmap memory from the process address space.
#[no_mangle]
pub unsafe extern "C" fn munmap(addr: *mut u8, len: usize) -> i32 {
    result_unit_to_c_int(libbreenix::memory::munmap(addr, len))
}

/// Change protection on a region of memory.
#[no_mangle]
pub unsafe extern "C" fn mprotect(addr: *mut u8, len: usize, prot: i32) -> i32 {
    result_unit_to_c_int(libbreenix::memory::mprotect(addr, len, prot))
}

/// Change the program break (heap end).
#[no_mangle]
pub unsafe extern "C" fn brk(addr: *mut u8) -> i32 {
    let result = libbreenix::memory::brk(addr as u64);

    if result == 0 && !addr.is_null() {
        ERRNO = libbreenix::Errno::ENOMEM as i32;
        -1
    } else {
        0
    }
}

/// Allocate memory by moving the program break.
#[no_mangle]
pub unsafe extern "C" fn sbrk(increment: isize) -> *mut u8 {
    if increment == 0 {
        return libbreenix::memory::get_brk() as *mut u8;
    }

    if increment < 0 {
        ERRNO = libbreenix::Errno::EINVAL as i32;
        return usize::MAX as *mut u8;
    }

    let result = libbreenix::memory::sbrk(increment as usize);

    if result.is_null() {
        ERRNO = libbreenix::Errno::ENOMEM as i32;
        usize::MAX as *mut u8
    } else {
        result
    }
}

// =============================================================================
// Memory Constants (re-exported for convenience)
// =============================================================================

pub const PROT_NONE: i32 = libbreenix::memory::PROT_NONE;
pub const PROT_READ: i32 = libbreenix::memory::PROT_READ;
pub const PROT_WRITE: i32 = libbreenix::memory::PROT_WRITE;
pub const PROT_EXEC: i32 = libbreenix::memory::PROT_EXEC;

pub const MAP_SHARED: i32 = libbreenix::memory::MAP_SHARED;
pub const MAP_PRIVATE: i32 = libbreenix::memory::MAP_PRIVATE;
pub const MAP_FIXED: i32 = libbreenix::memory::MAP_FIXED;
pub const MAP_ANONYMOUS: i32 = libbreenix::memory::MAP_ANONYMOUS;

pub const MAP_FAILED: *mut u8 = usize::MAX as *mut u8;

// =============================================================================
// Errno Constants (re-exported for C compatibility)
// =============================================================================

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
/// Uses a naked function to properly extract argc/argv from the stack
/// as set up by the kernel.
///
/// Stack layout at entry:
/// ```text
/// [rsp]     = argc
/// [rsp+8]   = argv[0]
/// [rsp+16]  = argv[1]
/// ...
/// ```
#[cfg(target_arch = "x86_64")]
#[unsafe(naked)]
#[no_mangle]
pub unsafe extern "C" fn _start() -> ! {
    core::arch::naked_asm!(
        "mov rdi, rsp",  // Pass stack pointer as first argument
        "and rsp, -16",  // Align stack to 16 bytes
        "call {entry}",
        entry = sym _start_rust,
    )
}

/// ARM64 _start entry point
#[cfg(target_arch = "aarch64")]
#[unsafe(naked)]
#[no_mangle]
pub unsafe extern "C" fn _start() -> ! {
    core::arch::naked_asm!(
        "mov x0, sp",    // Pass stack pointer as first argument
        "and sp, x0, #-16",  // Align stack to 16 bytes
        "bl {entry}",
        entry = sym _start_rust,
    )
}

/// Rust entry point called from _start with the stack pointer.
///
/// Extracts argc and argv from the stack and calls main().
extern "C" fn _start_rust(sp: *const u64) -> ! {
    unsafe {
        let argc = *sp as isize;
        let argv = sp.add(1) as *const *const u8;
        extern "C" {
            fn main(argc: isize, argv: *const *const u8) -> isize;
        }
        let ret = main(argc, argv);
        exit(ret as i32);
    }
}

// =============================================================================
// Memory Allocation Functions
// =============================================================================

/// Header size for allocation tracking.
const ALLOC_HEADER_SIZE: usize = 16;

/// Get the size of an allocation from its header.
#[inline]
unsafe fn get_alloc_size(ptr: *mut u8) -> usize {
    let header = ptr.sub(ALLOC_HEADER_SIZE);
    *(header as *const usize)
}

/// malloc - allocate memory with size tracking
#[no_mangle]
pub unsafe extern "C" fn malloc(size: usize) -> *mut u8 {
    if size == 0 {
        return core::ptr::null_mut();
    }

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
        *(ptr as *mut usize) = size;
        *((ptr as *mut usize).add(1)) = 0;
        ptr.add(ALLOC_HEADER_SIZE)
    }
}

/// free - deallocate memory
#[no_mangle]
pub unsafe extern "C" fn free(ptr: *mut u8) {
    if ptr.is_null() {
        return;
    }
    let header = ptr.sub(ALLOC_HEADER_SIZE);
    let size = *(header as *const usize);
    let base_ptr_field = *((header as *const usize).add(1));

    let header_addr = header as usize;
    if base_ptr_field != 0 && base_ptr_field <= header_addr {
        let base_ptr = base_ptr_field as *mut u8;
        let total_size = (ptr as usize - base_ptr_field) + size;
        munmap(base_ptr, total_size);
    } else {
        let munmap_size = size + ALLOC_HEADER_SIZE;
        munmap(header, munmap_size);
    }
}

/// calloc - allocate zero-initialized memory
#[no_mangle]
pub unsafe extern "C" fn calloc(nmemb: usize, size: usize) -> *mut u8 {
    let total = match nmemb.checked_mul(size) {
        Some(t) => t,
        None => return core::ptr::null_mut(),
    };
    let ptr = malloc(total);
    if !ptr.is_null() {
        // malloc via mmap already returns zero-initialized memory,
        // but be explicit for correctness
        core::ptr::write_bytes(ptr, 0, total);
    }
    ptr
}

/// realloc - resize memory allocation
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
        let copy_size = core::cmp::min(old_size, size);
        core::ptr::copy_nonoverlapping(ptr, new_ptr, copy_size);
        free(ptr);
    }
    new_ptr
}

/// posix_memalign - allocate aligned memory
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

    if alignment <= ALLOC_HEADER_SIZE {
        let ptr = malloc(size);
        if ptr.is_null() {
            return ENOMEM;
        }
        *memptr = ptr;
        return 0;
    }

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

    let after_header = base_ptr.add(ALLOC_HEADER_SIZE) as usize;
    let aligned_addr = (after_header + alignment - 1) & !(alignment - 1);
    let user_ptr = aligned_addr as *mut u8;

    let header = user_ptr.sub(ALLOC_HEADER_SIZE);
    *(header as *mut usize) = size;
    *((header as *mut usize).add(1)) = base_ptr as usize;

    *memptr = user_ptr;
    0
}

// =============================================================================
// String/Utility Functions
// =============================================================================

/// abort function - required by various runtime components
#[no_mangle]
pub extern "C" fn abort() -> ! {
    exit(134) // 128 + SIGABRT (6)
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

/// setenv - set environment variable (no-op, single-process)
#[no_mangle]
pub unsafe extern "C" fn setenv(_name: *const u8, _value: *const u8, _overwrite: i32) -> i32 {
    0
}

/// unsetenv - remove environment variable (no-op, single-process)
#[no_mangle]
pub unsafe extern "C" fn unsetenv(_name: *const u8) -> i32 {
    0
}

/// Get random bytes from the kernel.
#[no_mangle]
pub unsafe extern "C" fn getrandom(buf: *mut u8, buflen: usize, flags: u32) -> isize {
    let ret = libbreenix::syscall::raw::syscall3(
        libbreenix::syscall::nr::GETRANDOM,
        buf as u64,
        buflen as u64,
        flags as u64,
    );
    let ret = ret as i64;
    if ret < 0 {
        ERRNO = (-ret) as i32;
        -1
    } else {
        ret as isize
    }
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
        _ => -1,
    }
}

/// __xpg_strerror_r - convert error number to string (XPG version)
#[no_mangle]
pub unsafe extern "C" fn __xpg_strerror_r(errnum: i32, buf: *mut u8, buflen: usize) -> i32 {
    let msg: &[u8] = match errnum {
        0 => b"Success\0",
        1 => b"Operation not permitted\0",
        2 => b"No such file or directory\0",
        3 => b"No such process\0",
        4 => b"Interrupted system call\0",
        5 => b"Input/output error\0",
        9 => b"Bad file descriptor\0",
        10 => b"No child processes\0",
        11 => b"Resource temporarily unavailable\0",
        12 => b"Cannot allocate memory\0",
        13 => b"Permission denied\0",
        14 => b"Bad address\0",
        17 => b"File exists\0",
        20 => b"Not a directory\0",
        21 => b"Is a directory\0",
        22 => b"Invalid argument\0",
        25 => b"Inappropriate ioctl for device\0",
        28 => b"No space left on device\0",
        32 => b"Broken pipe\0",
        38 => b"Function not implemented\0",
        _ => b"Unknown error\0",
    };
    let copy_len = core::cmp::min(msg.len() - 1, buflen - 1);
    core::ptr::copy_nonoverlapping(msg.as_ptr(), buf, copy_len);
    *buf.add(copy_len) = 0;
    0
}

/// strerror_r - POSIX strerror_r (same as __xpg_strerror_r)
#[no_mangle]
pub unsafe extern "C" fn strerror_r(errnum: i32, buf: *mut u8, buflen: usize) -> i32 {
    __xpg_strerror_r(errnum, buf, buflen)
}

/// getauxval - get auxiliary vector value (stub)
#[no_mangle]
pub extern "C" fn getauxval(type_: u64) -> u64 {
    const AT_PAGESZ: u64 = 6;
    const AT_HWCAP: u64 = 16;
    const AT_HWCAP2: u64 = 26;

    match type_ {
        AT_PAGESZ => 4096,
        AT_HWCAP | AT_HWCAP2 => 0,
        _ => 0,
    }
}

// =============================================================================
// Signal Handling
// =============================================================================

/// signal - set signal handler (simple interface)
#[no_mangle]
pub extern "C" fn signal(_signum: i32, _handler: usize) -> usize {
    0 // SIG_DFL
}

/// sigaction - examine and change signal action
///
/// Wires through to libbreenix::signal::sigaction, converting from
/// the C sigaction struct layout to the kernel's layout.
#[no_mangle]
pub unsafe extern "C" fn sigaction(signum: i32, act: *const u8, oldact: *mut u8) -> i32 {
    // The C sigaction struct layout:
    //   offset 0: sa_sigaction (handler) - 8 bytes
    //   offset 8: sa_mask (sigset_t) - 8 bytes
    //   offset 16: sa_flags (c_int) - 4 bytes
    //   offset 20: padding - 4 bytes
    //   offset 24: sa_restorer (Option<extern "C" fn()>) - 8 bytes
    //
    // The kernel Sigaction struct layout (libbreenix::signal::Sigaction):
    //   offset 0: handler - 8 bytes (u64)
    //   offset 8: mask - 8 bytes (u64)
    //   offset 16: flags - 8 bytes (u64)
    //   offset 24: restorer - 8 bytes (u64)

    let act_ptr = if act.is_null() {
        core::ptr::null()
    } else {
        // Convert C sigaction to kernel Sigaction
        let c_handler = *(act as *const u64);
        let c_mask = *(act.add(8) as *const u64);
        let c_flags = *(act.add(16) as *const i32);
        let c_restorer = *(act.add(24) as *const u64);

        static mut KERNEL_ACT: libbreenix::signal::Sigaction = libbreenix::signal::Sigaction {
            handler: 0,
            mask: 0,
            flags: 0,
            restorer: 0,
        };
        KERNEL_ACT.handler = c_handler;
        KERNEL_ACT.mask = c_mask;
        KERNEL_ACT.flags = c_flags as u64;
        KERNEL_ACT.restorer = c_restorer;
        &raw const KERNEL_ACT
    };

    let mut kernel_oldact = libbreenix::signal::Sigaction {
        handler: 0,
        mask: 0,
        flags: 0,
        restorer: 0,
    };

    let oldact_opt = if oldact.is_null() {
        None
    } else {
        Some(&mut kernel_oldact)
    };

    let act_opt = if act_ptr.is_null() {
        None
    } else {
        Some(unsafe { &*act_ptr })
    };

    match libbreenix::signal::sigaction(signum, act_opt, oldact_opt) {
        Ok(()) => {
            if !oldact.is_null() {
                *(oldact as *mut u64) = kernel_oldact.handler;
                *(oldact.add(8) as *mut u64) = kernel_oldact.mask;
                *(oldact.add(16) as *mut i32) = kernel_oldact.flags as i32;
                // padding at offset 20
                *(oldact.add(24) as *mut u64) = kernel_oldact.restorer;
            }
            0
        }
        Err(e) => set_errno_from_error(e),
    }
}

/// sigaltstack - set/get signal stack context
#[no_mangle]
pub unsafe extern "C" fn sigaltstack(ss: *const u8, old_ss: *mut u8) -> i32 {
    // C stack_t layout:
    //   offset 0: ss_sp (*mut void) - 8 bytes
    //   offset 8: ss_flags (c_int) - 4 bytes
    //   offset 12: padding - 4 bytes
    //   offset 16: ss_size (size_t) - 8 bytes
    //
    // Kernel StackT layout:
    //   offset 0: ss_sp (u64) - 8 bytes
    //   offset 8: ss_flags (i32) - 4 bytes
    //   offset 12: _pad (i32) - 4 bytes
    //   offset 16: ss_size (usize) - 8 bytes

    let ss_opt = if ss.is_null() {
        None
    } else {
        static mut KERNEL_SS: libbreenix::signal::StackT = libbreenix::signal::StackT {
            ss_sp: 0,
            ss_flags: 2, // SS_DISABLE
            _pad: 0,
            ss_size: 0,
        };
        KERNEL_SS.ss_sp = *(ss as *const u64);
        KERNEL_SS.ss_flags = *(ss.add(8) as *const i32);
        KERNEL_SS.ss_size = *(ss.add(16) as *const usize);
        Some(&*(&raw const KERNEL_SS))
    };

    let mut kernel_old = libbreenix::signal::StackT {
        ss_sp: 0,
        ss_flags: 2,
        _pad: 0,
        ss_size: 0,
    };

    let old_opt = if old_ss.is_null() {
        None
    } else {
        Some(&mut kernel_old)
    };

    match libbreenix::signal::sigaltstack(ss_opt, old_opt) {
        Ok(()) => {
            if !old_ss.is_null() {
                *(old_ss as *mut u64) = kernel_old.ss_sp;
                *(old_ss.add(8) as *mut i32) = kernel_old.ss_flags;
                *(old_ss.add(16) as *mut usize) = kernel_old.ss_size;
            }
            0
        }
        Err(e) => set_errno_from_error(e),
    }
}

/// sigprocmask - examine and change blocked signals
#[no_mangle]
pub unsafe extern "C" fn sigprocmask(
    how: i32,
    set: *const u64,
    oldset: *mut u64,
) -> i32 {
    let set_opt = if set.is_null() { None } else { Some(&*set) };
    let oldset_opt = if oldset.is_null() { None } else { Some(&mut *oldset) };

    match libbreenix::signal::sigprocmask(how, set_opt, oldset_opt) {
        Ok(()) => 0,
        Err(e) => set_errno_from_error(e),
    }
}

// =============================================================================
// Time Functions
// =============================================================================

/// clock_gettime - get time from a clock
#[no_mangle]
pub unsafe extern "C" fn clock_gettime(clk_id: i32, tp: *mut u8) -> i32 {
    if tp.is_null() {
        ERRNO = EFAULT;
        return -1;
    }

    // The timespec struct matches between C and kernel (tv_sec: i64, tv_nsec: i64)
    let result = libbreenix::raw::syscall2(
        libbreenix::syscall::nr::CLOCK_GETTIME,
        clk_id as u64,
        tp as u64,
    ) as i64;
    syscall_result_to_c_int(result)
}

/// nanosleep - high-resolution sleep
#[no_mangle]
pub unsafe extern "C" fn nanosleep(req: *const u8, rem: *mut u8) -> i32 {
    let ret = libbreenix::syscall::raw::syscall2(
        libbreenix::syscall::nr::NANOSLEEP,
        req as u64,
        rem as u64,
    );
    let ret = ret as i64;
    if ret < 0 {
        ERRNO = (-ret) as i32;
        -1
    } else {
        0
    }
}

// =============================================================================
// Socket Functions
// =============================================================================

/// socket - create a socket
#[no_mangle]
pub extern "C" fn socket(domain: i32, sock_type: i32, protocol: i32) -> i32 {
    result_fd_to_c_int(libbreenix::socket::socket(domain, sock_type, protocol))
}

/// bind - bind a socket to an address
#[no_mangle]
pub unsafe extern "C" fn bind(sockfd: i32, addr: *const u8, addrlen: u32) -> i32 {
    if addr.is_null() {
        ERRNO = EFAULT;
        return -1;
    }

    let result = libbreenix::raw::syscall3(
        libbreenix::syscall::nr::BIND,
        sockfd as u64,
        addr as u64,
        addrlen as u64,
    ) as i64;
    syscall_result_to_c_int(result)
}

/// listen - listen for connections on a socket
#[no_mangle]
pub extern "C" fn listen(sockfd: i32, backlog: i32) -> i32 {
    result_unit_to_c_int(libbreenix::socket::listen(Fd::from_raw(sockfd as u64), backlog))
}

/// accept - accept a connection on a socket
#[no_mangle]
pub unsafe extern "C" fn accept(sockfd: i32, addr: *mut u8, addrlen: *mut u32) -> i32 {
    let result = libbreenix::raw::syscall3(
        libbreenix::syscall::nr::ACCEPT,
        sockfd as u64,
        addr as u64,
        addrlen as u64,
    ) as i64;
    syscall_result_to_c_int(result)
}

/// accept4 - accept a connection with flags
#[no_mangle]
pub unsafe extern "C" fn accept4(
    sockfd: i32,
    addr: *mut u8,
    addrlen: *mut u32,
    _flags: i32,
) -> i32 {
    // Ignore flags for now, delegate to accept
    accept(sockfd, addr, addrlen)
}

/// connect - connect a socket to an address
#[no_mangle]
pub unsafe extern "C" fn connect(sockfd: i32, addr: *const u8, addrlen: u32) -> i32 {
    if addr.is_null() {
        ERRNO = EFAULT;
        return -1;
    }

    let result = libbreenix::raw::syscall3(
        libbreenix::syscall::nr::CONNECT,
        sockfd as u64,
        addr as u64,
        addrlen as u64,
    ) as i64;
    syscall_result_to_c_int(result)
}

/// send - send data on a connected socket
#[no_mangle]
pub unsafe extern "C" fn send(sockfd: i32, buf: *const u8, len: usize, flags: i32) -> isize {
    // send is sendto with null address
    sendto(sockfd, buf, len, flags, core::ptr::null(), 0)
}

/// recv - receive data from a connected socket
#[no_mangle]
pub unsafe extern "C" fn recv(sockfd: i32, buf: *mut u8, len: usize, flags: i32) -> isize {
    // recv is recvfrom with null address
    recvfrom(sockfd, buf, len, flags, core::ptr::null_mut(), core::ptr::null_mut())
}

/// sendto - send data to a specific address
#[no_mangle]
pub unsafe extern "C" fn sendto(
    sockfd: i32,
    buf: *const u8,
    len: usize,
    flags: i32,
    dest_addr: *const u8,
    addrlen: u32,
) -> isize {
    let result = libbreenix::raw::syscall6(
        libbreenix::syscall::nr::SENDTO,
        sockfd as u64,
        buf as u64,
        len as u64,
        flags as u64,
        dest_addr as u64,
        addrlen as u64,
    ) as i64;
    syscall_result_to_c_ssize(result)
}

/// recvfrom - receive data and source address
#[no_mangle]
pub unsafe extern "C" fn recvfrom(
    sockfd: i32,
    buf: *mut u8,
    len: usize,
    flags: i32,
    src_addr: *mut u8,
    addrlen: *mut u32,
) -> isize {
    let result = libbreenix::raw::syscall6(
        libbreenix::syscall::nr::RECVFROM,
        sockfd as u64,
        buf as u64,
        len as u64,
        flags as u64,
        src_addr as u64,
        addrlen as u64,
    ) as i64;
    syscall_result_to_c_ssize(result)
}

/// sendmsg - send a message on a socket
#[no_mangle]
pub unsafe extern "C" fn sendmsg(_sockfd: i32, _msg: *const u8, _flags: i32) -> isize {
    ERRNO = ENOSYS;
    -1
}

/// recvmsg - receive a message from a socket
#[no_mangle]
pub unsafe extern "C" fn recvmsg(_sockfd: i32, _msg: *mut u8, _flags: i32) -> isize {
    ERRNO = ENOSYS;
    -1
}

/// setsockopt - set socket options
#[no_mangle]
pub unsafe extern "C" fn setsockopt(
    sockfd: i32,
    level: i32,
    optname: i32,
    optval: *const u8,
    optlen: u32,
) -> i32 {
    let result = libbreenix::raw::syscall5(
        libbreenix::syscall::nr::SETSOCKOPT,
        sockfd as u64,
        level as u64,
        optname as u64,
        optval as u64,
        optlen as u64,
    ) as i64;
    syscall_result_to_c_int(result)
}

/// getsockopt - get socket options
#[no_mangle]
pub unsafe extern "C" fn getsockopt(
    sockfd: i32,
    level: i32,
    optname: i32,
    optval: *mut u8,
    optlen: *mut u32,
) -> i32 {
    let result = libbreenix::raw::syscall5(
        libbreenix::syscall::nr::GETSOCKOPT,
        sockfd as u64,
        level as u64,
        optname as u64,
        optval as u64,
        optlen as u64,
    ) as i64;
    syscall_result_to_c_int(result)
}

/// getpeername - get name of connected peer socket
#[no_mangle]
pub unsafe extern "C" fn getpeername(
    sockfd: i32,
    addr: *mut u8,
    addrlen: *mut u32,
) -> i32 {
    let result = libbreenix::raw::syscall3(
        libbreenix::syscall::nr::GETPEERNAME,
        sockfd as u64,
        addr as u64,
        addrlen as u64,
    ) as i64;
    syscall_result_to_c_int(result)
}

/// getsockname - get socket name
#[no_mangle]
pub unsafe extern "C" fn getsockname(
    sockfd: i32,
    addr: *mut u8,
    addrlen: *mut u32,
) -> i32 {
    let result = libbreenix::raw::syscall3(
        libbreenix::syscall::nr::GETSOCKNAME,
        sockfd as u64,
        addr as u64,
        addrlen as u64,
    ) as i64;
    syscall_result_to_c_int(result)
}

/// shutdown - shut down part of a full-duplex connection
#[no_mangle]
pub extern "C" fn shutdown(sockfd: i32, how: i32) -> i32 {
    result_unit_to_c_int(libbreenix::socket::shutdown(Fd::from_raw(sockfd as u64), how))
}

/// socketpair - create a pair of connected sockets
#[no_mangle]
pub unsafe extern "C" fn socketpair(
    domain: i32,
    sock_type: i32,
    protocol: i32,
    sv: *mut i32,
) -> i32 {
    if sv.is_null() {
        ERRNO = EFAULT;
        return -1;
    }

    match libbreenix::socket::socketpair(domain, sock_type, protocol) {
        Ok((fd0, fd1)) => {
            *sv = fd0.raw() as i32;
            *sv.add(1) = fd1.raw() as i32;
            0
        }
        Err(e) => set_errno_from_error(e),
    }
}

/// select - synchronous I/O multiplexing
#[no_mangle]
pub unsafe extern "C" fn select(
    nfds: i32,
    readfds: *mut u8,
    writefds: *mut u8,
    exceptfds: *mut u8,
    timeout: *mut u8,
) -> i32 {
    let result = libbreenix::raw::syscall5(
        libbreenix::syscall::nr::SELECT,
        nfds as u64,
        readfds as u64,
        writefds as u64,
        exceptfds as u64,
        timeout as u64,
    ) as i64;
    syscall_result_to_c_int(result)
}

// =============================================================================
// Poll
// =============================================================================

/// poll - wait for events on file descriptors
#[no_mangle]
pub unsafe extern "C" fn poll(fds: *mut u8, nfds: u64, timeout: i32) -> i32 {
    let result = libbreenix::raw::syscall3(
        libbreenix::syscall::nr::POLL,
        fds as u64,
        nfds,
        timeout as u64,
    ) as i64;
    syscall_result_to_c_int(result)
}

// =============================================================================
// Syscall and Synchronization Functions
// =============================================================================

/// pause - suspend until signal
#[no_mangle]
pub extern "C" fn pause() -> i32 {
    result_unit_to_c_int(libbreenix::signal::pause())
}

/// syscall - generic syscall interface
#[no_mangle]
pub unsafe extern "C" fn syscall(num: i64, a1: i64, a2: i64, a3: i64, a4: i64, a5: i64, a6: i64) -> i64 {
    const SYS_FUTEX: i64 = 202;
    const SYS_GETRANDOM: i64 = 318;

    match num {
        SYS_FUTEX => {
            0
        }
        SYS_GETRANDOM => {
            -(ENOSYS as i64)
        }
        _ => {
            // Forward unknown syscalls to the kernel
            libbreenix::raw::syscall6(
                num as u64,
                a1 as u64,
                a2 as u64,
                a3 as u64,
                a4 as u64,
                a5 as u64,
                a6 as u64,
            ) as i64
        }
    }
}

// =============================================================================
// Scheduling
// =============================================================================

/// sched_yield - yield the processor
#[no_mangle]
pub extern "C" fn sched_yield() -> i32 {
    let _ = libbreenix::process::yield_now();
    0
}

// =============================================================================
// Thread-Local Storage and Pthread Functions
// =============================================================================

/// pthread_self - get current thread ID
#[no_mangle]
pub extern "C" fn pthread_self() -> usize {
    unsafe {
        libbreenix::syscall::raw::syscall0(libbreenix::syscall::nr::GETTID) as usize
    }
}

/// Clone flags for thread creation
const CLONE_VM: u64 = 0x00000100;
const CLONE_FS: u64 = 0x00000200;
const CLONE_FILES: u64 = 0x00000400;
const CLONE_SIGHAND: u64 = 0x00000800;
const CLONE_THREAD: u64 = 0x00010000;
const CLONE_CHILD_CLEARTID: u64 = 0x00200000;
const CLONE_CHILD_SETTID: u64 = 0x01000000;

/// Futex operation codes
const FUTEX_WAIT: u32 = 0;

/// Thread start info passed through the heap to the child thread
#[repr(C)]
struct ThreadStartInfo {
    func: extern "C" fn(*mut u8) -> *mut u8,
    arg: *mut u8,
    /// Address of the tid word that gets cleared on thread exit (for join)
    tid_addr: *mut u32,
}

/// Entry point for child threads created by pthread_create.
/// This function is set as the RIP for the new thread by the kernel's clone syscall.
/// RDI contains the pointer to a heap-allocated ThreadStartInfo.
extern "C" fn thread_entry(info_ptr: u64) -> ! {
    unsafe {
        let info = info_ptr as *mut ThreadStartInfo;
        let func = (*info).func;
        let arg = (*info).arg;
        // Don't free info - it's in shared memory and the parent may be reading tid_addr
        // The start info is small and will be cleaned up when the process exits.

        // Call the user's thread function
        func(arg);

        // Thread function returned - exit this thread
        libbreenix::process::exit(0);
    }
}

/// pthread_create - create a new thread
#[no_mangle]
pub unsafe extern "C" fn pthread_create(
    thread: *mut usize,
    _attr: *const u8,
    start_routine: extern "C" fn(*mut u8) -> *mut u8,
    arg: *mut u8,
) -> i32 {
    // Allocate stack for the child thread (2MB)
    let stack_size: usize = 2 * 1024 * 1024;
    let stack_base = mmap(
        core::ptr::null_mut(),
        stack_size,
        PROT_READ | PROT_WRITE,
        MAP_PRIVATE | MAP_ANONYMOUS,
        -1,
        0,
    );
    if stack_base == MAP_FAILED {
        return ENOMEM;
    }

    // Stack grows downward - child_stack is the top of the stack
    // Ensure 16-byte alignment
    let stack_top = (stack_base as usize + stack_size) & !0xF;

    // Allocate ThreadStartInfo on the heap (shared memory via CLONE_VM)
    // We use mmap to allocate since we don't have a proper allocator here
    let info_mem = mmap(
        core::ptr::null_mut(),
        4096,
        PROT_READ | PROT_WRITE,
        MAP_PRIVATE | MAP_ANONYMOUS,
        -1,
        0,
    );
    if info_mem == MAP_FAILED {
        munmap(stack_base, stack_size);
        return ENOMEM;
    }

    let info = info_mem as *mut ThreadStartInfo;
    (*info).func = start_routine;
    (*info).arg = arg;

    // The tid word follows the ThreadStartInfo struct
    // This is the address that gets written to 0 on thread exit and futex-woken
    let tid_addr = (info_mem as usize + core::mem::size_of::<ThreadStartInfo>()) as *mut u32;
    (*info).tid_addr = tid_addr;
    // Initialize tid to a non-zero value (will be set by kernel via CLONE_CHILD_SETTID)
    *tid_addr = 0xFFFF;

    // Clone flags for thread creation
    let flags = CLONE_VM | CLONE_FS | CLONE_FILES | CLONE_SIGHAND
        | CLONE_THREAD | CLONE_CHILD_CLEARTID | CLONE_CHILD_SETTID;

    // Call clone syscall: clone(flags, child_stack, fn_ptr, fn_arg, child_tidptr)
    let ret = libbreenix::syscall::raw::syscall5(
        libbreenix::syscall::nr::CLONE,
        flags,
        stack_top as u64,
        thread_entry as u64,
        info as u64,
        tid_addr as u64,
    ) as i64;

    if ret < 0 {
        munmap(stack_base, stack_size);
        munmap(info_mem, 4096);
        return -(ret as i32);
    }

    // Store the thread handle (we use the tid_addr as the handle since
    // pthread_join needs to know where to futex-wait)
    if !thread.is_null() {
        *thread = tid_addr as usize;
    }

    0
}

/// pthread_join - wait for thread termination
#[no_mangle]
pub extern "C" fn pthread_join(thread: usize, _retval: *mut *mut u8) -> i32 {
    if thread == 0 {
        return EINVAL;
    }

    // thread is the tid_addr pointer set up by pthread_create
    let tid_addr = thread as *const u32;

    // Wait for the tid word to become 0 (kernel writes 0 on thread exit)
    loop {
        let tid_val = unsafe { core::ptr::read_volatile(tid_addr) };
        if tid_val == 0 {
            // Thread has exited
            return 0;
        }

        // FUTEX_WAIT: block until *tid_addr != tid_val
        unsafe {
            libbreenix::syscall::raw::syscall6(
                libbreenix::syscall::nr::FUTEX,
                tid_addr as u64,
                FUTEX_WAIT as u64,
                tid_val as u64,
                0, // no timeout
                0, // uaddr2 unused
                0, // val3 unused
            );
        }
        // Loop back to check - may have been spuriously woken
    }
}

/// pthread_detach - detach a thread (stub - returns 0)
#[no_mangle]
pub extern "C" fn pthread_detach(_thread: usize) -> i32 {
    0
}

/// pthread_key_create - create a thread-local key
#[no_mangle]
pub unsafe extern "C" fn pthread_key_create(
    key: *mut u32,
    _destructor: Option<unsafe extern "C" fn(*mut u8)>,
) -> i32 {
    static mut NEXT_KEY: u32 = 0;
    *key = NEXT_KEY;
    NEXT_KEY += 1;
    0
}

/// pthread_key_delete - delete a thread-local key
#[no_mangle]
pub extern "C" fn pthread_key_delete(_key: u32) -> i32 {
    0
}

/// pthread_getspecific - get thread-local value
#[no_mangle]
pub extern "C" fn pthread_getspecific(_key: u32) -> *mut u8 {
    core::ptr::null_mut()
}

/// pthread_setspecific - set thread-local value
#[no_mangle]
pub extern "C" fn pthread_setspecific(_key: u32, _value: *const u8) -> i32 {
    0
}

/// pthread_getattr_np - get thread attributes
#[no_mangle]
pub extern "C" fn pthread_getattr_np(_thread: usize, _attr: *mut u8) -> i32 {
    0
}

/// pthread_attr_init - initialize thread attributes
#[no_mangle]
pub extern "C" fn pthread_attr_init(_attr: *mut u8) -> i32 {
    0
}

/// pthread_attr_destroy - destroy thread attributes
#[no_mangle]
pub extern "C" fn pthread_attr_destroy(_attr: *mut u8) -> i32 {
    0
}

/// pthread_attr_setstacksize - set stack size attribute
#[no_mangle]
pub extern "C" fn pthread_attr_setstacksize(_attr: *mut u8, _stacksize: usize) -> i32 {
    0
}

/// pthread_attr_getstack - get stack attributes
#[no_mangle]
pub unsafe extern "C" fn pthread_attr_getstack(
    _attr: *const u8,
    stackaddr: *mut *mut u8,
    stacksize: *mut usize,
) -> i32 {
    *stackaddr = 0x7fff0000_00000000_u64 as *mut u8;
    *stacksize = 8 * 1024 * 1024; // 8 MB stack
    0
}

/// pthread_attr_getguardsize - get guard size attribute
#[no_mangle]
pub unsafe extern "C" fn pthread_attr_getguardsize(
    _attr: *const u8,
    guardsize: *mut usize,
) -> i32 {
    if !guardsize.is_null() {
        *guardsize = 4096; // One page guard
    }
    0
}

/// pthread_attr_setguardsize - set guard size attribute
#[no_mangle]
pub extern "C" fn pthread_attr_setguardsize(_attr: *mut u8, _guardsize: usize) -> i32 {
    0
}

/// pthread_setname_np - set thread name
#[no_mangle]
pub extern "C" fn pthread_setname_np(_thread: usize, _name: *const u8) -> i32 {
    0
}

// =============================================================================
// Pthread Mutex Functions (no-op stubs for single-threaded)
// =============================================================================

#[no_mangle]
pub extern "C" fn pthread_mutex_init(_mutex: *mut u8, _attr: *const u8) -> i32 {
    0
}

#[no_mangle]
pub extern "C" fn pthread_mutex_destroy(_mutex: *mut u8) -> i32 {
    0
}

#[no_mangle]
pub extern "C" fn pthread_mutex_lock(_mutex: *mut u8) -> i32 {
    0
}

#[no_mangle]
pub extern "C" fn pthread_mutex_trylock(_mutex: *mut u8) -> i32 {
    0
}

#[no_mangle]
pub extern "C" fn pthread_mutex_unlock(_mutex: *mut u8) -> i32 {
    0
}

#[no_mangle]
pub extern "C" fn pthread_mutexattr_init(_attr: *mut u8) -> i32 {
    0
}

#[no_mangle]
pub extern "C" fn pthread_mutexattr_destroy(_attr: *mut u8) -> i32 {
    0
}

#[no_mangle]
pub extern "C" fn pthread_mutexattr_settype(_attr: *mut u8, _kind: i32) -> i32 {
    0
}

// =============================================================================
// Pthread Condition Variable Functions (no-op stubs)
// =============================================================================

#[no_mangle]
pub extern "C" fn pthread_cond_init(_cond: *mut u8, _attr: *const u8) -> i32 {
    0
}

#[no_mangle]
pub extern "C" fn pthread_cond_destroy(_cond: *mut u8) -> i32 {
    0
}

#[no_mangle]
pub extern "C" fn pthread_cond_signal(_cond: *mut u8) -> i32 {
    0
}

#[no_mangle]
pub extern "C" fn pthread_cond_broadcast(_cond: *mut u8) -> i32 {
    0
}

#[no_mangle]
pub extern "C" fn pthread_cond_wait(_cond: *mut u8, _mutex: *mut u8) -> i32 {
    0
}

#[no_mangle]
pub unsafe extern "C" fn pthread_cond_timedwait(
    _cond: *mut u8,
    _mutex: *mut u8,
    _abstime: *const u8,
) -> i32 {
    0
}

#[no_mangle]
pub extern "C" fn pthread_condattr_init(_attr: *mut u8) -> i32 {
    0
}

#[no_mangle]
pub extern "C" fn pthread_condattr_destroy(_attr: *mut u8) -> i32 {
    0
}

#[no_mangle]
pub extern "C" fn pthread_condattr_setclock(_attr: *mut u8, _clock: i32) -> i32 {
    0
}

// =============================================================================
// Pthread Read-Write Lock Functions (no-op stubs)
// =============================================================================

#[no_mangle]
pub extern "C" fn pthread_rwlock_init(_rwlock: *mut u8, _attr: *const u8) -> i32 {
    0
}

#[no_mangle]
pub extern "C" fn pthread_rwlock_destroy(_rwlock: *mut u8) -> i32 {
    0
}

#[no_mangle]
pub extern "C" fn pthread_rwlock_rdlock(_rwlock: *mut u8) -> i32 {
    0
}

#[no_mangle]
pub extern "C" fn pthread_rwlock_tryrdlock(_rwlock: *mut u8) -> i32 {
    0
}

#[no_mangle]
pub extern "C" fn pthread_rwlock_wrlock(_rwlock: *mut u8) -> i32 {
    0
}

#[no_mangle]
pub extern "C" fn pthread_rwlock_trywrlock(_rwlock: *mut u8) -> i32 {
    0
}

#[no_mangle]
pub extern "C" fn pthread_rwlock_unlock(_rwlock: *mut u8) -> i32 {
    0
}

// =============================================================================
// Additional libc functions needed by Rust std
// =============================================================================

/// readv - scatter read
#[no_mangle]
pub unsafe extern "C" fn readv(fd: i32, iov: *const Iovec, iovcnt: i32) -> isize {
    let mut total: isize = 0;
    for i in 0..iovcnt as usize {
        let vec = &*iov.add(i);
        let result = read(fd, vec.iov_base, vec.iov_len);
        if result < 0 {
            return result;
        }
        total += result;
    }
    total
}

// =============================================================================
// POSIX Directory Functions (opendir/readdir_r/closedir)
// =============================================================================

/// Internal DIR structure for directory iteration.
///
/// Contains a file descriptor, a buffer for getdents64 results,
/// and position tracking within the buffer.
#[repr(C)]
pub struct Dir {
    fd: i32,
    buf: [u8; 2048],
    buf_len: usize,   // How many valid bytes are in buf
    buf_pos: usize,   // Current read position within buf
    eof: bool,        // Whether getdents64 returned 0 (no more entries)
}

/// POSIX dirent structure
///
/// Note: d_name is a fixed-size array matching the Linux convention.
#[repr(C)]
pub struct Dirent {
    pub d_ino: u64,
    pub d_off: i64,
    pub d_reclen: u16,
    pub d_type: u8,
    pub d_name: [u8; 256],
}

const O_RDONLY_DIR: i32 = 0;
const O_DIRECTORY: i32 = 0o200000;

/// opendir - open a directory stream
#[no_mangle]
pub unsafe extern "C" fn opendir(name: *const u8) -> *mut Dir {
    if name.is_null() {
        ERRNO = EFAULT;
        return core::ptr::null_mut();
    }

    // Open the directory with O_RDONLY | O_DIRECTORY
    let fd = open(name, O_RDONLY_DIR | O_DIRECTORY, 0);
    if fd < 0 {
        // errno already set by open()
        return core::ptr::null_mut();
    }

    // Allocate a Dir struct using sbrk (we don't have malloc)
    let dir_size = core::mem::size_of::<Dir>();
    let ptr = sbrk(dir_size as isize) as *mut Dir;
    if ptr.is_null() || (ptr as usize) == usize::MAX {
        close(fd);
        ERRNO = libbreenix::Errno::ENOMEM as i32;
        return core::ptr::null_mut();
    }

    // Initialize the Dir struct
    core::ptr::write_bytes(ptr as *mut u8, 0, dir_size);
    (*ptr).fd = fd;
    (*ptr).buf_len = 0;
    (*ptr).buf_pos = 0;
    (*ptr).eof = false;

    ptr
}

/// readdir_r - reentrant read directory entry
///
/// Reads the next directory entry from the DIR stream into `entry`.
/// On success, stores a pointer to `entry` in `*result`.
/// At end of directory, `*result` is set to null.
#[no_mangle]
pub unsafe extern "C" fn readdir_r(
    dirp: *mut Dir,
    entry: *mut Dirent,
    result: *mut *mut Dirent,
) -> i32 {
    if dirp.is_null() || entry.is_null() || result.is_null() {
        return EINVAL;
    }

    let dir = &mut *dirp;

    // If we've consumed all buffered data, fetch more
    if dir.buf_pos >= dir.buf_len {
        if dir.eof {
            *result = core::ptr::null_mut();
            return 0;
        }

        // Call getdents64 to fill the buffer
        let ret = libbreenix::raw::syscall3(
            libbreenix::syscall::nr::GETDENTS64,
            dir.fd as u64,
            dir.buf.as_mut_ptr() as u64,
            dir.buf.len() as u64,
        ) as i64;

        if ret < 0 {
            *result = core::ptr::null_mut();
            return (-ret) as i32;
        }

        if ret == 0 {
            dir.eof = true;
            *result = core::ptr::null_mut();
            return 0;
        }

        dir.buf_len = ret as usize;
        dir.buf_pos = 0;
    }

    // Parse the next dirent64 from the buffer
    let pos = dir.buf_pos;
    if pos + 19 > dir.buf_len {
        // Not enough data for a header
        *result = core::ptr::null_mut();
        return 0;
    }

    let buf_ptr = dir.buf.as_ptr().add(pos);

    // Read fields from the kernel's linux_dirent64 format:
    // d_ino: u64 at offset 0
    // d_off: i64 at offset 8
    // d_reclen: u16 at offset 16
    // d_type: u8 at offset 18
    // d_name: [u8] at offset 19
    let d_ino = core::ptr::read_unaligned(buf_ptr as *const u64);
    let d_off = core::ptr::read_unaligned(buf_ptr.add(8) as *const i64);
    let d_reclen = core::ptr::read_unaligned(buf_ptr.add(16) as *const u16);
    let d_type = core::ptr::read(buf_ptr.add(18));

    // Copy d_name
    let name_ptr = buf_ptr.add(19);
    let name_max_len = (d_reclen as usize).saturating_sub(19).min(255);

    (*entry).d_ino = d_ino;
    (*entry).d_off = d_off;
    (*entry).d_reclen = d_reclen;
    (*entry).d_type = d_type;

    // Zero the name first, then copy
    core::ptr::write_bytes((*entry).d_name.as_mut_ptr(), 0, 256);
    core::ptr::copy_nonoverlapping(name_ptr, (*entry).d_name.as_mut_ptr(), name_max_len);

    // Advance buffer position by d_reclen
    dir.buf_pos += d_reclen as usize;

    *result = entry;
    0
}

/// closedir - close a directory stream
#[no_mangle]
pub unsafe extern "C" fn closedir(dirp: *mut Dir) -> i32 {
    if dirp.is_null() {
        ERRNO = EINVAL;
        return -1;
    }

    let fd = (*dirp).fd;
    // We can't free sbrk memory, but we can close the fd
    close(fd);
    0
}

/// dirfd - get directory file descriptor from DIR stream
#[no_mangle]
pub unsafe extern "C" fn dirfd(dirp: *mut Dir) -> i32 {
    if dirp.is_null() {
        ERRNO = EINVAL;
        return -1;
    }
    (*dirp).fd
}

/// futimens - change file timestamps with nanosecond precision
#[no_mangle]
pub unsafe extern "C" fn futimens(_fd: i32, _times: *const u8) -> i32 {
    ERRNO = ENOSYS;
    -1
}

/// setgroups - set list of supplementary group IDs
#[no_mangle]
pub unsafe extern "C" fn setgroups(_size: usize, _list: *const u32) -> i32 {
    0 // No-op: single-user system
}

/// getgroups - get list of supplementary group IDs
#[no_mangle]
pub unsafe extern "C" fn getgroups(size: i32, list: *mut u32) -> i32 {
    if size == 0 {
        return 0; // Return number of supplementary group IDs (none)
    }
    if !list.is_null() && size > 0 {
        *list = 0; // root group
        return 1;
    }
    0
}

/// getpwuid - get password entry by UID (stub)
#[no_mangle]
pub unsafe extern "C" fn getpwuid(_uid: u32) -> *mut u8 {
    core::ptr::null_mut()
}

/// getpwuid_r - reentrant version of getpwuid (stub)
#[no_mangle]
pub unsafe extern "C" fn getpwuid_r(
    _uid: u32,
    _pwd: *mut u8,
    _buf: *mut u8,
    _buflen: usize,
    result: *mut *mut u8,
) -> i32 {
    if !result.is_null() {
        *result = core::ptr::null_mut();
    }
    0 // Not found, but not an error
}

// =============================================================================
// Signal/Timer Functions (alarm, setitimer, getitimer, sigsuspend)
// =============================================================================

/// alarm - schedule a SIGALRM signal
///
/// Sets a timer to deliver SIGALRM after `seconds` seconds.
/// Setting seconds to 0 cancels any pending alarm.
///
/// Returns the number of seconds remaining from a previous alarm (0 if none).
#[no_mangle]
pub unsafe extern "C" fn alarm(seconds: u32) -> u32 {
    libbreenix::signal::alarm(seconds)
}

/// Interval timer value for setitimer/getitimer
#[repr(C)]
pub struct CTimeval {
    pub tv_sec: i64,
    pub tv_usec: i64,
}

#[repr(C)]
pub struct CItimerval {
    pub it_interval: CTimeval,
    pub it_value: CTimeval,
}

/// setitimer - set an interval timer
///
/// Returns 0 on success, -1 on error (errno set).
#[no_mangle]
pub unsafe extern "C" fn setitimer(which: i32, new_value: *const CItimerval, old_value: *mut CItimerval) -> i32 {
    // Convert C structs to libbreenix types
    if new_value.is_null() {
        ERRNO = EINVAL;
        return -1;
    }

    let new_val = libbreenix::signal::Itimerval {
        it_interval: libbreenix::signal::Timeval {
            tv_sec: (*new_value).it_interval.tv_sec,
            tv_usec: (*new_value).it_interval.tv_usec,
        },
        it_value: libbreenix::signal::Timeval {
            tv_sec: (*new_value).it_value.tv_sec,
            tv_usec: (*new_value).it_value.tv_usec,
        },
    };

    let mut old_val = libbreenix::signal::Itimerval::default();
    let old_opt = if old_value.is_null() {
        None
    } else {
        Some(&mut old_val)
    };

    match libbreenix::signal::setitimer(which, &new_val, old_opt) {
        Ok(()) => {
            if !old_value.is_null() {
                (*old_value).it_interval.tv_sec = old_val.it_interval.tv_sec;
                (*old_value).it_interval.tv_usec = old_val.it_interval.tv_usec;
                (*old_value).it_value.tv_sec = old_val.it_value.tv_sec;
                (*old_value).it_value.tv_usec = old_val.it_value.tv_usec;
            }
            0
        }
        Err(e) => set_errno_from_error(e),
    }
}

/// getitimer - get the value of an interval timer
///
/// Returns 0 on success, -1 on error (errno set).
#[no_mangle]
pub unsafe extern "C" fn getitimer(which: i32, curr_value: *mut CItimerval) -> i32 {
    if curr_value.is_null() {
        ERRNO = EINVAL;
        return -1;
    }

    let mut val = libbreenix::signal::Itimerval::default();
    match libbreenix::signal::getitimer(which, &mut val) {
        Ok(()) => {
            (*curr_value).it_interval.tv_sec = val.it_interval.tv_sec;
            (*curr_value).it_interval.tv_usec = val.it_interval.tv_usec;
            (*curr_value).it_value.tv_sec = val.it_value.tv_sec;
            (*curr_value).it_value.tv_usec = val.it_value.tv_usec;
            0
        }
        Err(e) => set_errno_from_error(e),
    }
}

/// sigsuspend - wait for a signal, temporarily replacing the signal mask
///
/// Always returns -1 with errno set to EINTR.
#[no_mangle]
pub unsafe extern "C" fn sigsuspend(mask: *const u64) -> i32 {
    if mask.is_null() {
        ERRNO = EINVAL;
        return -1;
    }
    let _ret = libbreenix::signal::sigsuspend(&*mask);
    // sigsuspend always returns -1 with EINTR
    ERRNO = 4; // EINTR
    -1
}

// =============================================================================
// PTY Functions (posix_openpt, grantpt, unlockpt, ptsname_r)
// =============================================================================

/// posix_openpt - open a PTY master device
///
/// Returns a file descriptor on success, -1 on error (errno set).
#[no_mangle]
pub unsafe extern "C" fn posix_openpt(flags: i32) -> i32 {
    result_fd_to_c_int(libbreenix::pty::posix_openpt(flags))
}

/// grantpt - grant access to slave PTY
///
/// Returns 0 on success, -1 on error (errno set).
#[no_mangle]
pub unsafe extern "C" fn grantpt(fd: i32) -> i32 {
    result_unit_to_c_int(libbreenix::pty::grantpt(Fd::from_raw(fd as u64)))
}

/// unlockpt - unlock the slave PTY for opening
///
/// Returns 0 on success, -1 on error (errno set).
#[no_mangle]
pub unsafe extern "C" fn unlockpt(fd: i32) -> i32 {
    result_unit_to_c_int(libbreenix::pty::unlockpt(Fd::from_raw(fd as u64)))
}

/// ptsname_r - get the path to the slave PTY device (reentrant)
///
/// Returns 0 on success, errno on error.
#[no_mangle]
pub unsafe extern "C" fn ptsname_r(fd: i32, buf: *mut u8, buflen: usize) -> i32 {
    if buf.is_null() || buflen == 0 {
        return EINVAL;
    }

    let slice = slice::from_raw_parts_mut(buf, buflen);
    match libbreenix::pty::ptsname(Fd::from_raw(fd as u64), slice) {
        Ok(_len) => 0,
        Err(e) => error_to_errno(&e),
    }
}

// =============================================================================
// Testing Functions (simulate_oom)
// =============================================================================

/// simulate_oom - enable/disable OOM simulation for testing
///
/// Returns 0 on success, -1 on error (errno set).
#[no_mangle]
pub unsafe extern "C" fn simulate_oom(enable: i32) -> i32 {
    result_unit_to_c_int(libbreenix::memory::simulate_oom(enable != 0))
}

// =============================================================================
// Time Functions (sleep_ms)
// =============================================================================

/// sleep_ms - sleep for the specified number of milliseconds
///
/// This is a busy-wait implementation using clock_gettime(CLOCK_MONOTONIC).
#[no_mangle]
pub unsafe extern "C" fn sleep_ms(ms: u64) {
    let _ = libbreenix::time::sleep_ms(ms);
}

// =============================================================================
// DNS Functions (dns_resolve)
// =============================================================================

/// dns_resolve - resolve a hostname to an IPv4 address
///
/// # Arguments
/// * `host` - Hostname string (NOT null-terminated, length given by host_len)
/// * `host_len` - Length of hostname string
/// * `server` - Pointer to 4-byte IPv4 address of DNS server
/// * `result_ip` - Pointer to 4-byte buffer for the resolved IPv4 address
///
/// Returns 0 on success, negative errno on error.
#[no_mangle]
pub unsafe extern "C" fn dns_resolve(
    host: *const u8,
    host_len: usize,
    server: *const u8,
    result_ip: *mut u8,
) -> i32 {
    if host.is_null() || server.is_null() || result_ip.is_null() || host_len == 0 {
        return -(EINVAL);
    }

    let host_bytes = slice::from_raw_parts(host, host_len);
    let hostname = match core::str::from_utf8(host_bytes) {
        Ok(s) => s,
        Err(_) => return -(EINVAL),
    };

    let dns_server = [
        *server,
        *server.add(1),
        *server.add(2),
        *server.add(3),
    ];

    match libbreenix::dns::resolve(hostname, dns_server) {
        Ok(result) => {
            *result_ip = result.addr[0];
            *result_ip.add(1) = result.addr[1];
            *result_ip.add(2) = result.addr[2];
            *result_ip.add(3) = result.addr[3];
            0
        }
        Err(_) => -(5), // EIO
    }
}

// =============================================================================
// Unwind Functions (stubs for backtrace support)
// =============================================================================

/// _Unwind_Backtrace - walk the call stack (stub)
#[no_mangle]
pub unsafe extern "C" fn _Unwind_Backtrace(
    _trace: extern "C" fn(*mut u8, *mut u8) -> i32,
    _trace_argument: *mut u8,
) -> i32 {
    5 // _URC_END_OF_STACK
}

/// _Unwind_GetIP - get instruction pointer from context (stub)
#[no_mangle]
pub unsafe extern "C" fn _Unwind_GetIP(_context: *mut u8) -> usize {
    0
}

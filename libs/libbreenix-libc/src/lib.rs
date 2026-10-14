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
// This crate defines memcpy, strlen, strcat and the rest: LLVM must not assume their C
// semantics inside it, or it rewrites their bodies into calls to themselves (or away).
#![no_builtins]
#![feature(c_variadic)]

mod crt;
mod setjmp;
mod grp;
mod libgen;
mod pwd;
mod unistd;
mod mntent;
mod statfs;
mod dirent;
mod env;
mod fnmatch;
mod getopt;
mod glob;
mod math;
mod stdlib;
mod string;
mod inet;
mod netdb;
mod regex;
mod mqueue;
mod shm;
mod semaphore;
mod sysv;
mod termios;

use libbreenix::types::Fd;
use libbreenix::error::Error;
use core::slice;

/// AT_FDCWD as raw u64 for direct syscall use (-100 as u64)
const AT_FDCWD_RAW: u64 = (-100i32) as u32 as u64;
/// AT_REMOVEDIR flag for unlinkat
#[cfg(target_arch = "aarch64")]
const AT_REMOVEDIR: u64 = 0x200;

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

pub use pthread::__errno_location;
pub use stdlib::exit::exit;
pub use stdlib::malloc::{calloc, free, malloc, realloc};

/// Set errno from a negative syscall return value
///
/// Syscalls return negative errno on error. This helper converts the return
/// value to the positive errno and stores it.
#[inline]
fn set_errno_from_result(result: i64) -> i32 {
    if result < 0 {
        let errno_val = (-result) as i32;
        unsafe {
            *__errno_location() = errno_val;
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
    unsafe { *__errno_location() = error_to_errno(&e); }
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
        *__errno_location() = libbreenix::Errno::EFAULT as i32;
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
        *__errno_location() = libbreenix::Errno::EFAULT as i32;
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
        *__errno_location() = EFAULT;
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
        *__errno_location() = EFAULT;
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
        *__errno_location() = EFAULT;
        return -1;
    }

    let result = {
        #[cfg(target_arch = "x86_64")]
        {
            libbreenix::raw::syscall3(
                libbreenix::syscall::nr::OPEN,
                path as u64,
                flags as u64,
                mode as u64,
            ) as i64
        }
        #[cfg(target_arch = "aarch64")]
        {
            libbreenix::raw::syscall4(
                libbreenix::syscall::nr::OPENAT,
                AT_FDCWD_RAW,
                path as u64,
                flags as u64,
                mode as u64,
            ) as i64
        }
    };
    syscall_result_to_c_int(result)
}

/// openat - open a file relative to a directory fd
#[no_mangle]
pub unsafe extern "C" fn openat(dirfd: i32, path: *const u8, flags: i32, mode: u32) -> i32 {
    if path.is_null() {
        *__errno_location() = EFAULT;
        return -1;
    }

    let result = libbreenix::raw::syscall4(
        libbreenix::syscall::nr::OPENAT,
        dirfd as u64,
        path as u64,
        flags as u64,
        mode as u64,
    ) as i64;
    syscall_result_to_c_int(result)
}

/// fstat - get file status by fd
#[no_mangle]
pub unsafe extern "C" fn fstat(fd: i32, buf: *mut u8) -> i32 {
    if buf.is_null() {
        *__errno_location() = EFAULT;
        return -1;
    }

    let result = libbreenix::raw::syscall2(
        libbreenix::syscall::nr::FSTAT,
        fd as u64,
        buf as u64,
    ) as i64;
    syscall_result_to_c_int(result)
}

/// stat - get file status by path, following the final symlink.
///
/// Uses newfstatat rather than open() + fstat(): stat needs no read
/// permission on the file, and opening a FIFO could block.
#[no_mangle]
pub unsafe extern "C" fn stat(path: *const u8, buf: *mut u8) -> i32 {
    stat_by_path(path, buf, 0)
}

/// Create a FIFO with permissions filtered by the kernel's creation mask.
#[no_mangle]
pub unsafe extern "C" fn mkfifo(path: *const u8, mode: u32) -> i32 {
    mkfifoat(AT_FDCWD_RAW as i32, path, mode)
}

/// Create a FIFO using the Linux mknodat interface (AT_FDCWD selects cwd).
#[no_mangle]
pub unsafe extern "C" fn mkfifoat(dirfd: i32, path: *const u8, mode: u32) -> i32 {
    if path.is_null() {
        *__errno_location() = EFAULT;
        return -1;
    }
    const S_IFIFO: u32 = 0o010000;
    let result = libbreenix::raw::syscall4(
        libbreenix::syscall::nr::MKNODAT,
        dirfd as u64,
        path as u64,
        (S_IFIFO | (mode & 0o7777)) as u64,
        0,
    ) as i64;
    syscall_result_to_c_int(result)
}

unsafe fn stat_by_path(path: *const u8, buf: *mut u8, flags: u64) -> i32 {
    if path.is_null() || buf.is_null() {
        *__errno_location() = EFAULT;
        return -1;
    }
    const AT_FDCWD: i64 = -100;
    let result = libbreenix::raw::syscall4(
        libbreenix::syscall::nr::NEWFSTATAT,
        AT_FDCWD as u64,
        path as u64,
        buf as u64,
        flags,
    ) as i64;
    syscall_result_to_c_int(result)
}

/// lstat - get the final symlink's own status.
#[no_mangle]
pub unsafe extern "C" fn lstat(path: *const u8, buf: *mut u8) -> i32 {
    stat_by_path(path, buf, libbreenix::fs::AT_SYMLINK_NOFOLLOW as u64)
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
        *__errno_location() = EFAULT;
        return -1;
    }

    let result = {
        #[cfg(target_arch = "x86_64")]
        {
            libbreenix::raw::syscall3(
                libbreenix::syscall::nr::READLINK,
                path as u64,
                buf as u64,
                bufsiz as u64,
            ) as i64
        }
        #[cfg(target_arch = "aarch64")]
        {
            libbreenix::raw::syscall4(
                libbreenix::syscall::nr::READLINKAT,
                AT_FDCWD_RAW,
                path as u64,
                buf as u64,
                bufsiz as u64,
            ) as i64
        }
    };
    syscall_result_to_c_ssize(result)
}

/// unlink - remove a file
#[no_mangle]
pub unsafe extern "C" fn unlink(path: *const u8) -> i32 {
    if path.is_null() {
        *__errno_location() = EFAULT;
        return -1;
    }

    let result = {
        #[cfg(target_arch = "x86_64")]
        {
            libbreenix::raw::syscall1(
                libbreenix::syscall::nr::UNLINK,
                path as u64,
            ) as i64
        }
        #[cfg(target_arch = "aarch64")]
        {
            libbreenix::raw::syscall3(
                libbreenix::syscall::nr::UNLINKAT,
                AT_FDCWD_RAW,
                path as u64,
                0, // flags=0 means unlink (not rmdir)
            ) as i64
        }
    };
    syscall_result_to_c_int(result)
}

/// rename - rename a file
#[no_mangle]
pub unsafe extern "C" fn rename(oldpath: *const u8, newpath: *const u8) -> i32 {
    if oldpath.is_null() || newpath.is_null() {
        *__errno_location() = EFAULT;
        return -1;
    }

    let result = {
        #[cfg(target_arch = "x86_64")]
        {
            libbreenix::raw::syscall2(
                libbreenix::syscall::nr::RENAME,
                oldpath as u64,
                newpath as u64,
            ) as i64
        }
        #[cfg(target_arch = "aarch64")]
        {
            libbreenix::raw::syscall4(
                libbreenix::syscall::nr::RENAMEAT,
                AT_FDCWD_RAW,
                oldpath as u64,
                AT_FDCWD_RAW,
                newpath as u64,
            ) as i64
        }
    };
    syscall_result_to_c_int(result)
}

/// mkdir - create a directory
#[no_mangle]
pub unsafe extern "C" fn mkdir(path: *const u8, mode: u32) -> i32 {
    if path.is_null() {
        *__errno_location() = EFAULT;
        return -1;
    }

    let result = {
        #[cfg(target_arch = "x86_64")]
        {
            libbreenix::raw::syscall2(
                libbreenix::syscall::nr::MKDIR,
                path as u64,
                mode as u64,
            ) as i64
        }
        #[cfg(target_arch = "aarch64")]
        {
            libbreenix::raw::syscall3(
                libbreenix::syscall::nr::MKDIRAT,
                AT_FDCWD_RAW,
                path as u64,
                mode as u64,
            ) as i64
        }
    };
    syscall_result_to_c_int(result)
}

/// rmdir - remove a directory
#[no_mangle]
pub unsafe extern "C" fn rmdir(path: *const u8) -> i32 {
    if path.is_null() {
        *__errno_location() = EFAULT;
        return -1;
    }

    let result = {
        #[cfg(target_arch = "x86_64")]
        {
            libbreenix::raw::syscall1(
                libbreenix::syscall::nr::RMDIR,
                path as u64,
            ) as i64
        }
        #[cfg(target_arch = "aarch64")]
        {
            libbreenix::raw::syscall3(
                libbreenix::syscall::nr::UNLINKAT,
                AT_FDCWD_RAW,
                path as u64,
                AT_REMOVEDIR,
            ) as i64
        }
    };
    syscall_result_to_c_int(result)
}

/// link - create a hard link
#[no_mangle]
pub unsafe extern "C" fn link(oldpath: *const u8, newpath: *const u8) -> i32 {
    if oldpath.is_null() || newpath.is_null() {
        *__errno_location() = EFAULT;
        return -1;
    }

    let result = {
        #[cfg(target_arch = "x86_64")]
        {
            libbreenix::raw::syscall2(
                libbreenix::syscall::nr::LINK,
                oldpath as u64,
                newpath as u64,
            ) as i64
        }
        #[cfg(target_arch = "aarch64")]
        {
            libbreenix::raw::syscall5(
                libbreenix::syscall::nr::LINKAT,
                AT_FDCWD_RAW,
                oldpath as u64,
                AT_FDCWD_RAW,
                newpath as u64,
                0, // flags
            ) as i64
        }
    };
    syscall_result_to_c_int(result)
}

/// linkat - create a hard link relative to directory file descriptors
#[no_mangle]
pub unsafe extern "C" fn linkat(
    olddirfd: i32,
    oldpath: *const u8,
    newdirfd: i32,
    newpath: *const u8,
    flags: i32,
) -> i32 {
    if oldpath.is_null() || newpath.is_null() {
        *__errno_location() = EFAULT;
        return -1;
    }

    let result = libbreenix::raw::syscall5(
        libbreenix::syscall::nr::LINKAT,
        olddirfd as u64,
        oldpath as u64,
        newdirfd as u64,
        newpath as u64,
        flags as u64,
    ) as i64;
    syscall_result_to_c_int(result)
}

/// symlink - create a symbolic link
#[no_mangle]
pub unsafe extern "C" fn symlink(target: *const u8, linkpath: *const u8) -> i32 {
    if target.is_null() || linkpath.is_null() {
        *__errno_location() = EFAULT;
        return -1;
    }

    let result = {
        #[cfg(target_arch = "x86_64")]
        {
            libbreenix::raw::syscall2(
                libbreenix::syscall::nr::SYMLINK,
                target as u64,
                linkpath as u64,
            ) as i64
        }
        #[cfg(target_arch = "aarch64")]
        {
            libbreenix::raw::syscall3(
                libbreenix::syscall::nr::SYMLINKAT,
                target as u64,
                AT_FDCWD_RAW,
                linkpath as u64,
            ) as i64
        }
    };
    syscall_result_to_c_int(result)
}

/// access - check user's permissions for a file
#[no_mangle]
pub unsafe extern "C" fn access(path: *const u8, mode: i32) -> i32 {
    if path.is_null() {
        *__errno_location() = EFAULT;
        return -1;
    }

    let result = {
        #[cfg(target_arch = "x86_64")]
        {
            libbreenix::raw::syscall2(
                libbreenix::syscall::nr::ACCESS,
                path as u64,
                mode as u64,
            ) as i64
        }
        #[cfg(target_arch = "aarch64")]
        {
            libbreenix::raw::syscall4(
                libbreenix::syscall::nr::FACCESSAT,
                AT_FDCWD_RAW,
                path as u64,
                mode as u64,
                0, // flags
            ) as i64
        }
    };
    syscall_result_to_c_int(result)
}

/// getcwd - get current working directory
///
/// Returns buf on success, NULL on error (sets errno).
#[no_mangle]
pub unsafe extern "C" fn getcwd(buf: *mut u8, size: usize) -> *mut u8 {
    if buf.is_null() || size == 0 {
        *__errno_location() = EINVAL;
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
        *__errno_location() = EFAULT;
        return -1;
    }

    let result = libbreenix::raw::syscall1(
        libbreenix::syscall::nr::CHDIR,
        path as u64,
    ) as i64;
    syscall_result_to_c_int(result)
}

/// fchdir - change working directory to an open directory
#[no_mangle]
pub unsafe extern "C" fn fchdir(fd: i32) -> i32 {
    let result = libbreenix::raw::syscall1(
        libbreenix::syscall::nr::FCHDIR,
        fd as u64,
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
        *__errno_location() = EFAULT;
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
pub unsafe extern "C" fn ftruncate(fd: i32, length: i64) -> i32 {
    let result =
        libbreenix::raw::syscall2(libbreenix::syscall::nr::FTRUNCATE, fd as u64, length as u64)
            as i64;
    syscall_result_to_c_int(result)
}

/// truncate - resize a file by path.
#[no_mangle]
pub unsafe extern "C" fn truncate(path: *const u8, length: i64) -> i32 {
    let result = libbreenix::raw::syscall2(
        libbreenix::syscall::nr::TRUNCATE,
        path as u64,
        length as u64,
    ) as i64;
    syscall_result_to_c_int(result)
}

/// truncate64 - same as truncate on 64-bit.
#[no_mangle]
pub unsafe extern "C" fn truncate64(path: *const u8, length: i64) -> i32 {
    truncate(path, length)
}

/// ftruncate64 - same as ftruncate on 64-bit
#[no_mangle]
pub unsafe extern "C" fn ftruncate64(fd: i32, length: i64) -> i32 {
    ftruncate(fd, length)
}

/// fsync - synchronize file state with storage
#[no_mangle]
pub extern "C" fn fsync(fd: i32) -> i32 {
    let result =
        unsafe { libbreenix::raw::syscall1(libbreenix::syscall::nr::FSYNC, fd as u64) } as i64;
    syscall_result_to_c_int(result)
}

/// fdatasync - synchronize file data with storage
#[no_mangle]
pub extern "C" fn fdatasync(fd: i32) -> i32 {
    let result =
        unsafe { libbreenix::raw::syscall1(libbreenix::syscall::nr::FDATASYNC, fd as u64) } as i64;
    syscall_result_to_c_int(result)
}

/// fchmod - change file mode bits (by fd)
#[no_mangle]
pub unsafe extern "C" fn fchmod(fd: i32, mode: u32) -> i32 {
    syscall_result_to_c_int(libbreenix::raw::syscall2(libbreenix::syscall::nr::FCHMOD, fd as u64, mode as u64) as i64)
}

/// fchown - change file owner/group (by fd)
#[no_mangle]
pub unsafe extern "C" fn fchown(fd: i32, owner: u32, group: u32) -> i32 {
    syscall_result_to_c_int(libbreenix::raw::syscall3(libbreenix::syscall::nr::FCHOWN, fd as u64, owner as u64, group as u64) as i64)
}

/// chmod follows the final symlink, using the shared *at ABI on both architectures.
#[no_mangle]
pub unsafe extern "C" fn chmod(path: *const u8, mode: u32) -> i32 {
    fchmodat(-100, path, mode, 0)
}

/// Linux's fchmodat syscall has no flags; unsupported libc flags fail explicitly.
#[no_mangle]
pub unsafe extern "C" fn fchmodat(fd: i32, path: *const u8, mode: u32, flags: i32) -> i32 {
    if flags != 0 {
        *__errno_location() = if flags & !0x100 != 0 { EINVAL } else { 95 };
        return -1;
    }
    syscall_result_to_c_int(libbreenix::raw::syscall3(libbreenix::syscall::nr::FCHMODAT, fd as u64, path as u64, mode as u64) as i64)
}

/// chown follows a final symlink; lchown changes the symlink's own ownership.
#[no_mangle]
pub unsafe extern "C" fn chown(path: *const u8, owner: u32, group: u32) -> i32 {
    fchownat(-100, path, owner, group, 0)
}

#[no_mangle]
pub unsafe extern "C" fn lchown(path: *const u8, owner: u32, group: u32) -> i32 {
    fchownat(-100, path, owner, group, 0x100)
}

#[no_mangle]
pub unsafe extern "C" fn fchownat(fd: i32, path: *const u8, owner: u32, group: u32, flags: i32) -> i32 {
    syscall_result_to_c_int(libbreenix::raw::syscall5(libbreenix::syscall::nr::FCHOWNAT, fd as u64, path as u64, owner as u64, group as u64, flags as u64) as i64)
}

#[no_mangle]
pub unsafe extern "C" fn umask(mask: u32) -> u32 {
    libbreenix::raw::syscall1(libbreenix::syscall::nr::UMASK, mask as u64) as u32
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

/// Terminate the calling process immediately.
#[no_mangle]
pub extern "C" fn _exit(status: i32) -> ! {
    unsafe { libbreenix::raw::syscall1(libbreenix::syscall::nr::EXIT_GROUP, status as u64); }
    loop { core::hint::spin_loop(); }
}

/// Terminate all threads in the current process group.
#[no_mangle]
pub extern "C" fn exit_group(status: i32) -> ! {
    unsafe { libbreenix::raw::syscall1(libbreenix::syscall::nr::EXIT_GROUP, status as u64); }
    loop { core::hint::spin_loop(); }
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
        Ok(libbreenix::process::ForkResult::Child) => {
            unsafe { pthread::__breenix_after_fork(); }
            0
        },
        Ok(libbreenix::process::ForkResult::Parent(pid)) => pid.raw() as i32,
        Err(e) => set_errno_from_error(e),
    }
}

/// execve - execute a program
///
/// Passes path, argv and envp to the kernel.
#[no_mangle]
pub unsafe extern "C" fn execve(
    path: *const u8,
    argv: *const *const u8,
    envp: *const *const u8,
) -> i32 {
    if path.is_null() {
        *__errno_location() = EFAULT;
        return -1;
    }

    // The message queue notification helper is stopped first: exec fails
    // while another thread shares the address space.
    let stopped = mqueue::before_exec();
    let result = libbreenix::raw::syscall3(
        libbreenix::syscall::nr::EXEC,
        path as u64,
        argv as u64,
        envp as u64,
    ) as i64;
    // execve should not return on success
    mqueue::after_failed_exec(stopped);
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

/// raise - send a signal to the calling thread
#[no_mangle]
pub extern "C" fn raise(sig: i32) -> i32 {
    syscall_result_to_c_int(pthread::signal_current(sig))
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
        libbreenix::syscall::nr::MMAP,
        addr as u64,
        len as u64,
        prot as u64,
        flags as u64,
        fd as u64,
        offset as u64,
    );

    let result_signed = result as i64;
    if result_signed < 0 && result_signed >= -4096 {
        *__errno_location() = (-result_signed) as i32;
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
        *__errno_location() = libbreenix::Errno::ENOMEM as i32;
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
        *__errno_location() = libbreenix::Errno::EINVAL as i32;
        return usize::MAX as *mut u8;
    }

    let result = libbreenix::memory::sbrk(increment as usize);

    if result.is_null() {
        *__errno_location() = libbreenix::Errno::ENOMEM as i32;
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
// String/Utility Functions
// =============================================================================

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
        *__errno_location() = (-ret) as i32;
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
    const _SC_ARG_MAX: i32 = 0;
    const _SC_PAGESIZE: i32 = 30;
    const _SC_NPROCESSORS_ONLN: i32 = 84;
    const _SC_NPROCESSORS_CONF: i32 = 83;
    const _SC_GETPW_R_SIZE_MAX: i32 = 70;
    const _SC_GETGR_R_SIZE_MAX: i32 = 69;

    match name {
        // The kernel reserves a quarter of its fixed 2 MiB stack cap for exec.
        _SC_ARG_MAX => 512 * 1024,
        _SC_PAGESIZE => 4096,
        28 => 32768, // _SC_MQ_PRIO_MAX
        33 => 0x7fff_ffff, // _SC_SEM_VALUE_MAX
        _SC_NPROCESSORS_ONLN | _SC_NPROCESSORS_CONF => 1,
        _SC_GETPW_R_SIZE_MAX | _SC_GETGR_R_SIZE_MAX => 1024,
        67 | 77 | 78 | 133 | 153 | 164 => 200809, // threads, stack attributes, barriers, rwlocks, timeouts
        73 => 4, // PTHREAD_DESTRUCTOR_ITERATIONS
        74 => -1, // keys grow until allocation fails; no fixed limit
        79 => -1, // priority-ordered mutex/condition wakeups are not provided
        80 | 81 => 200809, // PTHREAD_PRIO_INHERIT and PTHREAD_PRIO_PROTECT mutexes
        75 => 16384, // PTHREAD_STACK_MIN
        _ => -1,
    }
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
// Resource Limits
// =============================================================================

/// Linux rlimit structure
#[repr(C)]
pub struct rlimit {
    pub rlim_cur: u64,
    pub rlim_max: u64,
}

/// Linux utsname structure
#[repr(C)]
pub struct utsname {
    pub sysname: [u8; 65],
    pub nodename: [u8; 65],
    pub release: [u8; 65],
    pub version: [u8; 65],
    pub machine: [u8; 65],
    pub domainname: [u8; 65],
}

/// getrlimit - get resource limits
#[no_mangle]
pub unsafe extern "C" fn getrlimit(resource: i32, rlim: *mut rlimit) -> i32 {
    #[cfg(target_arch = "x86_64")]
    let ret = libbreenix::syscall::raw::syscall2(
        libbreenix::syscall::nr::GETRLIMIT,
        resource as u64,
        rlim as u64,
    ) as i64;
    #[cfg(target_arch = "aarch64")]
    let ret = libbreenix::syscall::raw::syscall4(
        libbreenix::syscall::nr::PRLIMIT64,
        0u64,  // pid=0 means current process
        resource as u64,
        0u64,  // new_rlim = NULL
        rlim as u64,
    ) as i64;
    if ret < 0 { set_errno_from_result(ret); -1 } else { 0 }
}

/// uname - get system identification
#[no_mangle]
pub unsafe extern "C" fn uname(buf: *mut utsname) -> i32 {
    let ret = libbreenix::syscall::raw::syscall1(
        libbreenix::syscall::nr::UNAME,
        buf as u64,
    ) as i64;
    if ret < 0 { set_errno_from_result(ret); -1 } else { 0 }
}

// =============================================================================
// epoll
// =============================================================================

/// epoll_event structure (matches Linux ABI)
#[repr(C)]
#[cfg_attr(target_arch = "x86_64", repr(packed))]
pub struct epoll_event {
    pub events: u32,
    pub data: u64,
}

/// epoll_create - create an epoll instance (legacy, calls epoll_create1)
#[no_mangle]
pub unsafe extern "C" fn epoll_create(_size: i32) -> i32 {
    epoll_create1(0)
}

/// epoll_create1 - create an epoll instance
#[no_mangle]
pub unsafe extern "C" fn epoll_create1(flags: i32) -> i32 {
    let ret = libbreenix::syscall::raw::syscall1(
        libbreenix::syscall::nr::EPOLL_CREATE1,
        flags as u64,
    ) as i64;
    if ret < 0 { set_errno_from_result(ret); -1 } else { ret as i32 }
}

/// epoll_ctl - control interface for an epoll instance
#[no_mangle]
pub unsafe extern "C" fn epoll_ctl(
    epfd: i32,
    op: i32,
    fd: i32,
    event: *mut epoll_event,
) -> i32 {
    let ret = libbreenix::syscall::raw::syscall4(
        libbreenix::syscall::nr::EPOLL_CTL,
        epfd as u64,
        op as u64,
        fd as u64,
        event as u64,
    ) as i64;
    if ret < 0 { set_errno_from_result(ret); -1 } else { 0 }
}

/// epoll_wait - wait for events on an epoll instance
#[no_mangle]
pub unsafe extern "C" fn epoll_wait(
    epfd: i32,
    events: *mut epoll_event,
    maxevents: i32,
    timeout: i32,
) -> i32 {
    epoll_pwait(epfd, events, maxevents, timeout, core::ptr::null())
}

/// epoll_pwait - wait for events on an epoll instance with signal mask
#[no_mangle]
pub unsafe extern "C" fn epoll_pwait(
    epfd: i32,
    events: *mut epoll_event,
    maxevents: i32,
    timeout: i32,
    sigmask: *const u8,
) -> i32 {
    let ret = libbreenix::syscall::raw::syscall6(
        libbreenix::syscall::nr::EPOLL_PWAIT,
        epfd as u64,
        events as u64,
        maxevents as u64,
        timeout as u64,
        sigmask as u64,
        8u64, // sigsetsize
    ) as i64;
    if ret < 0 { set_errno_from_result(ret); -1 } else { ret as i32 }
}

// =============================================================================
// Signal Handling
// =============================================================================

/// signal - set a signal's disposition with BSD semantics (SA_RESTART), as
/// glibc and musl do. Returns the previous handler, or SIG_ERR with errno set.
///
/// Rust's std runtime calls `signal(SIGPIPE, SIG_IGN)` at startup so that a
/// write to a broken pipe returns EPIPE instead of killing the program.
#[no_mangle]
pub extern "C" fn signal(signum: i32, handler: usize) -> usize {
    use libbreenix::signal::{Sigaction, SA_RESTART, SA_RESTORER, SIG_IGN};
    const SIG_ERR: usize = usize::MAX;

    let mut action = Sigaction {
        handler: handler as u64,
        mask: 0,
        flags: SA_RESTART,
        restorer: 0,
    };
    if action.handler > SIG_IGN {
        action.flags |= SA_RESTORER;
        action.restorer = libbreenix::signal::__restore_rt as u64;
    }
    let mut previous = Sigaction::default();
    match libbreenix::signal::sigaction(signum, Some(&action), Some(&mut previous)) {
        Ok(()) => previous.handler as usize,
        Err(e) => {
            set_errno_from_error(e);
            SIG_ERR
        }
    }
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
    // The kernel Sigaction struct layout (libbreenix::signal::Sigaction),
    // Linux's for rt_sigaction:
    //   offset 0: handler - 8 bytes (u64)
    //   offset 8: flags - 8 bytes (u64)
    //   offset 16: restorer - 8 bytes (u64)
    //   offset 24: mask - 8 bytes (u64)

    // A handler (not SIG_DFL or SIG_IGN) returns through __restore_rt, which calls
    // rt_sigreturn, unless the caller supplied its own restorer: the C struct's
    // sa_restorer is null in ordinary programs, and a handler must not depend on the
    // kernel placing a return trampoline on an executable stack.
    let mut kernel_act = libbreenix::signal::Sigaction { handler: 0, mask: 0, flags: 0, restorer: 0 };
    let act_ptr = if act.is_null() {
        core::ptr::null()
    } else {
        // Convert C sigaction to kernel Sigaction
        let c_handler = *(act as *const u64);
        let c_mask = *(act.add(8) as *const u64);
        let c_flags = *(act.add(16) as *const i32);
        let c_restorer = *(act.add(24) as *const u64);

        kernel_act.handler = c_handler;
        kernel_act.mask = c_mask;
        kernel_act.flags = c_flags as u32 as u64;
        kernel_act.restorer = c_restorer;
        let has_restorer = kernel_act.flags & libbreenix::signal::SA_RESTORER != 0 && c_restorer != 0;
        if c_handler > libbreenix::signal::SIG_IGN && !has_restorer {
            kernel_act.flags |= libbreenix::signal::SA_RESTORER;
            kernel_act.restorer = libbreenix::signal::__restore_rt as u64;
        }
        &raw const kernel_act
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
        *__errno_location() = EFAULT;
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
        *__errno_location() = (-ret) as i32;
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
        *__errno_location() = EFAULT;
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

/// accept4 - accept a connection, with SOCK_NONBLOCK and SOCK_CLOEXEC
/// applied to the new descriptor
#[no_mangle]
pub unsafe extern "C" fn accept4(
    sockfd: i32,
    addr: *mut u8,
    addrlen: *mut u32,
    flags: i32,
) -> i32 {
    let result = libbreenix::raw::syscall4(
        libbreenix::syscall::nr::ACCEPT4,
        sockfd as u64,
        addr as u64,
        addrlen as u64,
        flags as u32 as u64,
    ) as i64;
    syscall_result_to_c_int(result)
}

/// connect - connect a socket to an address
#[no_mangle]
pub unsafe extern "C" fn connect(sockfd: i32, addr: *const u8, addrlen: u32) -> i32 {
    if addr.is_null() {
        *__errno_location() = EFAULT;
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

/// sendmsg - send a message, gathered from msg's iovecs, with its address
/// and control messages
#[no_mangle]
pub unsafe extern "C" fn sendmsg(sockfd: i32, msg: *const u8, flags: i32) -> isize {
    let result = libbreenix::raw::syscall3(
        libbreenix::syscall::nr::SENDMSG,
        sockfd as u64,
        msg as u64,
        flags as u32 as u64,
    ) as i64;
    syscall_result_to_c_ssize(result)
}

/// recvmsg - receive a message, scattered over msg's iovecs, with its source
/// address, control messages and flags
#[no_mangle]
pub unsafe extern "C" fn recvmsg(sockfd: i32, msg: *mut u8, flags: i32) -> isize {
    let result = libbreenix::raw::syscall3(
        libbreenix::syscall::nr::RECVMSG,
        sockfd as u64,
        msg as u64,
        flags as u32 as u64,
    ) as i64;
    syscall_result_to_c_ssize(result)
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
        *__errno_location() = EFAULT;
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
    let result = {
        #[cfg(target_arch = "x86_64")]
        {
            libbreenix::raw::syscall5(
                libbreenix::syscall::nr::SELECT,
                nfds as u64,
                readfds as u64,
                writefds as u64,
                exceptfds as u64,
                timeout as u64,
            ) as i64
        }
        #[cfg(target_arch = "aarch64")]
        {
            let ts = if timeout.is_null() {
                None
            } else {
                let tv = core::ptr::read_unaligned(timeout as *const [i64; 2]);
                if tv[0] < 0 || tv[1] < 0 || tv[1] >= 1_000_000 {
                    *__errno_location() = EINVAL;
                    return -1;
                }
                Some([tv[0], tv[1] * 1000])
            };
            libbreenix::raw::syscall6(
                libbreenix::syscall::nr::PSELECT6,
                nfds as u64,
                readfds as u64,
                writefds as u64,
                exceptfds as u64,
                ts.as_ref().map_or(0, |t| t.as_ptr() as u64),
                0, // NULL sigmask
            ) as i64
        }
    };
    syscall_result_to_c_int(result)
}

// =============================================================================
// Poll
// =============================================================================

/// poll - wait for events on file descriptors
#[no_mangle]
pub unsafe extern "C" fn poll(fds: *mut u8, nfds: u64, timeout: i32) -> i32 {
    let result = {
        #[cfg(target_arch = "x86_64")]
        {
            libbreenix::raw::syscall3(
                libbreenix::syscall::nr::POLL,
                fds as u64,
                nfds,
                timeout as u64,
            ) as i64
        }
        #[cfg(target_arch = "aarch64")]
        {
            if timeout < 0 {
                libbreenix::raw::syscall5(
                    libbreenix::syscall::nr::PPOLL,
                    fds as u64,
                    nfds,
                    0, // NULL timespec = infinite
                    0, // NULL sigmask
                    0, // sigsetsize
                ) as i64
            } else {
                let ts = libbreenix::types::Timespec {
                    tv_sec: (timeout / 1000) as i64,
                    tv_nsec: ((timeout % 1000) as i64) * 1_000_000,
                };
                libbreenix::raw::syscall5(
                    libbreenix::syscall::nr::PPOLL,
                    fds as u64,
                    nfds,
                    &ts as *const libbreenix::types::Timespec as u64,
                    0, // NULL sigmask
                    0, // sigsetsize
                ) as i64
            }
        }
    };
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
    let sys_getrandom = libbreenix::syscall::nr::GETRANDOM as i64;

    let result = match num {
        n if n == sys_getrandom => {
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
    };
    if result < 0 { set_errno_from_result(result); -1 } else { result }
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

mod pthread;

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

/// setgroups - set list of supplementary group IDs
#[no_mangle]
pub unsafe extern "C" fn setgroups(size: usize, list: *const u32) -> i32 {
    syscall_result_to_c_int(libbreenix::raw::syscall2(libbreenix::syscall::nr::SETGROUPS, size as u64, list as u64) as i64)
}

/// getgroups - return the process's supplementary group list
#[no_mangle]
pub unsafe extern "C" fn getgroups(size: i32, list: *mut u32) -> i32 {
    syscall_result_to_c_int(libbreenix::raw::syscall2(libbreenix::syscall::nr::GETGROUPS, size as u64, list as u64) as i64)
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
        *__errno_location() = EINVAL;
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
        *__errno_location() = EINVAL;
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
        *__errno_location() = EINVAL;
        return -1;
    }
    let _ret = libbreenix::signal::sigsuspend(&*mask);
    // sigsuspend always returns -1 with EINTR
    *__errno_location() = 4; // EINTR
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

/// grantpt and unlockpt fail with EINVAL when `fd` is open but is not a
/// master (POSIX); the kernel reports that case as ENOTTY.
fn master_result(result: Result<(), Error>) -> i32 {
    match result {
        Err(Error::Os(libbreenix::errno::Errno::ENOTTY)) => {
            set_errno_from_error(Error::Os(libbreenix::errno::Errno::EINVAL))
        }
        result => result_unit_to_c_int(result),
    }
}

/// grantpt - grant access to slave PTY
///
/// Returns 0 on success, -1 on error (errno set).
#[no_mangle]
pub unsafe extern "C" fn grantpt(fd: i32) -> i32 {
    master_result(libbreenix::pty::grantpt(Fd::from_raw(fd as u64)))
}

/// unlockpt - unlock the slave PTY for opening
///
/// Returns 0 on success, -1 on error (errno set).
#[no_mangle]
pub unsafe extern "C" fn unlockpt(fd: i32) -> i32 {
    master_result(libbreenix::pty::unlockpt(Fd::from_raw(fd as u64)))
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

//! POSIX.1-2024 file and mapping interfaces for the hosted compiler.

use core::ffi::c_void;
use libbreenix::{raw, syscall::nr};

use crate::{syscall_result_to_c_int, syscall_result_to_c_ssize};
use crate::Iovec;

const FACCESSAT2: u64 = 439;
#[cfg(target_arch = "x86_64")]
const FADVISE: u64 = 221;
#[cfg(target_arch = "aarch64")]
const FADVISE: u64 = 223;
#[cfg(target_arch = "x86_64")]
const FALLOCATE: u64 = 285;
#[cfg(target_arch = "aarch64")]
const FALLOCATE: u64 = 47;
// Breenix's shared syscall dispatch, kernel/src/syscall/mod.rs.
#[cfg(target_arch = "x86_64")]
const MLOCK: u64 = 149;
#[cfg(target_arch = "aarch64")]
const MLOCK: u64 = 228;
#[cfg(target_arch = "x86_64")]
const MSYNC: u64 = 26;
#[cfg(target_arch = "aarch64")]
const MSYNC: u64 = 227;
#[cfg(target_arch = "x86_64")]
const PREADV: u64 = 295;
#[cfg(target_arch = "aarch64")]
const PREADV: u64 = 69;
#[cfg(target_arch = "x86_64")]
const PWRITEV: u64 = 296;
#[cfg(target_arch = "aarch64")]
const PWRITEV: u64 = 70;

/// POSIX.1-2024 getsid(): return the specified process's session ID.
#[cfg_attr(not(test), no_mangle)]
pub extern "C" fn getsid(pid: i32) -> i32 {
    // SAFETY: the syscall takes a process ID and interprets 0 as the caller.
    let result = unsafe { raw::syscall1(nr::GETSID, pid as i64 as u64) };
    syscall_result_to_c_int(result as i64)
}

/// POSIX.1-2024 mlock(): make the pages covering the byte range resident.
#[cfg_attr(not(test), no_mangle)]
pub unsafe extern "C" fn mlock(addr: *const c_void, len: usize) -> i32 {
    // SAFETY: the kernel validates the range and applies its residency policy.
    let result = unsafe { raw::syscall2(MLOCK, addr as u64, len as u64) };
    syscall_result_to_c_int(result as i64)
}

/// madvise(2): apply the selected advice to the given virtual address range.
#[cfg_attr(not(test), no_mangle)]
pub unsafe extern "C" fn madvise(addr: *mut c_void, len: usize, advice: i32) -> i32 {
    // SAFETY: the kernel validates the address range and advice.
    let result = unsafe { raw::syscall3(nr::MADVISE, addr as u64, len as u64, advice as u64) };
    syscall_result_to_c_int(result as i64)
}

#[cfg_attr(not(test), no_mangle)]
pub unsafe extern "C" fn dup3(old: i32, new: i32, flags: i32) -> i32 {
    // SAFETY: the syscall takes descriptor numbers and a flags word.
    let result = unsafe { raw::syscall3(nr::DUP3, old as u64, new as u64, flags as u64) };
    syscall_result_to_c_int(result as i64)
}

#[cfg_attr(not(test), no_mangle)]
pub unsafe extern "C" fn faccessat(dir: i32, path: *const u8, mode: i32, flags: i32) -> i32 {
    // SAFETY: the kernel checks the caller's pathname and selects credentials from flags.
    let result = unsafe {
        raw::syscall4(
            FACCESSAT2,
            dir as u64,
            path as u64,
            mode as u64,
            flags as u64,
        )
    };
    syscall_result_to_c_int(result as i64)
}

#[cfg_attr(not(test), no_mangle)]
pub unsafe extern "C" fn fstatat(dir: i32, path: *const u8, status: *mut u8, flags: i32) -> i32 {
    const AT_SYMLINK_NOFOLLOW: i32 = 0x100;
    if flags & !AT_SYMLINK_NOFOLLOW != 0 {
        return syscall_result_to_c_int(-i64::from(crate::EINVAL));
    }
    // SAFETY: the kernel validates the pathname and writes its native struct stat to status.
    let result = unsafe {
        raw::syscall4(
            nr::NEWFSTATAT,
            dir as u64,
            path as u64,
            status as u64,
            flags as u64,
        )
    };
    syscall_result_to_c_int(result as i64)
}

#[cfg_attr(not(test), no_mangle)]
pub unsafe extern "C" fn mkdirat(dir: i32, path: *const u8, mode: u32) -> i32 {
    // SAFETY: the kernel checks the pathname and applies the process creation mask.
    let result = unsafe { raw::syscall3(nr::MKDIRAT, dir as u64, path as u64, mode as u64) };
    syscall_result_to_c_int(result as i64)
}

#[cfg_attr(not(test), no_mangle)]
pub unsafe extern "C" fn msync(addr: *mut u8, len: usize, flags: i32) -> i32 {
    // SAFETY: the kernel validates the supplied virtual address range.
    let result = unsafe { raw::syscall3(MSYNC, addr as u64, len as u64, flags as u64) };
    syscall_result_to_c_int(result as i64)
}

#[cfg_attr(not(test), no_mangle)]
pub unsafe extern "C" fn posix_fadvise(fd: i32, offset: i64, len: i64, advice: i32) -> i32 {
    // SAFETY: the syscall receives only scalar arguments.
    let result =
        unsafe { raw::syscall4(FADVISE, fd as u64, offset as u64, len as u64, advice as u64) };
    -(result as i64) as i32
}

#[cfg_attr(not(test), no_mangle)]
pub unsafe extern "C" fn posix_fallocate(fd: i32, offset: i64, len: i64) -> i32 {
    // SAFETY: the syscall receives scalar values and selects the allocating mode.
    let result = unsafe { raw::syscall4(FALLOCATE, fd as u64, 0, offset as u64, len as u64) };
    -(result as i64) as i32
}

// Austin Group issue 1832 and the POSIX.1-2024 positional and vectored I/O
// contracts. Each operation is a cancellation point.
#[cfg_attr(not(test), no_mangle)]
pub unsafe extern "C" fn preadv(fd: i32, iov: *const Iovec, count: i32, offset: i64) -> isize {
    // SAFETY: the kernel checks the vector and each user buffer.
    let result = unsafe {
        crate::pthread::cancel_syscall(PREADV, [fd as u64, iov as u64, count as u64, offset as u64, 0, 0])
    };
    syscall_result_to_c_ssize(result)
}

#[cfg_attr(not(test), no_mangle)]
pub unsafe extern "C" fn pwritev(fd: i32, iov: *const Iovec, count: i32, offset: i64) -> isize {
    // SAFETY: the kernel checks the vector and each user buffer.
    let result = unsafe {
        crate::pthread::cancel_syscall(PWRITEV, [fd as u64, iov as u64, count as u64, offset as u64, 0, 0])
    };
    syscall_result_to_c_ssize(result)
}

#[cfg_attr(not(test), no_mangle)]
pub unsafe extern "C" fn readlinkat(dir: i32, path: *const u8, buf: *mut u8, len: usize) -> isize {
    // SAFETY: the kernel validates both pointers and bounds the copy to len.
    let result = unsafe {
        raw::syscall4(
            nr::READLINKAT,
            dir as u64,
            path as u64,
            buf as u64,
            len as u64,
        )
    };
    syscall_result_to_c_ssize(result as i64)
}

#[cfg_attr(not(test), no_mangle)]
pub unsafe extern "C" fn symlinkat(target: *const u8, dir: i32, link: *const u8) -> i32 {
    // SAFETY: the kernel validates both C strings and resolves link relative to dir.
    let result = unsafe { raw::syscall3(nr::SYMLINKAT, target as u64, dir as u64, link as u64) };
    syscall_result_to_c_int(result as i64)
}

#[cfg_attr(not(test), no_mangle)]
pub extern "C" fn sync() {
    // SAFETY: sync passes no arguments or user pointers and has no C return value.
    unsafe { raw::syscall0(nr::SYNC) };
}

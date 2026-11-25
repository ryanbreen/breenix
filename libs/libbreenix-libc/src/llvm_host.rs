//! POSIX.1-2024 C interfaces needed by the hosted LLVM tools.

use libbreenix::{raw, syscall::nr};

#[no_mangle]
pub unsafe extern "C" fn getrusage(who: i32, usage: *mut u8) -> i32 {
    // Getrusage dispatch in kernel/src/syscall/mod.rs:400 and :565.
    // The kernel fills struct rusage and validates the user pointer.
    #[cfg(target_arch = "x86_64")]
    const GETRUSAGE: u64 = 98;
    #[cfg(target_arch = "aarch64")]
    const GETRUSAGE: u64 = 165;
    let result = raw::syscall2(GETRUSAGE, who as i64 as u64, usage as u64);
    crate::syscall_result_to_c_int(result as i64)
}

#[no_mangle]
pub unsafe extern "C" fn posix_madvise(addr: *mut u8, len: usize, advice: i32) -> i32 {
    let hint = match advice {
        0..=3 => advice as u64,
        // POSIX_MADV_DONTNEED is an optional optimization. The kernel's
        // MADV_DONTNEED can discard contents, so use its NORMAL range check.
        4 => 0,
        _ => return crate::EINVAL,
    };
    let result = raw::syscall3(nr::MADVISE, addr as u64, len as u64, hint);
    // POSIX returns the error number directly and leaves errno alone.
    -(result as i64) as i32
}

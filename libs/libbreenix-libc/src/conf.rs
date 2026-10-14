//! POSIX confstr/pathconf/fpathconf. Query IDs are defined by our unistd.h.
use crate::pthread::{errno, set_errno};

#[no_mangle]
pub unsafe extern "C" fn confstr(name: i32, buffer: *mut u8, len: usize) -> usize {
    let value: &[u8] = match name {
        0 => b"/bin:/usr/bin\0",
        // Recognized compilation environments are unavailable: Breenix does
        // not ship a POSIX c17/c99 toolchain. Preserve errno in this case.
        1..=32 => return 0,
        _ => {
            set_errno(22);
            return 0;
        }
    };
    if len != 0 {
        if buffer.is_null() {
            set_errno(14);
            return 0;
        }
        let copied = len.min(value.len()) - 1;
        core::ptr::copy_nonoverlapping(value.as_ptr(), buffer, copied);
        *buffer.add(copied) = 0;
    }
    value.len()
}
fn limit(name: i32) -> Option<i64> {
    Some(match name {
        0 => 32000,       // ext2's enforced link limit
        1 | 2 | 3 => 255, // MAX_CANON, MAX_INPUT, NAME_MAX
        4 | 5 => 4096,    // PATH_MAX, PIPE_BUF
        6 | 7 => 1,       // CHOWN_RESTRICTED, NO_TRUNC
        8 => 0,           // VDISABLE
        9 => 1,           // _PC_2_SYMLINKS
        10 => 4096,       // _PC_ALLOC_SIZE_MIN
        11 => -1,         // _PC_ASYNC_IO
        12 => -1,         // _PC_FALLOC
        13 => 64,         // _PC_FILESIZEBITS
        14 => -1,         // _PC_PRIO_IO
        15 => -1,         // _PC_REC_INCR_XFER_SIZE
        16 => -1,         // _PC_REC_MAX_XFER_SIZE
        17 => 4096,       // _PC_REC_MIN_XFER_SIZE
        18 => 4096,       // _PC_REC_XFER_ALIGN
        19 => 4095,       // _PC_SYMLINK_MAX
        20 => -1,         // _PC_SYNC_IO
        21 => -1,         // _PC_TEXTDOMAIN_MAX
        22 => 1000000000, // _PC_TIMESTAMP_RESOLUTION
        _ => return None,
    })
}
#[no_mangle]
pub unsafe extern "C" fn pathconf(path: *const u8, name: i32) -> i64 {
    let Some(value) = limit(name) else {
        set_errno(22);
        return -1;
    };
    if path.is_null() {
        set_errno(14);
        return -1;
    }
    let saved = errno();
    // stat follows symlinks and reports nonexistent or inaccessible paths.
    let mut stat = [0u64; 18];
    if crate::stat(path, stat.as_mut_ptr().cast()) != 0 {
        return -1;
    }
    set_errno(saved);
    value
}
#[no_mangle]
pub unsafe extern "C" fn fpathconf(fd: i32, name: i32) -> i64 {
    let Some(value) = limit(name) else {
        set_errno(22);
        return -1;
    };
    let saved = errno();
    let mut stat = [0u64; 18];
    if crate::fstat(fd, stat.as_mut_ptr().cast()) != 0 {
        return -1;
    }
    set_errno(saved);
    value
}

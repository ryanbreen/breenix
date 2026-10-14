//! POSIX confstr/pathconf/fpathconf. Query IDs are defined by our unistd.h.
use crate::pthread::set_errno;

#[no_mangle]
pub unsafe extern "C" fn confstr(name: i32, buffer: *mut u8, len: usize) -> usize {
    let value: &[u8] = match name {
        0 => b"/bin:/usr/bin\0",
        // The C driver selects LP64/OFF64 and links the thread runtime by default.
        7..=9 | 13 | 14 | 16 => b"\0",
        15 => b"POSIX_V8_LP64_OFF64\0",
        // Recognized programming environments that Breenix does not support.
        1..=6 | 10..=12 => return 0,
        _ => { set_errno(22); return 0; }
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
        1 => 4096,        // MAX_CANON
        2 => 4095,        // MAX_INPUT
        3 => 255,         // NAME_MAX
        4 | 5 => 4096,    // PATH_MAX, PIPE_BUF
        6 | 7 => 1,       // CHOWN_RESTRICTED, NO_TRUNC
        8 => 0,           // VDISABLE
        9 | 10 | 12 | 13 | 14 | 15 | 16 | 17 | 18 | 21 | 22 => -1,
        // No async/prioritized I/O or fallocate; transfer recommendations and
        // file-size, symlink and textdomain limits are indeterminate, not invalid names.
        11 | 19 => 1,     // SYNC_IO, 2_SYMLINKS
        20 => 1_000_000_000, // ext2 timestamps have second resolution
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
    // stat follows symlinks and reports nonexistent or inaccessible paths.
    let mut stat = [0u64; 18];
    if crate::stat(path, stat.as_mut_ptr().cast()) != 0 {
        return -1;
    }
    value
}
#[no_mangle]
pub unsafe extern "C" fn fpathconf(fd: i32, name: i32) -> i64 {
    let Some(value) = limit(name) else {
        set_errno(22);
        return -1;
    };
    let mut stat = [0u64; 18];
    if crate::fstat(fd, stat.as_mut_ptr().cast()) != 0 {
        return -1;
    }
    value
}

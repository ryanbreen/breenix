//! POSIX confstr/pathconf/fpathconf. Query IDs are defined by our unistd.h.
use crate::pthread::set_errno;

#[no_mangle]
pub unsafe extern "C" fn confstr(name: i32, buffer: *mut u8, len: usize) -> usize {
    if name != 0 {
        set_errno(22);
        return 0;
    }
    let value = b"/bin:/usr/bin\0";
    // Like Linux, a null buffer only asks for the size, whatever len says.
    if len != 0 && !buffer.is_null() {
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

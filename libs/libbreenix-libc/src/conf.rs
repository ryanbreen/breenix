//! POSIX confstr/pathconf/fpathconf. Query IDs are defined by our unistd.h.
use crate::pthread::set_errno;

#[no_mangle]
pub unsafe extern "C" fn confstr(name: i32, buffer: *mut u8, len: usize) -> usize {
    // LP64/off64 is native on both Breenix targets. No extra flags or libraries
    // are required. Other data models have no configuration-defined value.
    let value: &[u8] = match name {
        0 => b"/bin:/usr/bin\0",
        7..=9 | 13..=14 | 16 | 23..=25 | 29..=30 | 32 => b"\0",
        15 => b"POSIX_V8_LP64_OFF64\0",
        31 => b"POSIX_V7_LP64_OFF64\0",
        1..=32 => return 0,
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
fn valid(name: i32) -> bool { (0..=22).contains(&name) }
fn limit(name: i32, dev: u64) -> i64 {
    // The kernel gives pseudo filesystems a separate device namespace. Their
    // link, symlink and allocation limits are not those of a mounted ext2 disk.
    let ext2 = dev < 0xffff0000;
    match name {
        0 if ext2 => 32000,
        1 | 2 | 3 => 255,
        4 | 5 => 4096,
        6 | 7 => 1,
        8 => 0,
        9 if ext2 => 64,
        10 => if ext2 { 1 } else { 0 },
        18..=20 => -1,     // async I/O, fallocate and prioritized I/O unsupported
        21 if ext2 => 1,   // synchronized file I/O
        22 if ext2 => 1_000_000_000, // ext2 inode timestamps have second resolution
        _ => -1,          // valid but indeterminate; leave errno unchanged
    }
}
#[no_mangle]
pub unsafe extern "C" fn pathconf(path: *const u8, name: i32) -> i64 {
    if !valid(name) {
        set_errno(22);
        return -1;
    }
    if path.is_null() {
        set_errno(14);
        return -1;
    }
    // stat follows symlinks and reports nonexistent or inaccessible paths.
    let mut stat = [0u64; 18];
    if crate::stat(path, stat.as_mut_ptr().cast()) != 0 {
        return -1;
    }
    limit(name, stat[0])
}
#[no_mangle]
pub unsafe extern "C" fn fpathconf(fd: i32, name: i32) -> i64 {
    if !valid(name) {
        set_errno(22);
        return -1;
    }
    let mut stat = [0u64; 18];
    if crate::fstat(fd, stat.as_mut_ptr().cast()) != 0 {
        return -1;
    }
    limit(name, stat[0])
}

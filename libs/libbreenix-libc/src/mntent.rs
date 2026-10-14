//! The mounted file system table (<mntent.h>): lines of `fsname dir type options freq
//! passno` separated by blanks, `#` starting a comment, and `\ooo` octal escapes for
//! blanks within a field. Streams are stdio's.

use core::ptr::{addr_of_mut, null_mut};

#[repr(C)]
pub struct Mntent {
    mnt_fsname: *mut u8,
    mnt_dir: *mut u8,
    mnt_type: *mut u8,
    mnt_opts: *mut u8,
    mnt_freq: i32,
    mnt_passno: i32,
}

/// stdio's opaque stream.
pub enum File {}

extern "C" {
    fn fopen(path: *const u8, mode: *const u8) -> *mut File;
    fn fclose(f: *mut File) -> i32;
    fn fgets(buf: *mut u8, size: i32, f: *mut File) -> *mut u8;
    fn fgetc(f: *mut File) -> i32;
    fn fwrite(p: *const u8, size: usize, n: usize, f: *mut File) -> usize;
    fn fseek(f: *mut File, offset: i64, whence: i32) -> i32;
    fn fflush(f: *mut File) -> i32;
}

const EOF: i32 = -1;
const SEEK_END: i32 = 2;

#[no_mangle]
pub unsafe extern "C" fn setmntent(path: *const u8, mode: *const u8) -> *mut File {
    fopen(path, mode)
}

#[no_mangle]
pub unsafe extern "C" fn endmntent(f: *mut File) -> i32 {
    if !f.is_null() {
        fclose(f);
    }
    1
}

fn is_blank(b: u8) -> bool {
    b == b' ' || b == b'\t' || b == b'\n'
}

/// Split the NUL-terminated line at `line` into blank-separated fields in place,
/// decoding `\ooo` escapes; at most `out.len()` fields.
unsafe fn split(line: *mut u8, out: &mut [*mut u8; 6]) -> usize {
    let mut p = line;
    let mut n = 0;
    while n < out.len() {
        while *p != 0 && is_blank(*p) {
            p = p.add(1);
        }
        if *p == 0 {
            break;
        }
        out[n] = p;
        n += 1;
        let mut w = p;
        while *p != 0 && !is_blank(*p) {
            let octal = |b: u8| (b'0'..=b'7').contains(&b);
            if *p == b'\\' && octal(*p.add(1)) && octal(*p.add(2)) && octal(*p.add(3)) {
                *w = ((*p.add(1) - b'0') << 6) | ((*p.add(2) - b'0') << 3) | (*p.add(3) - b'0');
                p = p.add(4);
            } else {
                *w = *p;
                p = p.add(1);
            }
            w = w.add(1);
        }
        let ended = *p == 0;
        *w = 0;
        if ended {
            break;
        }
        p = p.add(1);
    }
    n
}

unsafe fn number(field: *const u8) -> i32 {
    let mut value: i32 = 0;
    let mut p = field;
    while (*p).is_ascii_digit() {
        value = value.saturating_mul(10).saturating_add((*p - b'0') as i32);
        p = p.add(1);
    }
    value
}

#[no_mangle]
pub unsafe extern "C" fn getmntent_r(f: *mut File, mnt: *mut Mntent, buf: *mut u8, len: i32) -> *mut Mntent {
    if f.is_null() || mnt.is_null() || buf.is_null() || len < 2 {
        return null_mut();
    }
    loop {
        if fgets(buf, len, f).is_null() {
            return null_mut();
        }
        // A line longer than the buffer: drop the rest of it.
        let mut n = 0;
        while *buf.add(n) != 0 {
            n += 1;
        }
        if n > 0 && *buf.add(n - 1) != b'\n' {
            loop {
                let c = fgetc(f);
                if c == EOF || c == b'\n' as i32 {
                    break;
                }
            }
        }
        let mut start = buf;
        while *start != 0 && is_blank(*start) {
            start = start.add(1);
        }
        if *start == 0 || *start == b'#' {
            continue;
        }
        let mut fields = [null_mut(); 6];
        let count = split(start, &mut fields);
        if count < 4 {
            continue;
        }
        mnt.write(Mntent {
            mnt_fsname: fields[0],
            mnt_dir: fields[1],
            mnt_type: fields[2],
            mnt_opts: fields[3],
            mnt_freq: if count > 4 { number(fields[4]) } else { 0 },
            mnt_passno: if count > 5 { number(fields[5]) } else { 0 },
        });
        return mnt;
    }
}

static mut ENTRY: Mntent = Mntent {
    mnt_fsname: null_mut(), mnt_dir: null_mut(), mnt_type: null_mut(), mnt_opts: null_mut(),
    mnt_freq: 0, mnt_passno: 0,
};
static mut LINE: [u8; 4096] = [0; 4096];

#[no_mangle]
pub unsafe extern "C" fn getmntent(f: *mut File) -> *mut Mntent {
    getmntent_r(f, addr_of_mut!(ENTRY), addr_of_mut!(LINE) as *mut u8, 4096)
}

/// Write `field` with blanks and backslashes escaped as `\ooo`.
unsafe fn put_field(f: *mut File, field: *const u8) -> bool {
    let mut p = field;
    while *p != 0 {
        let b = *p;
        let ok = if is_blank(b) || b == b'\\' {
            let esc = [b'\\', b'0' + (b >> 6), b'0' + ((b >> 3) & 7), b'0' + (b & 7)];
            fwrite(esc.as_ptr(), 1, 4, f) == 4
        } else {
            fwrite(p, 1, 1, f) == 1
        };
        if !ok {
            return false;
        }
        p = p.add(1);
    }
    true
}

unsafe fn put_number(f: *mut File, mut n: i32) -> bool {
    let mut digits = [0u8; 12];
    let mut i = digits.len();
    let negative = n < 0;
    loop {
        i -= 1;
        digits[i] = b'0' + (n % 10).unsigned_abs() as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    if negative {
        i -= 1;
        digits[i] = b'-';
    }
    fwrite(digits.as_ptr().add(i), 1, digits.len() - i, f) == digits.len() - i
}

/// addmntent: append the entry; 0 on success, 1 on failure.
#[no_mangle]
pub unsafe extern "C" fn addmntent(f: *mut File, mnt: *const Mntent) -> i32 {
    if f.is_null() || mnt.is_null() || fseek(f, 0, SEEK_END) != 0 {
        return 1;
    }
    let m = &*mnt;
    let space = |f| fwrite(b" ".as_ptr(), 1, 1, f) == 1;
    let ok = put_field(f, m.mnt_fsname) && space(f) && put_field(f, m.mnt_dir) && space(f)
        && put_field(f, m.mnt_type) && space(f) && put_field(f, m.mnt_opts) && space(f)
        && put_number(f, m.mnt_freq) && space(f) && put_number(f, m.mnt_passno)
        && fwrite(b"\n".as_ptr(), 1, 1, f) == 1;
    if ok && fflush(f) == 0 { 0 } else { 1 }
}

/// hasmntopt: the option in the entry's comma-separated list that is `opt` or starts
/// with `opt=`, or null.
#[no_mangle]
pub unsafe extern "C" fn hasmntopt(mnt: *const Mntent, opt: *const u8) -> *mut u8 {
    if mnt.is_null() || opt.is_null() || (*mnt).mnt_opts.is_null() {
        return null_mut();
    }
    let mut olen = 0;
    while *opt.add(olen) != 0 {
        olen += 1;
    }
    let mut p = (*mnt).mnt_opts;
    while *p != 0 {
        let mut matches = true;
        for i in 0..olen {
            if *p.add(i) != *opt.add(i) {
                matches = false;
                break;
            }
        }
        if matches {
            let next = *p.add(olen);
            if next == 0 || next == b',' || next == b'=' {
                return p;
            }
        }
        while *p != 0 && *p != b',' {
            p = p.add(1);
        }
        if *p == b',' {
            p = p.add(1);
        }
    }
    null_mut()
}

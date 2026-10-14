use super::file;
use super::format::{Sink, StreamOut};
use core::{
    ptr,
    sync::atomic::{AtomicU64, Ordering},
};
/// POSIX perror: writes bytes without touching stderr's orientation.
#[no_mangle]
pub unsafe extern "C" fn perror(context: *const u8) {
    let saved = crate::pthread::errno();
    let text = crate::stdlib::numscan::error_text(saved);
    let f = file::stderr;
    file::lock(f);
    let mut out = StreamOut::new(f);
    if !context.is_null() && *context != 0 {
        let mut n = 0;
        while *context.add(n) != 0 {
            n += 1;
        }
        out.bytes(core::slice::from_raw_parts(context, n));
        out.bytes(b": ");
    }
    match text {
        Some(text) => {
            out.bytes(text.as_bytes());
        }
        None => {
            out.bytes(b"Unknown error ");
            let mut digits = [0u8; 12];
            let mut i = digits.len();
            let mut v = saved.unsigned_abs();
            loop {
                i -= 1;
                digits[i] = b'0' + (v % 10) as u8;
                v /= 10;
                if v == 0 {
                    break;
                }
            }
            if saved < 0 {
                i -= 1;
                digits[i] = b'-';
            }
            out.bytes(&digits[i..]);
        }
    }
    out.bytes(b"\n");
    out.finish();
    file::unlock(f);
    crate::pthread::set_errno(saved);
}
#[no_mangle]
pub unsafe extern "C" fn remove(path: *const u8) -> i32 {
    if crate::unlink(path) == 0 {
        return 0;
    }
    let e = crate::pthread::errno();
    if e != 21 && e != 1 {
        return -1;
    }
    // EISDIR or EPERM: a directory is removed by rmdir. ENOTDIR from rmdir means
    // the path is not a directory, so unlink's own error is the answer.
    if crate::rmdir(path) == 0 {
        return 0;
    }
    if crate::pthread::errno() == 20 {
        crate::pthread::set_errno(e);
    }
    -1
}
/// Writes 16 hex digits and a null: 32 bits from getrandom, then the next value of
/// a per-process 32-bit sequence. Distinct sequence values give distinct names, so a
/// process gets 2^32 different names (TMP_MAX in stdio.h is the largest int below
/// that), and the random half keeps them from being predictable.
pub(super) unsafe fn temp_name(out: &mut [u8], sequence: &AtomicU64) {
    let mut random = [0u8; 4];
    let mut got = 0;
    while got < random.len() {
        let n = crate::getrandom(random.as_mut_ptr().add(got), random.len() - got, 0);
        if n <= 0 {
            break;
        }
        got += n as usize;
    }
    let token = ((u32::from_ne_bytes(random) as u64) << 32)
        | (sequence.fetch_add(1, Ordering::Relaxed) & 0xffff_ffff);
    for (i, c) in out[..16].iter_mut().enumerate() {
        *c = b"0123456789abcdef"[((token >> ((15 - i) * 4)) & 15) as usize];
    }
    out[16] = 0;
}
static NAME_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static NAME_LOCK: crate::pthread::RecursiveLock = crate::pthread::RecursiveLock::new();
static mut NAME: [u8; 35] = [0; 35];
#[no_mangle]
pub unsafe extern "C" fn tmpnam(buffer: *mut u8) -> *mut u8 {
    NAME_LOCK.lock();
    let output = if buffer.is_null() {
        ptr::addr_of_mut!(NAME) as *mut u8
    } else {
        buffer
    };
    let prefix = b"/tmp/breenix-name-";
    ptr::copy_nonoverlapping(prefix.as_ptr(), output, prefix.len());
    let name = core::slice::from_raw_parts_mut(output.add(prefix.len()), 17);
    let saved = crate::pthread::errno();
    // lstat names a file without opening it, so a FIFO or device cannot block it.
    // As in tmpfile, 64 taken names in a row means the names are being taken
    // deliberately, and tmpnam gives up with EEXIST.
    let mut status = [0u64; 32];
    let mut result = ptr::null_mut();
    crate::pthread::set_errno(17);
    for _ in 0..64 {
        temp_name(name, &NAME_SEQUENCE);
        if crate::lstat(output, status.as_mut_ptr() as *mut u8) == 0 {
            continue;
        }
        if crate::pthread::errno() == 2 {
            crate::pthread::set_errno(saved);
            result = output;
        }
        break;
    }
    NAME_LOCK.unlock();
    result
}
static TERMINAL: [u8; 9] = *b"/dev/tty\0";
/// POSIX ctermid: the controlling terminal's pathname, L_ctermid bytes with its null.
#[no_mangle]
pub unsafe extern "C" fn ctermid(s: *mut u8) -> *mut u8 {
    if s.is_null() {
        return TERMINAL.as_ptr() as *mut u8;
    }
    ptr::copy_nonoverlapping(TERMINAL.as_ptr(), s, TERMINAL.len());
    s
}
/// The Linux renameat syscall number (UAPI unistd.h): asm-generic on aarch64.
#[cfg(target_arch = "aarch64")]
const RENAMEAT: i64 = 38;
#[cfg(target_arch = "x86_64")]
const RENAMEAT: i64 = 264;
#[no_mangle]
pub unsafe extern "C" fn renameat(
    old_dir: i32,
    old: *const u8,
    new_dir: i32,
    new: *const u8,
) -> i32 {
    crate::syscall(
        RENAMEAT,
        old_dir as i64,
        old as i64,
        new_dir as i64,
        new as i64,
        0,
        0,
    ) as i32
}

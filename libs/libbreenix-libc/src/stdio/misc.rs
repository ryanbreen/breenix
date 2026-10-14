use super::file;
#[no_mangle]
pub unsafe extern "C" fn perror(context: *const u8) {
    let saved = crate::pthread::errno();
    let text = crate::stdlib::numscan::error_text(saved);
    file::lock(file::stderr);
    if !context.is_null() && *context != 0 {
        file::fputs(context, file::stderr);
        file::write_locked(file::stderr, b": ");
    }
    if let Some(text) = text {
        file::write_locked(file::stderr, text.as_bytes());
    } else {
        super::format::fprintf(
            file::stderr,
            c"Unknown error %d".as_ptr() as *const u8,
            saved,
        );
    }
    file::putc_locked(b'\n', file::stderr);
    file::unlock(file::stderr);
    crate::pthread::set_errno(saved);
}
#[no_mangle]
pub unsafe extern "C" fn remove(path: *const u8) -> i32 {
    if crate::unlink(path) == 0 {
        return 0;
    }
    let e = crate::pthread::errno();
    if e == 21 || e == 1 {
        crate::rmdir(path)
    } else {
        -1
    }
}
use core::{
    ptr,
    sync::atomic::{AtomicU64, Ordering},
};
static NAME_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static NAME_LOCK: crate::pthread::RecursiveLock = crate::pthread::RecursiveLock::new();
static mut NAME: [u8; 64] = [0; 64];
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
    for _ in 0..238328 {
        let token =
            ((crate::getpid() as u64) << 32) ^ NAME_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        for i in 0..16 {
            *output.add(prefix.len() + i) =
                b"0123456789abcdef"[((token >> ((15 - i) * 4)) & 15) as usize];
        }
        *output.add(prefix.len() + 16) = 0;
        let fd = crate::open(output, 0, 0);
        if fd >= 0 {
            crate::close(fd);
            continue;
        }
        let result = if crate::pthread::errno() == 2 {
            output
        } else {
            ptr::null_mut()
        };
        NAME_LOCK.unlock();
        return result;
    }
    NAME_LOCK.unlock();
    crate::pthread::set_errno(17);
    ptr::null_mut()
}

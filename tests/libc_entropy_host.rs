use std::cell::Cell;

#[path = "../libs/libbreenix-libc/src/entropy.rs"]
mod entropy;

const EINTR: i32 = 4;
const EIO: i32 = 5;
const EFAULT: i32 = 14;
const EINVAL: i32 = 22;

thread_local! {
    static ERRNO: Cell<i32> = const { Cell::new(0) };
    static READS: Cell<usize> = const { Cell::new(0) };
}

fn __errno_location() -> *mut i32 {
    ERRNO.with(Cell::as_ptr)
}

unsafe fn getrandom(buffer: *mut u8, len: usize, flags: u32) -> isize {
    assert_eq!(flags, 0);
    READS.with(|reads| reads.set(reads.get() + 1));
    if buffer.is_null() {
        ERRNO.with(|errno| errno.set(EFAULT));
        return -1;
    }
    if READS.with(Cell::get) == 1 {
        ERRNO.with(|errno| errno.set(EINTR));
        return -1;
    }
    let count = len.min(97);
    // SAFETY: getentropy's caller supplied a writable span of len bytes.
    unsafe { buffer.write_bytes(0x6e, count) };
    count as isize
}

#[test]
fn getentropy_fills_the_whole_span_or_returns_an_error() {
    let mut bytes = [0u8; 257];
    let buffer = bytes.as_mut_ptr().cast();
    // SAFETY: the first two calls use the live buffer. The last asks the kernel
    // boundary to reject an invalid address without dereferencing it in libc.
    unsafe {
        assert_eq!(entropy::getentropy(buffer, 257), -1);
        assert_eq!(ERRNO.with(Cell::get), EINVAL);
        assert_eq!(READS.with(Cell::get), 0);
        assert_eq!(entropy::getentropy(buffer, 256), 0);
        assert_eq!(bytes[..256], [0x6e; 256]);
        assert_eq!(bytes[256], 0);
        assert_eq!(READS.with(Cell::get), 4);
        assert_eq!(entropy::getentropy(std::ptr::null_mut(), 1), -1);
        assert_eq!(ERRNO.with(Cell::get), EFAULT);
    }
}

//! getrandom 0.3.3 custom backend, linked into the Breenix driver dylib.
#![no_std]
#![feature(rustc_private)]
#![deny(unsafe_op_in_unsafe_fn)]

use core::ffi::{c_int, c_void};
use getrandom::Error;

extern "C" {
    fn getentropy(buffer: *mut c_void, length: usize) -> c_int;
    fn __errno_location() -> *mut c_int;
}

/// # Safety
/// `dest` must be writable for `len` bytes, which may be uninitialized.
#[no_mangle]
pub unsafe extern "Rust" fn __getrandom_v03_custom(dest: *mut u8, len: usize) -> Result<(), Error> {
    const GETENTROPY_MAX: usize = 256;
    let mut offset = 0;
    while offset < len {
        let length = (len - offset).min(GETENTROPY_MAX);
        // SAFETY: this chunk is within the caller's writable destination.
        if unsafe { getentropy(dest.add(offset).cast(), length) } != 0 {
            // SAFETY: libc supplies the calling thread's errno pointer.
            let errno = unsafe { *__errno_location() };
            return Err(match u16::try_from(errno) {
                Ok(errno) if errno != 0 => Error::new_custom(errno),
                _ => Error::ERRNO_NOT_POSITIVE,
            });
        }
        offset += length;
    }
    Ok(())
}

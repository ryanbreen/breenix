use core::ffi::{c_int, c_void};

use crate::{__errno_location, getrandom, EINTR, EINVAL, EIO};

// POSIX.1-2024: <limits.h> GETENTROPY_MAX and getentropy().
const GETENTROPY_MAX: usize = 256;

/// Fill the entire requested span with bytes from the kernel entropy pool.
#[no_mangle]
pub unsafe extern "C" fn getentropy(buf: *mut c_void, len: usize) -> c_int {
    if len > GETENTROPY_MAX {
        // SAFETY: errno points into the calling thread's libc state.
        unsafe { *__errno_location() = EINVAL };
        return -1;
    }

    let mut written = 0;
    while written < len {
        let remaining = len - written;
        // SAFETY: this points to the next byte of the caller's span. The kernel
        // validates the user address before writing to it.
        let result = unsafe { getrandom(buf.cast::<u8>().wrapping_add(written), remaining, 0) };
        if result < 0 {
            // SAFETY: errno points into the calling thread's libc state.
            if unsafe { *__errno_location() } == EINTR {
                continue;
            }
            return -1;
        }
        if result == 0 || result as usize > remaining {
            // SAFETY: errno points into the calling thread's libc state.
            unsafe { *__errno_location() = EIO };
            return -1;
        }
        written += result as usize;
    }
    0
}

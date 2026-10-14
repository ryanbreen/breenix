//! getcwd with caller-provided storage and the allocating extension ash uses.
use core::ptr::null_mut;

#[no_mangle]
pub unsafe extern "C" fn getcwd(buffer: *mut u8, size: usize) -> *mut u8 {
    let allocated = buffer.is_null();
    if !allocated && size == 0 {
        crate::pthread::set_errno(22);
        return null_mut();
    }
    // The kernel's pathname limit includes the terminator. An automatic allocation
    // is bounded by that limit; an explicit size retains the caller's semantics.
    let cap = if allocated && size == 0 { 4096 } else { size };
    let dest = if allocated {
        crate::malloc(cap)
    } else {
        buffer
    };
    if dest.is_null() {
        crate::pthread::set_errno(12);
        return null_mut();
    }
    let result =
        libbreenix::raw::syscall2(libbreenix::syscall::nr::GETCWD, dest as u64, cap as u64) as i64;
    if result < 0 {
        if allocated {
            crate::free(dest);
        }
        crate::pthread::set_errno((-result) as i32);
        null_mut()
    } else {
        dest
    }
}

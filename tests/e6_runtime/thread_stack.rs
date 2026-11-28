use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};

static STACK: AtomicUsize = AtomicUsize::new(0);
const EAGAIN: i32 = 11;

extern "C" {
    fn pthread_attr_getstacksize(attr: *const c_void, size: *mut usize) -> i32;
}

// Stop after the real pthread attribute setter has accepted the stack.
// Report a creation error so the probe needs no emulated thread scheduler.
#[no_mangle]
unsafe extern "C" fn __wrap_pthread_create(
    _: *mut usize,
    attr: *const c_void,
    _: extern "C" fn(*mut c_void) -> *mut c_void,
    _: *mut c_void,
) -> i32 {
    let mut size = 0;
    assert_eq!(unsafe { pthread_attr_getstacksize(attr, &mut size) }, 0);
    STACK.store(size, Ordering::Relaxed);
    EAGAIN
}

fn main() {
    let expected = std::env::args().nth(1).unwrap().parse::<usize>().unwrap();
    let result = std::thread::Builder::new().stack_size(16384).spawn(|| ());
    assert_eq!(result.unwrap_err().raw_os_error(), Some(EAGAIN));
    assert_eq!(STACK.load(Ordering::Relaxed), expected);
}

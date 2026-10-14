//! Compile the production getcwd wrapper against scripted kernel and allocator
//! boundaries. No host getcwd implementation participates in these tests.
extern crate self as libbreenix;
use std::{
    cell::{Cell, RefCell},
    ffi::CStr,
};
#[path = "../libs/libbreenix-libc/src/cwd.rs"]
mod cwd;
thread_local! {
    static ERRNO: Cell<i32> = const { Cell::new(77) };
    static FAIL_ALLOC: Cell<bool> = const { Cell::new(false) };
    static ERROR: Cell<i64> = const { Cell::new(0) };
    static LIVE: RefCell<std::collections::HashMap<usize,usize>> = RefCell::new(std::collections::HashMap::new());
    static CAP: Cell<usize> = const { Cell::new(0) };
}
mod pthread {
    pub fn set_errno(e: i32) {
        crate::ERRNO.with(|v| v.set(e));
    }
}
pub mod syscall {
    pub mod nr {
        pub const GETCWD: u64 = 17;
    }
}
pub mod raw {
    pub unsafe fn syscall2(nr: u64, ptr: u64, cap: u64) -> u64 {
        assert_eq!(nr, 17);
        crate::CAP.with(|v| v.set(cap as usize));
        let error = crate::ERROR.with(|v| v.get());
        if error != 0 {
            return error as u64;
        }
        let path = b"/tmp/suite/d\0";
        if (cap as usize) < path.len() {
            return (-34i64) as u64;
        }
        std::ptr::copy_nonoverlapping(path.as_ptr(), ptr as *mut u8, path.len());
        path.len() as u64
    }
}
unsafe fn malloc(size: usize) -> *mut u8 {
    if FAIL_ALLOC.with(|v| v.get()) {
        return std::ptr::null_mut();
    }
    let ptr = std::alloc::alloc(std::alloc::Layout::from_size_align(size, 16).unwrap());
    LIVE.with(|v| v.borrow_mut().insert(ptr as usize, size));
    ptr
}
unsafe fn free(ptr: *mut u8) {
    let size = LIVE.with(|v| {
        v.borrow_mut()
            .remove(&(ptr as usize))
            .expect("owned allocation")
    });
    std::alloc::dealloc(ptr, std::alloc::Layout::from_size_align(size, 16).unwrap());
    pthread::set_errno(99); // Failure cleanup must not overwrite the kernel error.
}
#[test]
fn caller_buffer_and_too_small_buffer() {
    unsafe {
        let mut buf = [0xaa; 32];
        assert_eq!(cwd::getcwd(buf.as_mut_ptr(), buf.len()), buf.as_mut_ptr());
        assert_eq!(
            CStr::from_ptr(buf.as_ptr().cast()).to_bytes(),
            b"/tmp/suite/d"
        );
        assert_eq!(ERRNO.with(|v| v.get()), 77);
        assert!(cwd::getcwd(buf.as_mut_ptr(), 2).is_null());
        assert_eq!(ERRNO.with(|v| v.get()), 34);
        assert!(cwd::getcwd(buf.as_mut_ptr(), 0).is_null());
        assert_eq!(ERRNO.with(|v| v.get()), 22);
    }
}
#[test]
fn allocated_current_directory_and_explicit_capacity() {
    unsafe {
        let p = cwd::getcwd(std::ptr::null_mut(), 0);
        assert!(!p.is_null());
        assert_eq!(CAP.with(|v| v.get()), 4096);
        assert_eq!(CStr::from_ptr(p.cast()).to_bytes(), b"/tmp/suite/d");
        free(p);
        let p = cwd::getcwd(std::ptr::null_mut(), 32);
        assert!(!p.is_null());
        assert_eq!(CAP.with(|v| v.get()), 32);
        free(p);
        LIVE.with(|v| assert!(v.borrow().is_empty()));
    }
}
#[test]
fn allocation_failure_and_kernel_failure_release_storage() {
    unsafe {
        FAIL_ALLOC.with(|v| v.set(true));
        assert!(cwd::getcwd(std::ptr::null_mut(), 0).is_null());
        assert_eq!(ERRNO.with(|v| v.get()), 12);
        FAIL_ALLOC.with(|v| v.set(false));
        ERROR.with(|v| v.set(-2));
        assert!(cwd::getcwd(std::ptr::null_mut(), 0).is_null());
        assert_eq!(ERRNO.with(|v| v.get()), 2);
        LIVE.with(|v| assert!(v.borrow().is_empty()));
        ERROR.with(|v| v.set(0));
        assert!(cwd::getcwd(std::ptr::null_mut(), 2).is_null());
        assert_eq!(ERRNO.with(|v| v.get()), 34);
        LIVE.with(|v| assert!(v.borrow().is_empty()));
    }
}

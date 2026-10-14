//! Check C affinity result translation without issuing guest syscalls.
extern crate self as libbreenix;
use core::sync::atomic::{AtomicI32, AtomicI64, Ordering::Relaxed};
static RESULT: AtomicI64 = AtomicI64::new(8);
static ERRNO: AtomicI32 = AtomicI32::new(0);
const EIO: i32 = 5;
const EINTR: i32 = 4;
unsafe fn __errno_location() -> *mut i32 { ERRNO.as_ptr() }
fn syscall_result_to_c_int(result: i64) -> i32 {
    ERRNO.store(-result as i32, Relaxed);
    -1
}
unsafe fn open_point(_: *const u8, _: i32, _: u32) -> i32 {
    ERRNO.store(2, Relaxed);
    -1
}
unsafe fn read_point(_: i32, _: *mut u8, _: usize) -> isize { panic!("read after failed open") }
fn close_point(_: i32) -> i32 { panic!("close after failed open") }
mod syscall {
    pub mod raw {
        pub unsafe fn syscall3(_: u64, pid: u64, len: u64, pointer: u64) -> u64 {
            assert_eq!(pid, 0);
            let result = crate::RESULT.load(crate::Relaxed);
            if result == 8 {
                assert!(len >= 8);
                core::ptr::copy_nonoverlapping(0b1011u64.to_ne_bytes().as_ptr(), pointer as *mut u8, 8);
            }
            result as u64
        }
    }
}
#[path = "../libs/libbreenix-libc/src/pthread"]
mod pthread {
    pub fn set_errno(errno: i32) { crate::ERRNO.store(errno, crate::Relaxed); }
    #[path = "cpuinfo.rs"]
    pub mod cpuinfo;
    #[path = "affinity.rs"]
    pub mod affinity;
}

#[test]
fn byte_count_success_clears_tail_and_errors_preserve_the_set() {
    let mut set = [0xaau8; 128];
    ERRNO.store(77, Relaxed);
    unsafe { assert_eq!(pthread::affinity::sched_getaffinity(0, set.len(), set.as_mut_ptr()), 0); }
    assert_eq!(u64::from_ne_bytes(set[..8].try_into().unwrap()), 0b1011);
    assert!(set[8..].iter().all(|&byte| byte == 0));
    assert_eq!(ERRNO.load(Relaxed), 77);
    for errno in [3, 14, 22] { // ESRCH, EFAULT, EINVAL
        RESULT.store(-errno, Relaxed);
        set.fill(0xaa);
        unsafe { assert_eq!(pthread::affinity::sched_getaffinity(0, set.len(), set.as_mut_ptr()), -1); }
        assert_eq!(ERRNO.load(Relaxed), errno as i32);
        assert!(set.iter().all(|&byte| byte == 0xaa));
    }
    assert_eq!(pthread::affinity::processor_count(), -1);
    assert_eq!(ERRNO.load(Relaxed), 2); // failed procfs open preserves ENOENT
}

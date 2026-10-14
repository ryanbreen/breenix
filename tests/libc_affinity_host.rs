//! Check C affinity result translation and sysconf's processor count without
//! issuing guest syscalls.
extern crate self as libbreenix;
use core::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, AtomicUsize, Ordering::Relaxed};
use std::sync::Mutex;
static RESULT: AtomicI64 = AtomicI64::new(8);
static ERRNO: AtomicI32 = AtomicI32::new(0);
static OPEN_FAILS: AtomicBool = AtomicBool::new(false);
static READ_ERRNO: AtomicI32 = AtomicI32::new(0);
static CPUINFO: Mutex<&[u8]> = Mutex::new(b"");
static OFFSET: AtomicUsize = AtomicUsize::new(0);
static OPEN_FDS: AtomicI32 = AtomicI32::new(0);
static SERIAL: Mutex<()> = Mutex::new(());
const EIO: i32 = 5;
const EINTR: i32 = 4;
const FD: i32 = 9;
unsafe fn __errno_location() -> *mut i32 { ERRNO.as_ptr() }
fn syscall_result_to_c_int(result: i64) -> i32 {
    if result < 0 {
        ERRNO.store(-result as i32, Relaxed);
        return -1;
    }
    result as i32
}
// The library's own non-cancelling descriptor calls; the exported
// cancellation points are deliberately absent, so using one fails to build.
unsafe fn open(path: *const u8, _: i32, _: u32) -> i32 {
    assert_eq!(core::ffi::CStr::from_ptr(path.cast()).to_bytes(), b"/proc/cpuinfo");
    if OPEN_FAILS.load(Relaxed) {
        ERRNO.store(24, Relaxed); // EMFILE
        return -1;
    }
    OFFSET.store(0, Relaxed);
    OPEN_FDS.fetch_add(1, Relaxed);
    FD
}
unsafe fn read(fd: i32, buf: *mut u8, count: usize) -> isize {
    assert_eq!(fd, FD);
    let text = *CPUINFO.lock().unwrap();
    let offset = OFFSET.load(Relaxed);
    let errno = READ_ERRNO.load(Relaxed);
    if errno != 0 && offset > 0 {
        ERRNO.store(errno, Relaxed);
        return -1;
    }
    // Short reads exercise the streaming parser's record boundaries.
    let n = count.min(7).min(text.len() - offset);
    core::ptr::copy_nonoverlapping(text[offset..].as_ptr(), buf, n);
    OFFSET.store(offset + n, Relaxed);
    n as isize
}
fn close(fd: i32) -> i32 {
    assert_eq!(fd, FD);
    OPEN_FDS.fetch_sub(1, Relaxed);
    0
}
mod syscall {
    pub mod raw {
        const GET: u64 = if cfg!(target_arch = "x86_64") { 204 } else { 123 };
        const SET: u64 = if cfg!(target_arch = "x86_64") { 203 } else { 122 };
        pub unsafe fn syscall3(number: u64, pid: u64, len: u64, pointer: u64) -> u64 {
            assert_eq!(pid, 0);
            let result = crate::RESULT.load(crate::Relaxed);
            if number == GET && result == 8 {
                assert!(len >= 8);
                core::ptr::copy_nonoverlapping(0b1011u64.to_ne_bytes().as_ptr(), pointer as *mut u8, 8);
            }
            if number == SET {
                assert_eq!(*(pointer as *const u64), 0b10);
                return if result < 0 { result as u64 } else { 0 };
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

fn reset(cpuinfo: &'static [u8]) -> std::sync::MutexGuard<'static, ()> {
    let guard = SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    RESULT.store(8, Relaxed);
    OPEN_FAILS.store(false, Relaxed);
    READ_ERRNO.store(0, Relaxed);
    *CPUINFO.lock().unwrap() = cpuinfo;
    ERRNO.store(77, Relaxed);
    guard
}

#[test]
fn byte_count_success_clears_tail_and_errors_preserve_the_set() {
    let _serial = reset(b"");
    let mut set = [0xaau8; 128];
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
}

#[test]
fn setaffinity_returns_the_kernel_status() {
    let _serial = reset(b"");
    let mut set = [0u8; 128];
    set[0] = 0b10;
    RESULT.store(0, Relaxed);
    unsafe { assert_eq!(pthread::affinity::sched_setaffinity(0, set.len(), set.as_ptr()), 0); }
    assert_eq!(ERRNO.load(Relaxed), 77);
    RESULT.store(-22, Relaxed);
    unsafe { assert_eq!(pthread::affinity::sched_setaffinity(0, set.len(), set.as_ptr()), -1); }
    assert_eq!(ERRNO.load(Relaxed), 22);
}

#[test]
fn processor_count_reads_every_cpuinfo_record_and_closes() {
    let _serial = reset(b"processor\t: 0\nBogoMIPS: 1\n\nprocessor\t: 1\n\nprocessor\t: 2\n\nprocessor\t: 3\n");
    assert_eq!(pthread::affinity::processor_count(), 4);
    assert_eq!(ERRNO.load(Relaxed), 77);
    assert_eq!(OPEN_FDS.load(Relaxed), 0);
}

#[test]
fn processor_count_falls_back_to_affinity_then_one_without_errors() {
    let _serial = reset(b"processor\t: 0\n");
    OPEN_FAILS.store(true, Relaxed);
    assert_eq!(pthread::affinity::processor_count(), 3); // the mask 0b1011
    assert_eq!(ERRNO.load(Relaxed), 77);
    OPEN_FAILS.store(false, Relaxed);
    READ_ERRNO.store(EIO, Relaxed);
    assert_eq!(pthread::affinity::processor_count(), 3);
    assert_eq!(ERRNO.load(Relaxed), 77);
    assert_eq!(OPEN_FDS.load(Relaxed), 0);
    READ_ERRNO.store(0, Relaxed);
    *CPUINFO.lock().unwrap() = b"no processors here\n";
    RESULT.store(-38, Relaxed); // ENOSYS
    assert_eq!(pthread::affinity::processor_count(), 1);
    assert_eq!(ERRNO.load(Relaxed), 77);
}

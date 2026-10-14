//! Focused host tests for the clean-room signal-set, signal-name and spin-lock code.
//! Compile directly with rustc --edition=2021 --test; no kernel or libc build.
use core::sync::atomic::{AtomicU32, Ordering::{Acquire, Relaxed, Release}};
const EINVAL: i32 = 22;
const EBUSY: i32 = 16;
unsafe fn word<'a>(p: *const u32) -> &'a AtomicU32 { &*(p as *const AtomicU32) }
// No host syscall emulation: the set/spin tests cannot call these wrappers.
extern crate self as libbreenix;
mod raw {
    pub unsafe fn syscall0(_: u64) -> u64 { panic!("guest syscall in host test") }
    pub unsafe fn syscall2(_: u64, _: u64, _: u64) -> u64 { panic!("guest syscall in host test") }
    pub unsafe fn syscall3(_: u64, _: u64, _: u64, _: u64) -> u64 { panic!("guest syscall in host test") }
}
mod syscall {
    pub mod nr { pub const SIGPENDING: u64 = 136; pub const SIGTIMEDWAIT: u64 = 137; pub const GETPID: u64 = 172; }
}
fn syscall_result_to_c_int(_: i64) -> i32 { panic!("guest result in host test") }
fn getuid() -> u32 { panic!("guest call in host test") }
mod stdio {
    pub mod file {
        pub struct File;
        #[allow(non_upper_case_globals)] // the C stream name stdio exports
        pub static mut stderr: *mut File = core::ptr::null_mut();
        pub unsafe fn lock(_: *mut File) { panic!("guest stdio in host test") }
        pub unsafe fn unlock(_: *mut File) { panic!("guest stdio in host test") }
        pub unsafe fn fputs(_: *const u8, _: *mut File) -> i32 { panic!("guest stdio in host test") }
    }
}
mod pthread {
    pub unsafe fn cancel_syscall(_: u64, _: [u64; 6]) -> i64 { panic!("guest syscall in host test") }
    std::thread_local! { pub static ERRNO: core::cell::Cell<i32> = const { core::cell::Cell::new(0) }; }
    pub fn set_errno(e: i32) { ERRNO.with(|v| v.set(e)); }
}
#[path = "../libs/libbreenix-libc/src/signal.rs"]
mod signal;
#[path = "../libs/libbreenix-libc/src/pthread/spin.rs"]
mod spin;

#[test]
fn signal_sets_preserve_errno_and_bound_every_shift() {
    unsafe {
        let mut set = 0;
        pthread::set_errno(77);
        assert_eq!(signal::sigfillset(&mut set), 0);
        for sig in 1..=64 {
            if sig == signal::CANCEL_SIGNAL { continue; }
            assert_eq!(signal::sigismember(&set, sig), 1);
            assert_eq!(signal::sigdelset(&mut set, sig), 0);
            assert_eq!(signal::sigismember(&set, sig), 0);
            assert_eq!(signal::sigaddset(&mut set, sig), 0);
        }
        assert_eq!(set & signal::CANCEL_MASK, 0);
        assert_eq!(pthread::ERRNO.with(|v| v.get()), 77);
        assert_eq!(signal::sigemptyset(&mut set), 0);
        assert_eq!(set, 0);
        for sig in [i32::MIN, -1, 0, signal::CANCEL_SIGNAL, 65, i32::MAX] {
            assert_eq!(signal::sigaddset(&mut set, sig), -1);
            assert_eq!(signal::sigdelset(&mut set, sig), -1);
            assert_eq!(signal::sigismember(&set, sig), -1);
            assert_eq!(set, 0);
            assert_eq!(pthread::ERRNO.with(|v| v.get()), EINVAL);
        }
    }
}
#[test]
fn spin_contention_publishes_all_protected_writes() {
    struct Protected { lock: AtomicU32, value: core::cell::UnsafeCell<usize> }
    unsafe impl Sync for Protected {}
    let p = std::sync::Arc::new(Protected { lock: AtomicU32::new(0), value: core::cell::UnsafeCell::new(0) });
    let mut handles = Vec::new();
    for _ in 0..4 {
        let p = p.clone();
        handles.push(std::thread::spawn(move || {
            let lock = &p.lock as *const AtomicU32 as *mut i32;
            for _ in 0..10000 { unsafe {
                assert_eq!(spin::pthread_spin_lock(lock), 0);
                *p.value.get() += 1;
                assert_eq!(spin::pthread_spin_trylock(lock), EBUSY);
                assert_eq!(spin::pthread_spin_unlock(lock), 0);
            } }
        }));
    }
    for h in handles { h.join().unwrap(); }
    unsafe { assert_eq!(*p.value.get(), 40000); }
    let lock = &p.lock as *const AtomicU32 as *mut i32;
    unsafe {
        assert_eq!(spin::pthread_spin_destroy(lock), 0);
        assert_eq!(spin::pthread_spin_init(lock, 1), 0);
        assert_eq!(spin::pthread_spin_trylock(lock), 0);
        assert_eq!(spin::pthread_spin_destroy(lock), EBUSY);
        assert_eq!(spin::pthread_spin_unlock(lock), 0);
    }
}

fn name_of(sig: i32) -> Option<String> {
    let mut buf = [0xffu8; 16];
    if unsafe { signal::sig2str(sig, buf.as_mut_ptr()) } != 0 { return None; }
    let end = buf.iter().position(|&b| b == 0).unwrap();
    Some(String::from_utf8(buf[..end].to_vec()).unwrap())
}
fn number_of(name: &str) -> Option<i32> {
    let c = std::ffi::CString::new(name).unwrap();
    let mut sig = -7;
    match unsafe { signal::str2sig(c.as_ptr() as *const u8, &mut sig) } {
        0 => Some(sig),
        r => { assert_eq!((r, sig), (-1, -7)); None }
    }
}
#[test]
fn signal_names_round_trip_and_reserve_the_cancel_signal() {
    for (sig, name) in [(1, "HUP"), (9, "KILL"), (16, "STKFLT"), (29, "IO"), (31, "SYS"), (33, "RTMIN"),
        (34, "RTMIN+1"), (48, "RTMIN+15"), (49, "RTMAX-15"), (63, "RTMAX-1"), (64, "RTMAX")] {
        assert_eq!(name_of(sig).as_deref(), Some(name));
        assert_eq!(number_of(name), Some(sig));
    }
    for sig in (1..=64).filter(|&s| s != signal::CANCEL_SIGNAL) {
        let name = name_of(sig).unwrap();
        assert!(name.len() < 16);
        assert_eq!(number_of(&name), Some(sig));
        assert_eq!(number_of(&sig.to_string()), Some(sig));
    }
    for sig in [i32::MIN, -1, 0, signal::CANCEL_SIGNAL, 65] {
        assert_eq!(name_of(sig), None);
    }
    assert_eq!(number_of("IOT"), Some(6));
    assert_eq!(number_of("CLD"), Some(17));
    assert_eq!(number_of("POLL"), Some(29));
    for bad in ["", "SIGHUP", "hup", "32", "0", "65", "RTMIN+32", "RTMAX-32", "RTMIN+", "RTMIN-1", "RTMAX+1",
        "123", "HUPX"] {
        assert_eq!(number_of(bad), None, "{bad}");
    }
}

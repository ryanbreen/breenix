//! POSIX spin locks. Process sharing needs no kernel state: the lock word is
//! in the memory supplied by the caller and never uses a private futex key.
use super::*;

#[no_mangle]
pub unsafe extern "C" fn pthread_spin_init(p: *mut i32, shared: i32) -> i32 {
    if p.is_null() || !(0..=1).contains(&shared) { return EINVAL; }
    core::ptr::write(p as *mut AtomicU32, AtomicU32::new(0));
    0
}
#[no_mangle]
pub unsafe extern "C" fn pthread_spin_destroy(p: *mut i32) -> i32 {
    if p.is_null() { return EINVAL; }
    if word(p as *const u32).load(Relaxed) != 0 { EBUSY } else { 0 }
}
#[no_mangle]
pub unsafe extern "C" fn pthread_spin_trylock(p: *mut i32) -> i32 {
    if p.is_null() { return EINVAL; }
    if word(p as *const u32).compare_exchange(0, 1, Acquire, Relaxed).is_ok() { 0 } else { EBUSY }
}
#[no_mangle]
pub unsafe extern "C" fn pthread_spin_lock(p: *mut i32) -> i32 {
    if p.is_null() { return EINVAL; }
    let w = word(p as *const u32);
    loop {
        if w.compare_exchange(0, 1, Acquire, Relaxed).is_ok() { return 0; }
        while w.load(Relaxed) != 0 { core::hint::spin_loop(); }
    }
}
#[no_mangle]
pub unsafe extern "C" fn pthread_spin_unlock(p: *mut i32) -> i32 {
    if p.is_null() { return EINVAL; }
    word(p as *const u32).store(0, Release);
    0
}

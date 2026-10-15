//! <threads.h>: ISO C17 7.26 threads, written from the standard over this library's
//! POSIX threads (pthread.rs).
//!
//! thrd_t is pthread_t, mtx_t pthread_mutex_t, cnd_t pthread_cond_t, tss_t
//! pthread_key_t and once_flag pthread_once_t, so each call is the POSIX one with its
//! result mapped to C17's. A C17 start routine returns int where a POSIX one returns a
//! pointer: thrd_create hands the routine and its argument to a trampoline, which
//! returns the int through the pointer-sized result that thrd_join reads back.

use crate::pthread::{
    pthread_cond_broadcast, pthread_cond_destroy, pthread_cond_init, pthread_cond_signal,
    pthread_cond_timedwait, pthread_cond_wait, pthread_create, pthread_detach, pthread_equal,
    pthread_exit, pthread_getspecific, pthread_join, pthread_key_create, pthread_key_delete,
    pthread_mutex_destroy, pthread_mutex_init, pthread_mutex_lock, pthread_mutex_timedlock,
    pthread_mutex_trylock, pthread_mutex_unlock, pthread_mutexattr_destroy, pthread_mutexattr_init,
    pthread_mutexattr_settype, pthread_once, pthread_self, pthread_setspecific,
};
use core::ptr::{null, null_mut};

const THRD_SUCCESS: i32 = 0;
const THRD_BUSY: i32 = 1;
const THRD_ERROR: i32 = 2;
const THRD_NOMEM: i32 = 3;
const THRD_TIMEDOUT: i32 = 4;

const MTX_PLAIN: i32 = 0;
const MTX_RECURSIVE: i32 = 1;
const MTX_TIMED: i32 = 2;

const PTHREAD_MUTEX_RECURSIVE: i32 = 1;

const EAGAIN: i32 = 11;
const ENOMEM: i32 = 12;
const EBUSY: i32 = 16;
const EINTR: i32 = 4;
const ETIMEDOUT: i32 = 110;

type ThrdStart = extern "C" fn(*mut u8) -> i32;

/// What thrd_create hands its trampoline, in memory from malloc that the new thread frees.
#[derive(Clone, Copy)]
struct Start {
    func: ThrdStart,
    arg: *mut u8,
}

extern "C" fn trampoline(p: *mut u8) -> *mut u8 {
    // SAFETY: thrd_create passes a malloc'd Start that only this thread uses.
    let start = unsafe { *(p as *const Start) };
    unsafe { crate::free(p) };
    (start.func)(start.arg) as isize as *mut u8
}

fn success_or_error(e: i32) -> i32 {
    if e == 0 { THRD_SUCCESS } else { THRD_ERROR }
}

#[no_mangle]
pub unsafe extern "C" fn thrd_create(thr: *mut usize, func: Option<ThrdStart>, arg: *mut u8) -> i32 {
    let Some(func) = func else { return THRD_ERROR };
    let p = crate::malloc(core::mem::size_of::<Start>()) as *mut Start;
    if p.is_null() {
        return THRD_NOMEM;
    }
    p.write(Start { func, arg });
    match pthread_create(thr, null(), trampoline, p as *mut u8) {
        0 => THRD_SUCCESS,
        e => {
            crate::free(p as *mut u8);
            if e == ENOMEM || e == EAGAIN { THRD_NOMEM } else { THRD_ERROR }
        }
    }
}

#[no_mangle]
pub extern "C" fn thrd_current() -> usize {
    pthread_self()
}

#[no_mangle]
pub unsafe extern "C" fn thrd_detach(thr: usize) -> i32 {
    success_or_error(pthread_detach(thr))
}

#[no_mangle]
pub extern "C" fn thrd_equal(a: usize, b: usize) -> i32 {
    pthread_equal(a, b)
}

#[no_mangle]
pub unsafe extern "C" fn thrd_exit(res: i32) -> ! {
    pthread_exit(res as isize as *mut u8)
}

#[no_mangle]
pub unsafe extern "C" fn thrd_join(thr: usize, res: *mut i32) -> i32 {
    let mut value = null_mut();
    if pthread_join(thr, &mut value) != 0 {
        return THRD_ERROR;
    }
    if !res.is_null() {
        *res = value as isize as i32;
    }
    THRD_SUCCESS
}

/// 0 once the time has passed, -1 when a signal cut the sleep short (with the time left
/// in `remaining`), and -2 when the request was invalid.
#[no_mangle]
pub unsafe extern "C" fn thrd_sleep(duration: *const u8, remaining: *mut u8) -> i32 {
    if crate::nanosleep(duration, remaining) == 0 {
        0
    } else if crate::pthread::errno() == EINTR {
        -1
    } else {
        -2
    }
}

#[no_mangle]
pub extern "C" fn thrd_yield() {
    crate::sched_yield();
}

#[no_mangle]
pub unsafe extern "C" fn mtx_init(mtx: *mut u8, kind: i32) -> i32 {
    match kind {
        MTX_PLAIN | MTX_TIMED => success_or_error(pthread_mutex_init(mtx, null())),
        k if k == MTX_PLAIN | MTX_RECURSIVE || k == MTX_TIMED | MTX_RECURSIVE => {
            // pthread_mutexattr_t: four bytes, eight-aligned.
            let mut attr = 0u64;
            let a = &mut attr as *mut u64 as *mut u8;
            if pthread_mutexattr_init(a) != 0 {
                return THRD_ERROR;
            }
            let e = if pthread_mutexattr_settype(a, PTHREAD_MUTEX_RECURSIVE) == 0 {
                pthread_mutex_init(mtx, a)
            } else {
                -1
            };
            pthread_mutexattr_destroy(a);
            success_or_error(e)
        }
        _ => THRD_ERROR,
    }
}

#[no_mangle]
pub unsafe extern "C" fn mtx_destroy(mtx: *mut u8) {
    pthread_mutex_destroy(mtx);
}

#[no_mangle]
pub unsafe extern "C" fn mtx_lock(mtx: *mut u8) -> i32 {
    success_or_error(pthread_mutex_lock(mtx))
}

#[no_mangle]
pub unsafe extern "C" fn mtx_timedlock(mtx: *mut u8, ts: *const i64) -> i32 {
    match pthread_mutex_timedlock(mtx, ts) {
        0 => THRD_SUCCESS,
        ETIMEDOUT => THRD_TIMEDOUT,
        _ => THRD_ERROR,
    }
}

#[no_mangle]
pub unsafe extern "C" fn mtx_trylock(mtx: *mut u8) -> i32 {
    match pthread_mutex_trylock(mtx) {
        0 => THRD_SUCCESS,
        EBUSY => THRD_BUSY,
        _ => THRD_ERROR,
    }
}

#[no_mangle]
pub unsafe extern "C" fn mtx_unlock(mtx: *mut u8) -> i32 {
    success_or_error(pthread_mutex_unlock(mtx))
}

#[no_mangle]
pub unsafe extern "C" fn cnd_init(cond: *mut u8) -> i32 {
    match pthread_cond_init(cond, null()) {
        0 => THRD_SUCCESS,
        ENOMEM => THRD_NOMEM,
        _ => THRD_ERROR,
    }
}

#[no_mangle]
pub unsafe extern "C" fn cnd_destroy(cond: *mut u8) {
    pthread_cond_destroy(cond);
}

#[no_mangle]
pub unsafe extern "C" fn cnd_signal(cond: *mut u8) -> i32 {
    success_or_error(pthread_cond_signal(cond))
}

#[no_mangle]
pub unsafe extern "C" fn cnd_broadcast(cond: *mut u8) -> i32 {
    success_or_error(pthread_cond_broadcast(cond))
}

#[no_mangle]
pub unsafe extern "C" fn cnd_wait(cond: *mut u8, mtx: *mut u8) -> i32 {
    success_or_error(pthread_cond_wait(cond, mtx))
}

#[no_mangle]
pub unsafe extern "C" fn cnd_timedwait(cond: *mut u8, mtx: *mut u8, ts: *const i64) -> i32 {
    match pthread_cond_timedwait(cond, mtx, ts) {
        0 => THRD_SUCCESS,
        ETIMEDOUT => THRD_TIMEDOUT,
        _ => THRD_ERROR,
    }
}

#[no_mangle]
pub unsafe extern "C" fn tss_create(key: *mut u32, dtor: Option<unsafe extern "C" fn(*mut u8)>) -> i32 {
    success_or_error(pthread_key_create(key, dtor))
}

#[no_mangle]
pub unsafe extern "C" fn tss_delete(key: u32) {
    pthread_key_delete(key);
}

#[no_mangle]
pub unsafe extern "C" fn tss_get(key: u32) -> *mut u8 {
    pthread_getspecific(key)
}

#[no_mangle]
pub unsafe extern "C" fn tss_set(key: u32, value: *mut u8) -> i32 {
    success_or_error(pthread_setspecific(key, value))
}

#[no_mangle]
pub unsafe extern "C" fn call_once(flag: *mut i32, func: extern "C" fn()) {
    pthread_once(flag, func);
}

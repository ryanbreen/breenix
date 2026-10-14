//! Signal set operations, using Breenix's 64-signal Linux kernel ABI.
use crate::pthread::set_errno;

pub(crate) const CANCEL_SIGNAL: i32 = 32;
pub(crate) const CANCEL_MASK: u64 = 1 << (CANCEL_SIGNAL - 1);

fn invalid() -> i32 { set_errno(22); -1 }
fn bit(sig: i32) -> Option<u64> {
    if (1..=64).contains(&sig) && sig != CANCEL_SIGNAL {
        Some(1u64 << (sig - 1))
    } else { None }
}
#[no_mangle]
pub unsafe extern "C" fn sigemptyset(set: *mut u64) -> i32 {
    if set.is_null() { return invalid(); }
    *set = 0;
    0
}
#[no_mangle]
pub unsafe extern "C" fn sigfillset(set: *mut u64) -> i32 {
    if set.is_null() { return invalid(); }
    *set = !CANCEL_MASK;
    0
}
#[no_mangle]
pub unsafe extern "C" fn sigaddset(set: *mut u64, sig: i32) -> i32 {
    let Some(b) = bit(sig) else { return invalid(); };
    if set.is_null() { return invalid(); }
    *set |= b;
    0
}
#[no_mangle]
pub unsafe extern "C" fn sigdelset(set: *mut u64, sig: i32) -> i32 {
    let Some(b) = bit(sig) else { return invalid(); };
    if set.is_null() { return invalid(); }
    *set &= !b;
    0
}
#[no_mangle]
pub unsafe extern "C" fn sigismember(set: *const u64, sig: i32) -> i32 {
    let Some(b) = bit(sig) else { return invalid(); };
    if set.is_null() { return invalid(); }
    ((*set & b) != 0) as i32
}

#[no_mangle]
pub unsafe extern "C" fn sigpending(set: *mut u64) -> i32 {
    if set.is_null() { return invalid(); }
    let r = libbreenix::raw::syscall2(libbreenix::syscall::nr::SIGPENDING, set as u64, 8) as i64;
    if r >= 0 { *set &= !CANCEL_MASK; }
    crate::syscall_result_to_c_int(r)
}
#[no_mangle]
pub unsafe extern "C" fn sigtimedwait(set: *const u64, info: *mut u8, timeout: *const i64) -> i32 {
    if set.is_null() { return invalid(); }
    let mask = *set & !CANCEL_MASK;
    crate::syscall_result_to_c_int(crate::pthread::cancel_syscall(libbreenix::syscall::nr::SIGTIMEDWAIT,
        [&mask as *const u64 as u64, info as u64, timeout as u64, 8, 0, 0]))
}
#[no_mangle]
pub unsafe extern "C" fn sigwaitinfo(set: *const u64, info: *mut u8) -> i32 {
    sigtimedwait(set, info, core::ptr::null())
}

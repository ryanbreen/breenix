//! POSIX.1-2024 clock_getres and clock_settime for <time.h>. The syscall numbers
//! are the Linux UAPI ones the Breenix kernel dispatches on.
use super::set_errno;
use libbreenix::{raw::syscall2, syscall::nr};

#[cfg(target_arch = "aarch64")]
const CLOCK_GETRES: u64 = 114;
#[cfg(target_arch = "x86_64")]
const CLOCK_GETRES: u64 = 229;

fn result(r: i64) -> i32 {
    if r < 0 {
        set_errno(-r as i32);
        -1
    } else {
        0
    }
}
/// A null res only validates the clock.
#[no_mangle]
pub unsafe extern "C" fn clock_getres(clock: i32, res: *mut u8) -> i32 {
    result(syscall2(CLOCK_GETRES, clock as u64, res as u64) as i64)
}
#[no_mangle]
pub unsafe extern "C" fn clock_settime(clock: i32, tp: *const u8) -> i32 {
    result(syscall2(nr::CLOCK_SETTIME, clock as u64, tp as u64) as i64)
}

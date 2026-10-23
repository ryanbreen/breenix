//! Suite-mode test controls: system call 415 (`docs/gui/breengel.md` section 21). Enabled
//! only in boots whose PID 1 is a suite binary, and only for root; ENOSYS everywhere else.

use breengel_abi::{tc_op, Plain, TcArm, TcFired, SYS_TEST_CONTROL};

use crate::error::Error;
use crate::syscall::raw;

/// `test_control(op, arg, len)`: the kernel's return, a value or a negated errno.
///
/// # Safety
/// `arg` must be null or point to `len` bytes the kernel may read and write.
pub unsafe fn raw_call(op: u32, arg: *mut u8, len: usize) -> i64 {
    raw::syscall3(SYS_TEST_CONTROL, op as u64, arg as u64, len as u64) as i64
}

/// `STATE`: 1 when test controls are enabled in this boot.
pub fn state() -> Result<u64, Error> {
    // SAFETY: STATE takes no argument.
    Error::from_syscall(unsafe { raw_call(tc_op::STATE, core::ptr::null_mut(), 0) })
}

/// `ARM` one one-shot fault at `point` for `target` (0 for any) with `param`.
pub fn arm(point: u32, target: u64, param: u64) -> Result<u64, Error> {
    let mut a = TcArm { point, reserved: 0, target, param };
    // SAFETY: `a` is a live TcArm of its own size.
    Error::from_syscall(unsafe {
        raw_call(tc_op::ARM, &mut a as *mut TcArm as *mut u8, core::mem::size_of::<TcArm>())
    })
}

/// `DISARM_ALL`.
pub fn disarm_all() -> Result<u64, Error> {
    // SAFETY: DISARM_ALL takes no argument.
    Error::from_syscall(unsafe { raw_call(tc_op::DISARM_ALL, core::ptr::null_mut(), 0) })
}

/// `FIRED`: how many times `point` has fired this boot.
pub fn fired(point: u32) -> Result<u64, Error> {
    let mut f = TcFired::ZERO;
    f.point = point;
    // SAFETY: `f` is a live TcFired of its own size.
    Error::from_syscall(unsafe {
        raw_call(tc_op::FIRED, &mut f as *mut TcFired as *mut u8, core::mem::size_of::<TcFired>())
    })?;
    Ok(f.count)
}

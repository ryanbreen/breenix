//! Suite-mode test controls: system call 415 (`docs/gui/gpu.md` section 1.8, which takes
//! the latch and ops from BreenGL's contract, section 21).
//!
//! The controls exist in the production kernel and are reachable only in a suite boot: the
//! kernel latches them on when the first program it designates as PID 1 is a suite binary
//! (`/sbin/suite-*`), before any user code runs, and nothing changes the latch afterwards.
//! In every other boot each op is ENOSYS; in a suite boot only root may use them (EPERM).
//! The latch and the caller are checked before an argument is decoded.
//!
//! The points defined here are the GPU effort's, 19 to 31. Each is a branch in the real
//! code path that asks `take` (or, for a mask that holds while armed, `peek`) whether its
//! point is armed; both read the latch first, so outside a suite boot a point costs one
//! load and can never fire. Arming is one-shot unless the point's own documentation says
//! it holds until `DISARM_ALL`.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use gpu_abi::{test_control as tc_op, test_point, Plain, TcArm, TcFired};

use crate::syscall::errno::{EFAULT, EINVAL, ENOSYS, EPERM};
use crate::syscall::SyscallResult;

/// Whether test controls are enabled for this boot. Written once, by `latch_init_program`.
static ENABLED: AtomicBool = AtomicBool::new(false);
/// Whether the latch has been decided. The first designation decides it for the boot.
static DECIDED: AtomicBool = AtomicBool::new(false);

/// The path prefix of a suite binary.
const SUITE_PREFIX: &[u8] = b"/sbin/suite-";

/// Decide the latch from the program the kernel designated as PID 1. Called from
/// `ProcessManager::designate_init`, which succeeds once per boot; only the first call
/// decides.
pub fn latch_init_program(exe_path: &[u8]) {
    if DECIDED
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
    {
        ENABLED.store(exe_path.starts_with(SUITE_PREFIX), Ordering::Release);
    }
}

/// Whether test controls are enabled in this boot.
#[inline]
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Acquire)
}

const POINTS: usize = test_point::LAST as usize + 1;

/// Whether `point` is one this kernel defines.
fn defined(point: u32) -> bool {
    (test_point::FIRST..=test_point::LAST).contains(&point)
}

struct Point {
    armed: AtomicBool,
    target: AtomicU64,
    param: AtomicU64,
    fired: AtomicU64,
}

impl Point {
    const fn new() -> Point {
        Point {
            armed: AtomicBool::new(false),
            target: AtomicU64::new(0),
            param: AtomicU64::new(0),
            fired: AtomicU64::new(0),
        }
    }
}

static POINT_TABLE: [Point; POINTS] = [const { Point::new() }; POINTS];

/// Fire `point` if it is armed for `target` (or armed with target 0, which matches every
/// target): disarm it, count the firing and return its parameter. `None` outside a suite
/// boot and whenever the point is not armed.
#[inline]
pub fn take(point: u32, target: u64) -> Option<u64> {
    if !enabled() {
        return None;
    }
    let p = POINT_TABLE.get(point as usize)?;
    if !p.armed.load(Ordering::Acquire) {
        return None;
    }
    let want = p.target.load(Ordering::Acquire);
    if want != 0 && want != target {
        return None;
    }
    if p
        .armed
        .compare_exchange(true, false, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return None;
    }
    p.fired.fetch_add(1, Ordering::AcqRel);
    Some(p.param.load(Ordering::Acquire))
}

/// The parameter of `point` while it is armed, without disarming it: for a point that
/// holds a condition until `DISARM_ALL` (`mask-capability`). The caller counts each
/// effect with [`count_fired`].
#[inline]
pub fn peek(point: u32) -> Option<u64> {
    if !enabled() {
        return None;
    }
    let p = POINT_TABLE.get(point as usize)?;
    p.armed
        .load(Ordering::Acquire)
        .then(|| p.param.load(Ordering::Acquire))
}

/// Count one effect of a held point.
pub fn count_fired(point: u32) {
    if let Some(p) = POINT_TABLE.get(point as usize) {
        p.fired.fetch_add(1, Ordering::AcqRel);
    }
}

fn caller_is_root() -> bool {
    let Some(tid) = crate::task::scheduler::current_thread_id() else {
        return false;
    };
    crate::process::with_process_manager(|manager| {
        manager
            .find_process_by_thread(tid)
            .is_some_and(|(_, process)| process.cred.euid == 0)
    })
    .unwrap_or(false)
}

fn err(errno: i32) -> SyscallResult {
    SyscallResult::Err(errno as u64)
}

fn read_arg<T: Plain>(arg: u64, len: u64) -> Result<T, i32> {
    if len != core::mem::size_of::<T>() as u64 {
        return Err(EINVAL);
    }
    crate::syscall::userptr::copy_from_user(arg as *const T).map_err(|_| EFAULT)
}

/// `test_control(op, arg, len)`.
pub fn sys_test_control(op: u64, arg: u64, len: u64) -> SyscallResult {
    if !enabled() {
        return err(ENOSYS);
    }
    if !caller_is_root() {
        return err(EPERM);
    }
    let Ok(op) = u32::try_from(op) else {
        return err(EINVAL);
    };
    match op {
        tc_op::STATE => SyscallResult::Ok(1),
        tc_op::ARM => {
            let a: TcArm = match read_arg(arg, len) {
                Ok(a) => a,
                Err(e) => return err(e),
            };
            if a.reserved != 0 || !defined(a.point) {
                return err(EINVAL);
            }
            let p = &POINT_TABLE[a.point as usize];
            p.armed.store(false, Ordering::Release);
            p.target.store(a.target, Ordering::Release);
            p.param.store(a.param, Ordering::Release);
            p.armed.store(true, Ordering::Release);
            SyscallResult::Ok(0)
        }
        tc_op::DISARM_ALL => {
            for p in POINT_TABLE.iter() {
                p.armed.store(false, Ordering::Release);
            }
            SyscallResult::Ok(0)
        }
        tc_op::FIRED => {
            let mut f: TcFired = match read_arg(arg, len) {
                Ok(f) => f,
                Err(e) => return err(e),
            };
            if f.reserved != 0 || !defined(f.point) {
                return err(EINVAL);
            }
            f.count = POINT_TABLE[f.point as usize].fired.load(Ordering::Acquire);
            match crate::syscall::userptr::copy_to_user(arg as *mut TcFired, &f) {
                Ok(()) => SyscallResult::Ok(0),
                Err(_) => err(EFAULT),
            }
        }
        // There is no op that sets or clears the latch: 4 and everything above.
        _ => err(EINVAL),
    }
}

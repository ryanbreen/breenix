//! Suite-mode test controls: system call 415 (`docs/gui/breengel.md` section 21).
//!
//! The controls exist in the production kernel and are reachable only in a suite boot: the
//! kernel latches them on when the first program it runs as PID 1 is a suite binary
//! (`/sbin/suite-*`), at the designation of that PID 1, before any user code runs. Nothing
//! changes the latch afterwards. In every other boot each op is ENOSYS; in a suite boot only
//! root may use them.
//!
//! Each fault point is a branch in the real code path that asks `take` whether its point is
//! armed. `take` reads the latch first, so outside a suite boot a fault point costs one
//! relaxed load and can never fire. Every arming is one-shot.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use breengel_abi::{tc_op, tc_point, Plain, TcArm, TcFired};

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

const POINTS: usize = tc_point::LAST as usize + 1;

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

/// Whether `point` is armed now, without firing it. For a point another point resumes
/// (`release-stall` ends an `acquire-stall` pause).
#[inline]
pub fn armed(point: u32) -> bool {
    enabled()
        && POINT_TABLE
            .get(point as usize)
            .is_some_and(|p| p.armed.load(Ordering::Acquire))
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
            if a.reserved != 0 || a.point == 0 || a.point > tc_point::LAST {
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
            if f.reserved != 0 || f.point == 0 || f.point > tc_point::LAST {
                return err(EINVAL);
            }
            f.count = POINT_TABLE[f.point as usize].fired.load(Ordering::Acquire);
            match crate::syscall::userptr::copy_to_user(arg as *mut TcFired, &f) {
                Ok(()) => SyscallResult::Ok(0),
                Err(_) => err(EFAULT),
            }
        }
        // SET_LATCH (4) and everything above: the latch has no setter.
        _ => err(EINVAL),
    }
}

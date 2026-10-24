//! Suite-mode test controls: system call 415 (`docs/gui/gpu.md` section 1.8, which takes
//! the latch and ops from BreenGL's contract, section 21).
//!
//! The controls exist in the production kernel and are reachable only in a suite boot: the
//! kernel latches them on when the first program it designates as PID 1 is a suite binary
//! (`/sbin/suite-*`), before any user code runs, and nothing changes the latch afterwards.
//! In every other boot each op is ENOSYS; in a suite boot only root may use them (EPERM).
//! The latch and the caller are checked before an argument is decoded.
//!
//! The points defined here are the GPU effort's, 19 to 31, and BreenGL's, 1 to 18
//! (`breengel_abi::tc_point`); the GPU core builds BreenGL's 2 `irq-hold` and 3
//! `emit-diagnostic` too (`docs/gui/gpu-abi.md`), plus users point 32 `cred-orphan`. Each is a branch in the real
//! code path that asks `take` (or `take_if`) whether its point is armed; both read the
//! latch first, so outside a suite boot a point costs one load and cannot fire while
//! disabled. A point is one-shot: it affects the next occurrence of its event, counts one firing and
//! disarms itself. Fallible submission preparation reserves points 22 and 28, counting
//! them only when queued and restoring their arm when preparation fails.

use core::sync::atomic::{AtomicBool, Ordering};

use gpu_abi::{test_control as tc_op, test_point, Plain, TcArm, TcFired};

use crate::syscall::errno::{EFAULT, EINVAL, ENOSYS, EPERM};
use crate::syscall::SyscallResult;

/// Whether test controls are enabled for this boot. Written once, by `latch_init_program`.
static ENABLED: AtomicBool = AtomicBool::new(false);
/// Whether the latch has been decided. The first designation decides it for the boot.
static DECIDED: AtomicBool = AtomicBool::new(false);

/// The path prefix of a suite binary.
const SUITE_PREFIX: &[u8] = b"/sbin/suite-";

/// Decide the latch from the program the ARM64 init launcher has designated as PID 1,
/// before publishing its thread. The first call decides the latch for the boot.
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

/// The users effort's point, armed with a thread ID: that thread's next credential
/// lookup (`fs::permissions::Credentials::current`) finds no process.
pub const CRED_ORPHAN: u32 = test_point::CRED_ORPHAN;

/// Whether `point` is defined by BreenGL, the GPU effort or the users effort.
fn defined(point: u32) -> bool {
    breengel_abi::tc_point::defined(point)
        || test_point::defined(point)
}
const POINTS: usize = if breengel_abi::tc_point::LAST > CRED_ORPHAN {
    breengel_abi::tc_point::LAST as usize + 1
} else {
    CRED_ORPHAN as usize + 1
};

mod point;
use point::Point;
pub use point::Reservation;

static POINT_TABLE: [Point; POINTS] = [const { Point::new() }; POINTS];

/// Reserve a one-shot while fallible preparation runs. Dropping it leaves it armed;
/// `fire` commits the firing at the point's effect: validator refusal for point 24,
/// queued device work for points 22 and 28. These reserve across fallible preparation;
/// device points reserve in that order and outside the core lock.
pub fn reserve(point: u32, target: u64) -> Option<Reservation<'static>> {
    if !enabled() { return None; }
    POINT_TABLE.get(point as usize)?.reserve(target, |_| true)
}

/// Fire a matching one-shot immediately at its event.
#[inline]
pub fn take(point: u32, target: u64) -> Option<u64> {
    take_if(point, target, |_| true)
}

#[inline]
pub fn take_if(point: u32, target: u64, wanted: impl Fn(u64) -> bool) -> Option<u64> {
    if !enabled() { return None; }
    Some(POINT_TABLE.get(point as usize)?.reserve(target, wanted)?.fire())
}

/// `emit-diagnostic` (section 22): write the fixed 32-byte payload once through writer `path`
/// (`breengel_abi::emit_path`). The window system call calls this when the point fires for the
/// calling thread; an unknown path writes nothing.
pub fn emit_diagnostic(path: u64) {
    use breengel_abi::{emit_path as p, EMIT_DIAGNOSTIC_PAYLOAD as PAYLOAD};
    use crate::serial_writer::Writer;
    let Ok(text) = core::str::from_utf8(PAYLOAD) else {
        return;
    };
    match path {
        p::SERIAL_PRINTLN => crate::serial_println!("{}", text),
        p::LOG_WARN => log::warn!(target: "test_control", "{}", text),
        p::WRITE_BYTE => {
            for &byte in PAYLOAD.iter() {
                crate::serial::write_byte(Writer::Kernel, byte);
            }
        }
        p::WRITE_BYTES_ATOMIC => crate::serial::write_bytes_atomic(Writer::Kernel, PAYLOAD),
        p::TRY_PRINT => {
            let _ = crate::serial::try_print(format_args!("{}", text));
        }
        p::WRITE_STR_BOUNDED => crate::serial::write_str_bounded(text),
        p::LOG_PRINT => crate::serial::_log_print(format_args!("{}", text)),
        p::NOTICE => {
            // Posted here and written out by whichever thread writes output next.
            static NODE: crate::log_notice::Node = crate::log_notice::Node::new();
            NODE.post_again(text);
        }
        p::EMERGENCY_PRINT => {
            let _ = crate::serial::emergency_print(format_args!("{}", text));
        }
        _ => {}
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
            p.arm(a.target, a.param);
            SyscallResult::Ok(0)
        }
        tc_op::DISARM_ALL => {
            for p in POINT_TABLE.iter() {
                p.disarm();
            }
            #[cfg(target_arch = "aarch64")]
            // A case's measurements start clean: the IRQ-masked gauges restart at 0.
            crate::tracing::providers::gpu::reset_irq_masked();
            crate::tracing::providers::window::reset_irq_masked();
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
            f.count = POINT_TABLE[f.point as usize].fired();
            match crate::syscall::userptr::copy_to_user(arg as *mut TcFired, &f) {
                Ok(()) => SyscallResult::Ok(0),
                Err(_) => err(EFAULT),
            }
        }
        // There is no op that sets or clears the latch: 4 and everything above.
        _ => err(EINVAL),
    }
}

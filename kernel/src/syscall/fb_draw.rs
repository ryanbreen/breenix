//! `fb_draw` (411): the operation table. `FLUSH` and `FLUSH_BATCH` are carried out in `fb.rs`,
//! the version 1 window ops (10 to 26 and 31) in `window_v1.rs`.

#[cfg(all(target_arch = "aarch64", feature = "boot_tests"))]
use super::fb::USER_SPACE_MAX;
use super::SyscallResult;

/// Dedicated F32c waitqueue race reproducer. These test-only FBDRAW ops (27 to 30) let a
/// userspace harness drive wait/wake races without involving BWM state. They exist only in
/// `boot_tests` kernels; the production kernel answers ENOSYS (docs/gui/breengel.md 17.6).
#[cfg(all(target_arch = "aarch64", feature = "boot_tests"))]
static WAIT_STRESS_WQ: crate::task::waitqueue::WaitQueueHead =
    crate::task::waitqueue::WaitQueueHead::new();
#[cfg(all(target_arch = "aarch64", feature = "boot_tests"))]
static WAIT_STRESS_ENTERED: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
#[cfg(all(target_arch = "aarch64", feature = "boot_tests"))]
static WAIT_STRESS_RETURNED: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
#[cfg(all(target_arch = "aarch64", feature = "boot_tests"))]
static WAIT_STRESS_WAKES: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

#[cfg(all(target_arch = "aarch64", feature = "boot_tests"))]
fn handle_wait_stress_reset() -> SyscallResult {
    use core::sync::atomic::Ordering;

    WAIT_STRESS_WQ.wake_up();
    WAIT_STRESS_ENTERED.store(0, Ordering::SeqCst);
    WAIT_STRESS_RETURNED.store(0, Ordering::SeqCst);
    WAIT_STRESS_WAKES.store(0, Ordering::SeqCst);
    SyscallResult::Ok(0)
}

#[cfg(all(target_arch = "aarch64", feature = "boot_tests"))]
fn handle_wait_stress_wait() -> SyscallResult {
    use core::sync::atomic::Ordering;

    let entered = WAIT_STRESS_ENTERED.fetch_add(1, Ordering::SeqCst) + 1;
    if WAIT_STRESS_WQ
        .prepare_to_wait(crate::task::thread::ThreadState::BlockedOnIO)
        .is_none()
    {
        return SyscallResult::Err(super::ErrorCode::InvalidArgument as u64);
    }

    crate::task::waitqueue::schedule_current_wait();
    WAIT_STRESS_WQ.finish_wait();
    super::window_v1::ensure_current_address_space();

    WAIT_STRESS_RETURNED.fetch_add(1, Ordering::SeqCst);
    SyscallResult::Ok(entered)
}

#[cfg(all(target_arch = "aarch64", feature = "boot_tests"))]
fn handle_wait_stress_wake() -> SyscallResult {
    use core::sync::atomic::Ordering;

    let wakes = WAIT_STRESS_WAKES.fetch_add(1, Ordering::SeqCst) + 1;
    WAIT_STRESS_WQ.wake_up();
    SyscallResult::Ok(wakes)
}

#[cfg(all(target_arch = "aarch64", feature = "boot_tests"))]
fn handle_wait_stress_stats(cmd: &FbDrawCmd) -> SyscallResult {
    use core::sync::atomic::Ordering;

    let out_ptr = (cmd.p1 as u32 as u64) | ((cmd.p2 as u32 as u64) << 32);
    let out_len = cmd.p3 as u32;
    if out_ptr == 0 || out_ptr >= USER_SPACE_MAX || out_len < 4 {
        return SyscallResult::Err(super::ErrorCode::InvalidArgument as u64);
    }

    let out_end = out_ptr + 4 * core::mem::size_of::<u64>() as u64;
    if out_end > USER_SPACE_MAX {
        return SyscallResult::Err(super::ErrorCode::Fault as u64);
    }

    let waiters = if WAIT_STRESS_WQ.has_waiters() { 1 } else { 0 };
    unsafe {
        let dst = out_ptr as *mut u64;
        core::ptr::write(dst.add(0), WAIT_STRESS_ENTERED.load(Ordering::SeqCst));
        core::ptr::write(dst.add(1), WAIT_STRESS_RETURNED.load(Ordering::SeqCst));
        core::ptr::write(dst.add(2), WAIT_STRESS_WAKES.load(Ordering::SeqCst));
        core::ptr::write(dst.add(3), waiters);
    }

    SyscallResult::Ok(0)
}

// =============================================================================
// Window Buffer Registry — kernel-side window management for GPU compositing
// Only compiled for ARM64 (Parallels VirGL compositor path).
// =============================================================================

/// Draw command operations for sys_fbdraw
/// The operations of `fb_draw` (411) the production interface keeps (docs/gui/breengel.md
/// section 17.6). Ops 0 to 5, 7 and 9 were demo draws and VirGL submissions and are ENOSYS;
/// ops 10 to 26 and 31 are the version 1 window interface (`window_v1.rs`).
#[repr(u32)]
pub enum FbDrawOp {
    /// Flush the caller's fb_mmap pane, or the rectangle p1-p4 of it, to the screen
    Flush = 6,
    /// Batch flush multiple dirty rects with one DSB barrier
    FlushBatch = 8,
}

/// Draw command structure passed from userspace.
/// Must match the FbDrawCmd struct in libbreenix.
#[derive(Clone, Copy)]
#[repr(C)]
pub struct FbDrawCmd {
    /// Operation code (FbDrawOp)
    pub op: u32,
    /// First parameter (x, cx, x1, or unused)
    pub p1: i32,
    /// Second parameter (y, cy, y1, or unused)
    pub p2: i32,
    /// Third parameter (width, radius, x2, or unused)
    pub p3: i32,
    /// Fourth parameter (height, y2, or unused)
    pub p4: i32,
    /// Color as packed RGB (0x00RRGGBB)
    pub color: u32,
}

/// sys_fbdraw - the framebuffer operations: `FLUSH` and `FLUSH_BATCH`, the version 1 window
/// ops, and the wait-stress ops of `boot_tests` kernels.
///
/// # Arguments
/// * `cmd_ptr` - Pointer to userspace FbDrawCmd structure
///
/// # Returns
/// * 0 on success
/// * -EFAULT if cmd_ptr is invalid
/// * -ENODEV if no framebuffer is available
/// * -EINVAL if operation is invalid
#[cfg(any(target_arch = "aarch64", feature = "interactive"))]
pub fn sys_fbdraw(cmd_ptr: u64) -> SyscallResult {
    let cmd: FbDrawCmd = match crate::syscall::userptr::copy_from_user(cmd_ptr as *const FbDrawCmd)
    {
        Ok(cmd) => cmd,
        Err(_) => return SyscallResult::Err(super::ErrorCode::Fault as u64),
    };
    match cmd.op {
        0..=5 | 7 | 9 => return SyscallResult::Err(super::errno::ENOSYS as u64),
        27..=30 => return wait_stress(&cmd),
        #[cfg(target_arch = "aarch64")]
        op if op >= 10 => return super::window_v1::handle_op(&cmd),
        _ => {}
    }

    super::fb::framebuffer_op(cmd)
}

/// The F32c wait-stress ops (27 to 30), present only in `boot_tests` kernels.
#[cfg(any(target_arch = "aarch64", feature = "interactive"))]
fn wait_stress(cmd: &FbDrawCmd) -> SyscallResult {
    #[cfg(all(target_arch = "aarch64", feature = "boot_tests"))]
    match cmd.op {
        27 => handle_wait_stress_reset(),
        // A wait with no persistent condition: a wake after prepare_to_wait must make
        // schedule a no-op, matching the Linux waitqueue race-closure invariant.
        28 => handle_wait_stress_wait(),
        29 => handle_wait_stress_wake(),
        _ => handle_wait_stress_stats(cmd),
    }
    #[cfg(not(all(target_arch = "aarch64", feature = "boot_tests")))]
    {
        let _ = cmd;
        SyscallResult::Err(super::errno::ENOSYS as u64)
    }
}

/// sys_fbdraw - production x86_64 form. Only Flush (op 6) is supported: it copies
/// the display owner's whole-screen fb_mmap pane (or the rectangle p1-p4 of it,
/// when p3 and p4 are non-zero) onto the screen. The drawing ops draw on the
/// interactive kernel's shell framebuffer, which this kernel does not have.
///
/// # Returns
/// * 0 on success
/// * -EFAULT if cmd_ptr is invalid
/// * -ENOSYS for the retired demo, VirGL and wait-stress ops
/// * -EINVAL for any other op but Flush, or a caller with no fb_mmap pane
/// * -EPERM if the caller does not own the display
/// * -EBUSY if the kernel's log sink holds the framebuffer (flush again)
#[cfg(all(target_arch = "x86_64", not(feature = "interactive")))]
pub fn sys_fbdraw(cmd_ptr: u64) -> SyscallResult {
    let cmd: FbDrawCmd = match crate::syscall::userptr::copy_from_user(cmd_ptr as *const FbDrawCmd)
    {
        Ok(cmd) => cmd,
        Err(_) => return SyscallResult::Err(super::ErrorCode::Fault as u64),
    };
    match cmd.op {
        6 => super::fb::flush_owner_pane(&cmd),
        // The demo draws, VirGL submissions and wait-stress ops are not in this kernel.
        0..=5 | 7 | 9 | 27..=30 => SyscallResult::Err(super::errno::ENOSYS as u64),
        _ => SyscallResult::Err(super::ErrorCode::InvalidArgument as u64),
    }
}

//! System call 416, `gpu(op, arg, len)`, and the suite-mode test controls of system call
//! 415 (`docs/gui/gpu.md` section 1.4 and section 1.8; layouts in `docs/gui/gpu-abi.md`).
//!
//! Every function returns what the kernel returned: a value, or a negated errno. The
//! layouts and the command stream encoders are `gpu_abi`'s, re-exported as [`abi`].

pub use gpu_abi as abi;

#[cfg(feature = "suite")]
pub mod upload_audit;

use abi::{op, test_control as tc, Plain, TcArm, TcFired, SYS_GPU, SYS_TEST_CONTROL};

use crate::syscall::raw;

/// `gpu(op, arg, len)` as the raw return.
///
/// # Safety
/// `arg` must be null or point to `len` bytes the kernel may read and write.
pub unsafe fn call(op: u32, arg: *mut u8, len: usize) -> i64 {
    let result = raw::syscall3(SYS_GPU, op as u64, arg as u64, len as u64) as i64;
    #[cfg(feature = "suite")]
    if result == 0 { upload_audit::observe(op, arg, len); }
    result
}

/// `gpu(op, &mut arg, size_of(arg))`.
pub fn call_with<T: Plain>(op: u32, arg: &mut T) -> i64 {
    // SAFETY: `arg` is a live, exclusively borrowed value of its own size.
    unsafe { call(op, arg as *mut T as *mut u8, core::mem::size_of::<T>()) }
}

/// `VERSION`.
pub fn version() -> i64 {
    // SAFETY: VERSION takes no argument.
    unsafe { call(op::VERSION, core::ptr::null_mut(), 0) }
}

/// `CAPS`, with the record.
pub fn caps() -> (i64, abi::Caps) {
    let mut c = abi::Caps::ZERO;
    let r = call_with(op::CAPS, &mut c);
    (r, c)
}

/// `CONTEXT_CREATE`: the handle, or the negated errno.
pub fn context_create() -> Result<u64, i64> {
    let mut a = abi::ContextCreate::ZERO;
    match call_with(op::CONTEXT_CREATE, &mut a) {
        0 => Ok(a.handle),
        e => Err(e),
    }
}

/// `CONTEXT_DESTROY`.
pub fn context_destroy(handle: u64) -> i64 {
    let mut a = abi::Handle { handle, reserved: 0 };
    call_with(op::CONTEXT_DESTROY, &mut a)
}

/// `RESOURCE_CREATE` of a target or texture.
pub fn image_create(kind: u32, format: u32, width: u32, height: u32) -> Result<u64, i64> {
    image_create_flags(kind, format, width, height, 0)
}

fn image_create_flags(kind: u32, format: u32, width: u32, height: u32, flags: u32) -> Result<u64, i64> {
    let mut a = abi::ResourceCreate { kind, format, width, height, flags, ..abi::ResourceCreate::ZERO };
    match call_with(op::RESOURCE_CREATE, &mut a) {
        0 => Ok(a.handle),
        e => Err(e),
    }
}

/// `RESOURCE_CREATE` of a render target.
pub fn target_create(format: u32, width: u32, height: u32) -> Result<u64, i64> {
    image_create(abi::kind::TARGET, format, width, height)
}

/// `RESOURCE_CREATE` of a target with scanout intent, required by `PRESENT`.
pub fn scanout_target_create(format: u32, width: u32, height: u32) -> Result<u64, i64> {
    image_create_flags(abi::kind::TARGET, format, width, height, abi::resource_flag::SCANOUT)
}

/// `RESOURCE_CREATE` of a buffer of `size` bytes.
pub fn buffer_create(size: u64) -> Result<u64, i64> {
    let mut a = abi::ResourceCreate { kind: abi::kind::BUFFER, size, ..abi::ResourceCreate::ZERO };
    match call_with(op::RESOURCE_CREATE, &mut a) {
        0 => Ok(a.handle),
        e => Err(e),
    }
}

/// `RESOURCE_DESTROY`.
pub fn resource_destroy(handle: u64) -> i64 {
    let mut a = abi::Handle { handle, reserved: 0 };
    call_with(op::RESOURCE_DESTROY, &mut a)
}

/// `RESOURCE_CREATE` of a texture.
pub fn texture_create(format: u32, width: u32, height: u32) -> Result<u64, i64> {
    image_create(abi::kind::TEXTURE, format, width, height)
}

/// `UPLOAD` of the rectangle `rect` (`x, y, width, height`) of `pixels`, laid out as `source`
/// at `stride`, into `resource`, sent in `context`: the return and the argument.
pub fn upload(context: u64, resource: u64, rect: [u32; 4], pixels: &[u8], stride: u32, source: u32, purpose: u32) -> (i64, abi::Upload) {
    upload_raw(context, resource, rect, pixels.as_ptr() as u64, pixels.len() as u64, stride, source, purpose)
}

/// `UPLOAD` from an arbitrary address and length.
#[allow(clippy::too_many_arguments)]
pub fn upload_raw(context: u64, resource: u64, rect: [u32; 4], pixels: u64, pixels_len: u64, stride: u32, source: u32, purpose: u32) -> (i64, abi::Upload) {
    let mut a = abi::Upload {
        context,
        resource,
        x: rect[0],
        y: rect[1],
        width: rect[2],
        height: rect[3],
        pixels,
        pixels_len,
        stride,
        source,
        purpose,
        reserved: 0,
    };
    let r = call_with(op::UPLOAD, &mut a);
    (r, a)
}

/// `SUBMIT` of `stream` on `context`: the return and the argument as the kernel left it
/// (its fence, or its refusal's index and code).
pub fn submit(context: u64, stream: &[u32]) -> (i64, abi::Submit) {
    submit_raw(context, stream.as_ptr() as u64, (stream.len() * 4) as u64)
}

/// `SUBMIT` of a stream at an arbitrary address and length.
pub fn submit_raw(context: u64, stream: u64, length: u64) -> (i64, abi::Submit) {
    let mut a = abi::Submit { context, stream, length, ..abi::Submit::ZERO };
    let r = call_with(op::SUBMIT, &mut a);
    (r, a)
}

/// `WAIT` for `fence` on `context` for at most `timeout_ns`: the return and the
/// argument (its status and the device's answer when the return is EIO).
pub fn wait(context: u64, fence: u64, timeout_ns: u64) -> (i64, abi::Wait) {
    let mut a = abi::Wait { context, fence, timeout_ns, ..abi::Wait::ZERO };
    let r = call_with(op::WAIT, &mut a);
    (r, a)
}

/// `READBACK` of a rectangle into `pixels` at `stride`, with the record.
pub fn readback(target: u64, rect: [u32; 4], pixels: &mut [u8], stride: u32, record: &mut [u8; abi::record::SIZE]) -> i64 {
    readback_why(target, rect, pixels, stride, record).0
}

/// `READBACK`, with the argument as the kernel left it (an EIO's reason and failed submission).
pub fn readback_why(
    target: u64,
    rect: [u32; 4],
    pixels: &mut [u8],
    stride: u32,
    record: &mut [u8; abi::record::SIZE],
) -> (i64, abi::Readback) {
    let mut a = abi::Readback {
        target,
        x: rect[0],
        y: rect[1],
        width: rect[2],
        height: rect[3],
        pixels: pixels.as_mut_ptr() as u64,
        pixels_len: pixels.len() as u64,
        stride,
        reason: 0,
        record: record.as_mut_ptr() as u64,
        failed_submission: 0,
    };
    let r = call_with(op::READBACK, &mut a);
    (r, a)
}

/// `PRESENT` of `target` in `context`: bind it to scanout 0 and flush it. The returned
/// fence is a fence of `context`, already complete when the call returns 0.
pub fn present(context: u64, target: u64) -> (i64, abi::Present) {
    let mut a = abi::Present { target, context, ..abi::Present::ZERO };
    let r = call_with(op::PRESENT, &mut a);
    (r, a)
}

/// `COUNTERS` for a context, or for the process with `context` 0.
pub fn counters(context: u64) -> (i64, abi::Counters) {
    let mut a = abi::Counters { context, ..abi::Counters::ZERO };
    let r = call_with(op::COUNTERS, &mut a);
    (r, a)
}

/// `test_control(op, arg, len)` as the raw return.
///
/// # Safety
/// `arg` must be null or point to `len` bytes the kernel may read and write.
pub unsafe fn test_control(op: u32, arg: *mut u8, len: usize) -> i64 {
    raw::syscall3(SYS_TEST_CONTROL, op as u64, arg as u64, len as u64) as i64
}

/// Test controls' `STATE`: 1 in a suite boot.
pub fn tc_state() -> i64 {
    // SAFETY: STATE takes no argument.
    unsafe { test_control(tc::STATE, core::ptr::null_mut(), 0) }
}

/// Arm `point` for `target` (0 for any) with `param`.
pub fn tc_arm(point: u32, target: u64, param: u64) -> i64 {
    let mut a = TcArm { point, reserved: 0, target, param };
    // SAFETY: `a` is a live TcArm of its own size.
    unsafe { test_control(tc::ARM, &mut a as *mut TcArm as *mut u8, core::mem::size_of::<TcArm>()) }
}

/// Disarm every point.
pub fn tc_disarm_all() -> i64 {
    // SAFETY: DISARM_ALL takes no argument.
    unsafe { test_control(tc::DISARM_ALL, core::ptr::null_mut(), 0) }
}

/// How many times `point` has fired this boot, or the negated errno.
pub fn tc_fired(point: u32) -> Result<u64, i64> {
    let mut f = TcFired { point, reserved: 0, count: 0 };
    // SAFETY: `f` is a live TcFired of its own size.
    match unsafe { test_control(tc::FIRED, &mut f as *mut TcFired as *mut u8, core::mem::size_of::<TcFired>()) } {
        0 => Ok(f.count),
        e => Err(e),
    }
}

/// Supported display modes, scale and current binding.
pub fn modes() -> (i64, abi::Modes) {
    let mut a = abi::Modes::ZERO;
    let r = call_with(op::MODES, &mut a);
    (r, a)
}
/// Select the display owner's mode for subsequent full-screen targets.
pub fn set_mode(index: u32) -> i64 {
    let mut a = abi::SetMode { index, reserved: [0; 3] };
    call_with(op::SET_MODE, &mut a)
}

//! System call 414, `window(op, arg, len)`: the window interface, version 2
//! (`docs/gui/breengel.md` section 17.1).
//!
//! `arg` points to the op's argument structure and `len` must equal its size. The structure
//! is copied in whole before anything else happens; an op whose structure has output fields
//! has the whole structure checked writable first. Another thread can still unmap the
//! structure or make it read-only while the op runs, so an op that changes state copies its
//! output out before its commit point, or undoes what it did when the copy fails: an EFAULT
//! leaves no trace (section 17.5). No path here writes to any kernel output, except the
//! payload of an armed `emit-diagnostic`.

use breengel_abi::errno::{EFAULT, EINVAL, ENOSYS, EPERM};
use breengel_abi::{
    arg_size, op, Acquire, Capture, Claim, Commit, Composite, CompositeEntry, CompositeRecordHeader,
    Create, Cursor, Geometry, Give, List, MapTree, Meta, MetaQuery, NoChange, Plain, ReadEvents,
    Readback, RecordQuery, Resize, SceneSnapshot, Title, WinId, WriteEvents, INTERFACE_VERSION,
};

/// Copy `value` to the user at `arg`: an op's output, written before its commit point.
pub fn copy_out<T: Plain>(arg: u64, value: &T) -> Result<(), i32> {
    output(arg, value)
}

use super::SyscallResult;
use crate::window::{mem, ops, present, Caller, REGISTRY};

/// `window(op, arg, len)`.
pub fn sys_window(op: u64, arg: u64, len: u64) -> SyscallResult {
    match window(op, arg, len) {
        Ok(value) => SyscallResult::Ok(value),
        Err(errno) => SyscallResult::Err(errno as u64),
    }
}

fn input<T: Plain>(arg: u64) -> Result<T, i32> {
    crate::syscall::userptr::copy_from_user(arg as *const T).map_err(|_| EFAULT)
}

fn output<T: Plain>(arg: u64, value: &T) -> Result<(), i32> {
    crate::syscall::userptr::copy_to_user(arg as *mut T, value).map_err(|_| EFAULT)
}

/// Run an op that changes no state and whose structure is both input and output: copy it in,
/// check it can be written back, run `f` on it and copy it out.
fn inout<T: Plain>(arg: u64, f: impl FnOnce(&mut T) -> Result<u64, i32>) -> Result<u64, i32> {
    let mut value = input::<T>(arg)?;
    mem::probe_writable(arg, core::mem::size_of::<T>() as u64)?;
    let result = f(&mut value)?;
    output(arg, &value)?;
    Ok(result)
}

/// Run an op that changes state and copies its own output to `arg` before its commit point
/// (or undoes its change when that copy fails): copy the structure in, check it can be
/// written back, and run `f` on it with `arg`.
fn committing<T: Plain>(arg: u64, f: impl FnOnce(&mut T, u64) -> Result<u64, i32>) -> Result<u64, i32> {
    let mut value = input::<T>(arg)?;
    mem::probe_writable(arg, core::mem::size_of::<T>() as u64)?;
    f(&mut value, arg)
}

/// A commit's metadata: none (all zero), or one `Meta` at `ptr`, whose reserved fields are
/// zero.
fn meta(ptr: u64, len: u32) -> Result<Meta, i32> {
    match (ptr, len as usize) {
        (0, 0) => Ok(Meta::ZERO),
        (0, _) => Err(EINVAL),
        (_, n) if n == core::mem::size_of::<Meta>() => {
            let m = input::<Meta>(ptr)?;
            if m.reserved != 0 || m.reserved2 != 0 {
                return Err(EINVAL);
            }
            Ok(m)
        }
        _ => Err(EINVAL),
    }
}

fn window(op: u64, arg: u64, len: u64) -> Result<u64, i32> {
    // The only output a window op ever makes: the payload `emit-diagnostic` was armed to write
    // on this thread's next op (sections 21 and 22).
    if crate::test_control::enabled() {
        if let Some(tid) = crate::task::scheduler::current_thread_id() {
            if let Some(path) = crate::test_control::take(breengel_abi::tc_point::EMIT_DIAGNOSTIC, tid) {
                crate::test_control::emit_diagnostic(path);
            }
        }
    }
    let op = u32::try_from(op).map_err(|_| EINVAL)?;
    match arg_size(op) {
        Some(size) if len != size as u64 => return Err(EINVAL),
        Some(_) => {}
        None if op > op::LAST => return Err(EINVAL),
        None if arg != 0 || len != 0 => return Err(EINVAL),
        None => {}
    }
    if op == op::VERSION {
        return Ok(INTERFACE_VERSION as u64);
    }
    let caller = Caller::current()?;
    let result = dispatch(&caller, op, arg);
    // An op that can change what the next composite shows wakes the compositor's scene
    // descriptor, whatever it returned.
    if !matches!(
        op,
        op::LIST | op::ACQUIRE | op::READ_EVENTS | op::WRITE_EVENTS | op::EVENT_FD | op::CAPTURE | op::MAP_TREE
            | op::COMPOSITE_SNAPSHOT | op::COMPOSITE_RECORD | op::SCANOUT_READBACK | op::READ_META | op::READ_REMNANT
            | op::MAP_COMMITTED | op::MAP_COMPOSITOR_TEXTURE
            | op::COMPOSITOR_ACK | op::COMPOSITOR_STATUS | op::CLIPBOARD_SET
            | op::CLIPBOARD_GET | op::CLIPBOARD_SUBSCRIBE | op::HIT_TEST
            | op::EVENT_TAG | op::GESTURE_REPORT
    ) {
        crate::window::scene::changed();
    }
    result
}

fn dispatch(caller: &Caller, op: u32, arg: u64) -> Result<u64, i32> {
    let caller = *caller;
    match op {
        op::CREATE => {
            let mut a = input::<Create>(arg)?;
            mem::probe_writable(arg, core::mem::size_of::<Create>() as u64)?;
            ops::create(&caller, &mut a)?;
            if let Err(e) = output(arg, &a) {
                ops::uncreate(&caller, a.id);
                return Err(e);
            }
            Ok(0)
        }
        op::DESTROY => ops::destroy(&caller, input::<WinId>(arg)?.id),
        op::REGISTER => ops::set_title(&caller, &input::<Title>(arg)?, true),
        op::SET_TITLE => ops::set_title(&caller, &input::<Title>(arg)?, false),
        op::LIST => ops::list(&caller, &input::<List>(arg)?),
        op::ACQUIRE => {
            let mut a = input::<Acquire>(arg)?;
            mem::probe_writable(arg, core::mem::size_of::<Acquire>() as u64)?;
            let id = a.id;
            ops::acquire(&caller, &mut a)?;
            if let Err(e) = output(arg, &a) {
                ops::unacquire_index(id, a.buffer);
                return Err(e);
            }
            Ok(0)
        }
        op::COMMIT => {
            let c = input::<Commit>(arg)?;
            let m = meta(c.meta, c.meta_len)?;
            committing::<Commit>(arg, |c, out| ops::commit(&caller, c, m, out))
        }
        op::COMMIT_NO_CHANGE => {
            let n = input::<NoChange>(arg)?;
            let m = meta(n.meta, n.meta_len)?;
            committing::<NoChange>(arg, |n, out| ops::no_change(&caller, n, m, out))
        }
        op::RESIZE => committing::<Resize>(arg, |r, out| ops::resize(&caller, r, out)),
        op::READ_EVENTS => committing::<ReadEvents>(arg, |r, out| ops::read_events(&caller, r, out)),
        op::WRITE_EVENTS => ops::write_events(&caller, &input::<WriteEvents>(arg)?),
        op::EVENT_FD => ops::event_fd(&caller, input::<WinId>(arg)?.id),
        op::SET_GEOMETRY => ops::set_geometry(&caller, &input::<Geometry>(arg)?),
        // TODO(gpu-display): B2 retires CPU buffer mappings; the GPU compositor must use
        // GPU resources instead (docs/gui/gpu.md section 2.7).
        op::MAP_COMMITTED => Err(ENOSYS),
        op::CAPTURE => inout::<Capture>(arg, |c| ops::capture(&caller, c)),
        op::MAP_TREE => committing::<MapTree>(arg, |m, out| ops::map_tree(&caller, m, out)),
        op::CLAIM_COMPOSITOR => committing::<Claim>(arg, |c, out| ops::claim(&caller, c, out)),
        op::RELEASE_COMPOSITOR => ops::release(&caller),
        op::COMPOSITE => committing::<Composite>(arg, |a,out| crate::window::scene::submit(&caller,a,out)),
        op::COMPOSITE_SNAPSHOT => committing::<SceneSnapshot>(arg, |a,out| crate::window::scene::snapshot(&caller,a,out)),
        op::COMPOSITE_RECORD => composite_record(&caller, &input::<RecordQuery>(arg)?),
        op::MAP_COMPOSITOR_TEXTURE => Err(ENOSYS),
        op::SCANOUT_READBACK => {
            let mut rb = input::<Readback>(arg)?;
            mem::probe_writable(arg, core::mem::size_of::<Readback>() as u64)?;
            readback(&caller, arg, &mut rb)
        }
        op::SET_CURSOR => {
            let c = input::<Cursor>(arg)?;
            compositor_only(&caller)?;
            if c.shape > breengel_abi::cursor_shape::HIDDEN || c.visible > 1 {
                return Err(EINVAL);
            }
            let mut reg = REGISTRY.lock();
            // The next composite repairs the sprite's old and new footprints, not the screen.
            if reg.cursor != c {
                let (old, new) = (reg.cursor.footprint(), c.footprint());
                reg.present.uncover(old);
                reg.present.uncover(new);
            }
            reg.cursor = c;
            Ok(0)
        }
        op::HOLD_EVIDENCE => ops::hold_evidence(&caller, input::<WinId>(arg)?.id),
        op::READ_REMNANT => inout::<Capture>(arg, |c| ops::read_remnant(&caller, c)),
        op::RELEASE_REMNANT => ops::release_remnant(&caller, input::<WinId>(arg)?.id),
        op::GIVE_DISPLAY => {
            let g = input::<Give>(arg)?;
            if g.reserved != 0 {
                return Err(EINVAL);
            }
            present::backend().ok_or(breengel_abi::errno::EIO)?.give_display(&caller, g.pid)?;
            Ok(0)
        }
        op::READ_META => inout::<MetaQuery>(arg, |q| {
            mem::probe_writable(q.meta, core::mem::size_of::<Meta>() as u64)?;
            let (commit_id, meta) = ops::read_meta(&caller, q.id)?;
            output(q.meta, &meta)?;
            q.commit_id = commit_id;
            Ok(0)
        }),
        // E18 contract-first allocation; each implementation lane replaces its
        // arm. Exact sizes were validated before dispatch (breengel.md 17.1).
        op::COMPOSITOR_ACK => Err(ENOSYS),
        op::COMPOSITOR_STATUS => Err(ENOSYS),
        op::CLIPBOARD_SET => Err(ENOSYS),
        op::CLIPBOARD_GET => Err(ENOSYS),
        op::CLIPBOARD_SUBSCRIBE => Err(ENOSYS),
        op::HIT_TEST => Err(ENOSYS),
        op::EVENT_TAG => inout::<breengel_abi::EventTag>(arg, |tag| crate::input::stream::event_tag(&caller, tag)),
        op::GESTURE_REPORT => Err(ENOSYS),
        _ => Err(EINVAL),
    }
}

fn compositor_only(caller: &Caller) -> Result<(), i32> {
    if REGISTRY.lock().is_compositor(caller) {
        Ok(())
    } else {
        Err(EPERM)
    }
}

/// `COMPOSITE_RECORD`: the header, then its entries, into the caller's buffer.
fn composite_record(caller: &Caller, q: &RecordQuery) -> Result<u64, i32> {
    if q.reserved != 0 {
        return Err(EINVAL);
    }
    let (header, entries) = present::record(caller, q.serial)?;
    let head = core::mem::size_of::<CompositeRecordHeader>();
    let entry = core::mem::size_of::<CompositeEntry>();
    let total = head + entries.len() * entry;
    if (q.len as usize) < total {
        return Err(EINVAL);
    }
    mem::probe_writable(q.ptr, total as u64)?;
    output(q.ptr, &header)?;
    for (i, e) in entries.iter().enumerate() {
        output(q.ptr + (head + i * entry) as u64, e)?;
    }
    Ok(0)
}

/// `SCANOUT_READBACK` (root or the compositor): the whole scanout and its record version 2,
/// observed by the GPU core's pinned-binding service (gpu-abi.md, 414/22). The window layer
/// supplies only the authority; an EIO writes only its reason.
fn readback(caller: &Caller, arg: u64, rb: &mut Readback) -> Result<u64, i32> {
    if !caller.is_root() && !REGISTRY.lock().is_compositor(caller) {
        return Err(EPERM);
    }
    if rb.reserved != 0
        || rb.reserved2 != 0
        || rb.serial != 0
        || rb.reason != 0
        || rb.record_len as usize != gpu_abi::record::SIZE
    {
        return Err(EINVAL);
    }
    match crate::gpu::observe_scanout(rb.dst, rb.dst_len, rb.record) {
        Ok(serial) => {
            rb.serial = serial;
            output(arg, rb)?;
            Ok(0)
        }
        Err((errno, reason)) => {
            if errno == gpu_abi::errno::EIO {
                rb.reason = reason;
                output(arg, rb)?;
            }
            Err(errno as i32)
        }
    }
}

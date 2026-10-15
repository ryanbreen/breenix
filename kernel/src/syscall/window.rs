//! System call 414, `window(op, arg, len)`: the window interface, version 2
//! (`docs/gui/breengel.md` section 17.1).
//!
//! `arg` points to the op's argument structure and `len` must equal its size. The structure
//! is copied in whole before anything else happens; an op whose structure has output fields
//! has the whole structure checked writable first, so an EFAULT leaves no trace. No path here
//! writes to any kernel output, except the payload of an armed `emit-diagnostic`.

use breengel_abi::errno::{EFAULT, EINVAL, EPERM};
use breengel_abi::{
    arg_size, op, Acquire, Capture, Claim, Commit, Composite, CompositeEntry, CompositeRecordHeader,
    Create, Cursor, Geometry, Give, List, MapBuffers, MapTree, Meta, MetaQuery, NoChange, Plain, ReadEvents,
    Readback, RecordQuery, Resize, Title, WinId, WriteEvents, INTERFACE_VERSION,
};

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

/// Run an op whose structure is both input and output: copy it in, check it can be written
/// back, run `f` on it and copy it out.
fn inout<T: Plain>(arg: u64, f: impl FnOnce(&mut T) -> Result<u64, i32>) -> Result<u64, i32> {
    let mut value = input::<T>(arg)?;
    mem::probe_writable(arg, core::mem::size_of::<T>() as u64)?;
    let result = f(&mut value)?;
    output(arg, &value)?;
    Ok(result)
}

/// A commit's metadata: none (all zero), or one `Meta` at `ptr`.
fn meta(ptr: u64, len: u32) -> Result<Meta, i32> {
    match (ptr, len as usize) {
        (0, 0) => Ok(Meta::ZERO),
        (0, _) => Err(EINVAL),
        (_, n) if n == core::mem::size_of::<Meta>() => input::<Meta>(ptr),
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
            inout::<Commit>(arg, |c| ops::commit(&caller, c, m))
        }
        op::COMMIT_NO_CHANGE => {
            let n = input::<NoChange>(arg)?;
            let m = meta(n.meta, n.meta_len)?;
            inout::<NoChange>(arg, |n| ops::no_change(&caller, n, m))
        }
        op::RESIZE => inout::<Resize>(arg, |r| ops::resize(&caller, r)),
        op::READ_EVENTS => inout::<ReadEvents>(arg, |r| ops::read_events(&caller, r)),
        op::WRITE_EVENTS => ops::write_events(&caller, &input::<WriteEvents>(arg)?),
        op::EVENT_FD => ops::event_fd(&caller, input::<WinId>(arg)?.id),
        op::SET_GEOMETRY => ops::set_geometry(&caller, &input::<Geometry>(arg)?),
        op::MAP_COMMITTED => inout::<MapBuffers>(arg, |m| ops::map_committed(&caller, m)),
        op::CAPTURE => inout::<Capture>(arg, |c| ops::capture(&caller, c)),
        op::MAP_TREE => inout::<MapTree>(arg, |m| ops::map_tree(&caller, m)),
        op::CLAIM_COMPOSITOR => inout::<Claim>(arg, |c| ops::claim(&caller, c)),
        op::RELEASE_COMPOSITOR => ops::release(&caller),
        op::COMPOSITE => inout::<Composite>(arg, |c| {
            if c.flags != 0 || c.reserved != 0 {
                return Err(EINVAL);
            }
            let (serial, pixels) = present::composite(&caller)?;
            c.serial = serial;
            c.pixels = pixels;
            Ok(0)
        }),
        op::COMPOSITE_RECORD => composite_record(&caller, &input::<RecordQuery>(arg)?),
        op::MAP_COMPOSITOR_TEXTURE => inout::<MapTree>(arg, |m| {
            compositor_only(&caller)?;
            present::backend().ok_or(breengel_abi::errno::EIO)?.map_texture(&caller, m)?;
            Ok(0)
        }),
        op::SCANOUT_READBACK => inout::<Readback>(arg, |rb| readback(&caller, rb)),
        op::SET_CURSOR => {
            let c = input::<Cursor>(arg)?;
            compositor_only(&caller)?;
            if c.shape > breengel_abi::cursor_shape::HIDDEN || c.visible > 1 {
                return Err(EINVAL);
            }
            present::backend().ok_or(breengel_abi::errno::EIO)?.set_cursor(&caller, &c)?;
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

/// `SCANOUT_READBACK` (root or the compositor): the destination and record are checked here,
/// the display backend fills them.
fn readback(caller: &Caller, rb: &mut Readback) -> Result<u64, i32> {
    use breengel_abi::limits::{SCREEN_HEIGHT, SCREEN_WIDTH};
    if !caller.is_root() && !REGISTRY.lock().is_compositor(caller) {
        return Err(EPERM);
    }
    if rb.reserved != 0 {
        return Err(EINVAL);
    }
    let pixels = SCREEN_WIDTH as u64 * SCREEN_HEIGHT as u64 * 4;
    let record = core::mem::size_of::<breengel_abi::ObservationRecord>() as u64;
    if rb.dst_len < pixels || (rb.record_len as u64) < record {
        return Err(EINVAL);
    }
    mem::probe_writable(rb.dst, pixels)?;
    mem::probe_writable(rb.record, record)?;
    present::backend()
        .ok_or(breengel_abi::errno::EIO)?
        .readback(caller, rb)?;
    Ok(0)
}

//! The window interface, version 2: system call 414 (`docs/gui/breengel.md` section 17).
//!
//! `call` makes any op with its argument structure; the functions below are the common ops
//! spelled out. The structures and constants come from `breengel-abi`, re-exported here as
//! `abi`.

pub use breengel_abi as abi;

use abi::{op, Plain};

use crate::error::Error;
use crate::syscall::raw;

/// `window(op, arg, len)` with a raw pointer: the kernel's return, a value or a negated
/// errno.
///
/// # Safety
/// `arg` must be null or point to `len` bytes the kernel may read and, for an op with output
/// fields, write.
pub unsafe fn raw_call(op: u32, arg: *mut u8, len: usize) -> i64 {
    raw::syscall3(abi::SYS_WINDOW, op as u64, arg as u64, len as u64) as i64
}

/// `window(op, &mut value, size_of(value))`.
pub fn call<T: Plain>(op: u32, value: &mut T) -> Result<u64, Error> {
    // SAFETY: `value` is a live, exclusively borrowed plain structure of its own size.
    Error::from_syscall(unsafe {
        raw_call(op, value as *mut T as *mut u8, core::mem::size_of::<T>())
    })
}

/// `VERSION`: 2 when the interface exists.
pub fn version() -> Result<u64, Error> {
    // SAFETY: VERSION takes no argument.
    Error::from_syscall(unsafe { raw_call(op::VERSION, core::ptr::null_mut(), 0) })
}

/// `CREATE`: a `width` x `height` window with `buffers` buffers (2 or 3) and `tree_kib` KiB of
/// tree page per buffer (0 for the default 64). The result holds its id, stride, buffer
/// GPU target handles and the mapped tree address.
pub fn create(width: u32, height: u32, buffers: u32, tree_kib: u32) -> Result<abi::Create, Error> {
    let mut c = abi::Create::ZERO;
    c.width = width;
    c.height = height;
    c.buffers = buffers;
    c.tree_kib = tree_kib;
    call(op::CREATE, &mut c)?;
    Ok(c)
}

/// An op whose argument is a window id.
pub fn with_id(op: u32, id: u64) -> Result<u64, Error> {
    call(op, &mut abi::WinId { id })
}

pub fn destroy(id: u64) -> Result<u64, Error> {
    with_id(op::DESTROY, id)
}

fn title_op(op: u32, id: u64, title: &[u8]) -> Result<u64, Error> {
    let mut t = abi::Title::ZERO;
    t.id = id;
    t.ptr = title.as_ptr() as u64;
    t.len = title.len() as u32;
    call(op, &mut t)
}

/// `REGISTER`: list the window under `title`.
pub fn register(id: u64, title: &[u8]) -> Result<u64, Error> {
    title_op(op::REGISTER, id, title)
}

pub fn set_title(id: u64, title: &[u8]) -> Result<u64, Error> {
    title_op(op::SET_TITLE, id, title)
}

/// `LIST` into `out`: the registered windows whose owners share the caller's effective uid.
/// Root and the compositor see the registered registry. The returned count is filtered too
/// and may exceed the output capacity (section 17.4, B46).
pub fn list(out: &mut [abi::WinInfo]) -> Result<usize, Error> {
    let mut l = abi::List::ZERO;
    l.ptr = out.as_mut_ptr() as u64;
    l.capacity = out.len() as u32;
    call(op::LIST, &mut l).map(|n| n as usize)
}

/// `ACQUIRE`, waiting up to `timeout_ns` (`u64::MAX` forever) for a free buffer.
pub fn acquire(id: u64, timeout_ns: u64) -> Result<abi::Acquire, Error> {
    let mut a = abi::Acquire::ZERO;
    a.id = id;
    a.timeout_ns = timeout_ns;
    call(op::ACQUIRE, &mut a)?;
    Ok(a)
}

/// `COMMIT` buffer `buffer` with `damage` (at most 16 rectangles) and optional metadata. The
/// result holds the commit id and pixel generation.
pub fn commit(id: u64, buffer: u32, damage: &[abi::Rect], meta: Option<&abi::Meta>) -> Result<abi::Commit, Error> {
    let mut c = abi::Commit::ZERO;
    c.id = id;
    c.buffer = buffer;
    c.damage_count = damage.len() as u32;
    for (slot, r) in c.damage.iter_mut().zip(damage.iter()) {
        *slot = *r;
    }
    if let Some(m) = meta {
        c.meta = m as *const abi::Meta as u64;
        c.meta_len = core::mem::size_of::<abi::Meta>() as u32;
    }
    call(op::COMMIT, &mut c)?;
    Ok(c)
}

/// `COMMIT_NO_CHANGE` with optional metadata: returns the new commit id.
pub fn commit_no_change(id: u64, meta: Option<&abi::Meta>) -> Result<u64, Error> {
    let mut n = abi::NoChange::ZERO;
    n.id = id;
    if let Some(m) = meta {
        n.meta = m as *const abi::Meta as u64;
        n.meta_len = core::mem::size_of::<abi::Meta>() as u32;
    }
    call(op::COMMIT_NO_CHANGE, &mut n)?;
    Ok(n.commit_id)
}

/// `RESIZE` to `width` x `height`.
pub fn resize(id: u64, width: u32, height: u32) -> Result<abi::Resize, Error> {
    let mut r = abi::Resize::ZERO;
    r.id = id;
    r.width = width;
    r.height = height;
    call(op::RESIZE, &mut r)?;
    Ok(r)
}

/// `READ_EVENTS` into `out`, blocking up to `timeout_ns` when `block`.
pub fn read_events(id: u64, out: &mut [abi::WindowEvent], block: bool, timeout_ns: u64) -> Result<usize, Error> {
    let mut r = abi::ReadEvents::ZERO;
    r.id = id;
    r.ptr = out.as_mut_ptr() as u64;
    r.capacity = out.len() as u32;
    r.flags = if block { abi::READ_EVENTS_BLOCK } else { 0 };
    r.timeout_ns = timeout_ns;
    call(op::READ_EVENTS, &mut r).map(|n| n as usize)
}

/// `WRITE_EVENTS` (compositor): returns how many were queued.
pub fn write_events(id: u64, events: &[abi::WindowEvent]) -> Result<usize, Error> {
    let mut w = abi::WriteEvents::ZERO;
    w.id = id;
    w.ptr = events.as_ptr() as u64;
    w.count = events.len() as u32;
    call(op::WRITE_EVENTS, &mut w).map(|n| n as usize)
}

/// `SET_GEOMETRY` (compositor).
pub fn set_geometry(id: u64, x: i32, y: i32, z: u32, flags: u32) -> Result<u64, Error> {
    call(op::SET_GEOMETRY, &mut abi::Geometry { id, x, y, z, flags })
}

/// `CAPTURE` into `dst` (at least width x height x 4 bytes), and the tree page into `tree`.
pub fn capture(id: u64, dst: &mut [u8], tree: Option<&mut [u8]>) -> Result<abi::Capture, Error> {
    let mut c = abi::Capture::ZERO;
    c.id = id;
    c.dst = dst.as_mut_ptr() as u64;
    c.dst_len = dst.len() as u64;
    if let Some(t) = tree {
        c.tree_dst = t.as_mut_ptr() as u64;
        c.tree_len = t.len() as u64;
    }
    call(op::CAPTURE, &mut c)?;
    Ok(c)
}

/// `CLAIM_COMPOSITOR` (root): returns the claim's epoch.
pub fn claim_compositor() -> Result<u64, Error> {
    let mut c = abi::Claim::ZERO;
    call(op::CLAIM_COMPOSITOR, &mut c)?;
    Ok(c.epoch)
}

pub fn release_compositor() -> Result<u64, Error> {
    // SAFETY: RELEASE_COMPOSITOR takes no argument.
    Error::from_syscall(unsafe { raw_call(op::RELEASE_COMPOSITOR, core::ptr::null_mut(), 0) })
}

/// Reserve scene metadata for one userspace-built GPU composition.
pub fn composite_snapshot(snapshot: &mut abi::SceneSnapshot) -> Result<u64, Error> {
    call(op::COMPOSITE_SNAPSHOT, snapshot)
}

/// Submit or cancel a B13 reservation. No drawing is generated by the kernel.
pub fn composite(submission: &mut abi::Composite) -> Result<u64, Error> {
    call(op::COMPOSITE, submission)
}

/// Read the successfully published commit and its metadata.
pub fn read_meta(id: u64) -> Result<(u64, abi::Meta), Error> {
    let mut meta = abi::Meta::ZERO;
    let mut query = abi::MetaQuery { id, meta: &mut meta as *mut abi::Meta as u64, commit_id: 0 };
    call(op::READ_META, &mut query)?;
    Ok((query.commit_id, meta))
}

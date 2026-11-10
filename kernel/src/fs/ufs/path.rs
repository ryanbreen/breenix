//! Daemon pathname operations. Callers hold no namespace or filesystem guards.
use super::call;
use crate::fs::{attrs::InodeAttrs, permissions::Credentials};
use crate::syscall::errno::*;
use alloc::vec::Vec;
use ufs_abi::{payload::{Attr, SetTimes, Writer}, Op, Request, NAME_MAX};

pub fn attrs(attr: Attr) -> InodeAttrs { InodeAttrs::new(attr.mode, attr.uid, attr.gid) }

pub fn request(mount: u64, op: Op, node: u64, payload: &[u8]) -> Result<Vec<u8>, u64> {
    if node == 0 { return Err(ENOENT as u64); }
    call(mount, Request { id: 0, op, node, payload })
}

fn attribute(bytes: &[u8], node: Option<u64>) -> Result<Attr, u64> {
    let attr = Attr::decode(bytes).map_err(|_| EIO as u64)?;
    if attr.node == 0 || node.is_some_and(|node| node != attr.node)
        || attr.size > i64::MAX as u64
        || !matches!(attr.mode & 0o170000, 0o040000 | 0o100000) {
        return Err(EIO as u64);
    }
    Ok(attr)
}

pub fn getattr(mount: u64, node: u64) -> Result<Attr, u64> {
    attribute(&request(mount, Op::Getattr, node, &[])?, Some(node))
}

pub fn named(mount: u64, op: Op, parent: u64, name: &[u8], mode: Option<u32>) -> Result<Vec<u8>, u64> {
    let mut bytes = [0; NAME_MAX + 6];
    let mut w = Writer::new(&mut bytes);
    if let Some(mode) = mode { w.u32(mode).map_err(|_| EINVAL as u64)?; }
    w.name(name).map_err(|_| EINVAL as u64)?;
    let len = w.len();
    request(mount, op, parent, &bytes[..len])
}

pub fn lookup(mount: u64, parent: u64, name: &[u8]) -> Result<Attr, u64> {
    attribute(&named(mount, Op::Lookup, parent, name, None)?, None)
}

/// Resolve a held directory's current name without remembering an old path.
pub fn parent(mount: u64, node: u64) -> Result<(u64, Vec<u8>), u64> {
    let bytes = request(mount, Op::Parent, node, &[])?;
    let mut r = ufs_abi::payload::Reader::new(&bytes);
    let parent = r.u64().map_err(|_| EIO as u64)?;
    let name = r.name().map_err(|_| EIO as u64)?;
    r.finish().map_err(|_| EIO as u64)?;
    if parent == 0 || parent == node { return Err(EIO as u64); }
    let mut owned = Vec::new();
    owned.try_reserve_exact(name.len()).map_err(|_| ENOMEM as u64)?;
    owned.extend_from_slice(name);
    Ok((parent, owned))
}

pub fn parent_access(mount: u64, parent: u64, cred: &Credentials, owner: Option<u32>) -> Result<Attr, u64> {
    let attr = getattr(mount, parent)?;
    if !attrs(attr).is_dir() { return Err(ENOTDIR as u64); }
    if attr.nlink == 0 { return Err(ENOENT as u64); }
    if !cred.may(attrs(attr), 3) { return Err(EACCES as u64); }
    if let Some(owner) = owner {
        if attr.mode & 0o1000 != 0 && cred.euid != 0 && cred.euid != attr.uid && cred.euid != owner {
            return Err(EPERM as u64);
        }
    }
    Ok(attr)
}

pub fn mkdir(mount: u64, node: u64, parent: Option<u64>, name: &[u8], mode: u32, cred: &Credentials) -> Result<(), u64> {
    if node != 0 { return Err(EEXIST as u64); }
    let parent = parent.ok_or(EBUSY as u64)?;
    let attr = parent_access(mount, parent, cred, None)?;
    let inherit = attr.mode & 0o2000 != 0;
    let gid = if inherit { attr.gid } else { cred.egid };
    let mode = crate::syscall::fifo_policy::creation_mode(mode, cred, gid, inherit) as u32;
    let created = attribute(&named(mount, Op::Mkdir, parent, name, Some(mode))?, None)?;
    if !attrs(created).is_dir() { return Err(EIO as u64); }
    Ok(())
}

fn empty(bytes: Vec<u8>) -> Result<(), u64> {
    if bytes.is_empty() { Ok(()) } else { Err(EIO as u64) }
}

pub fn remove(mount: u64, node: u64, parent: Option<u64>, name: &[u8], dir: bool, cred: &Credentials) -> Result<(), u64> {
    let attr = getattr(mount, node)?;
    let parent = parent.ok_or(EBUSY as u64)?;
    parent_access(mount, parent, cred, Some(attr.uid))?;
    if attrs(attr).is_dir() != dir { return Err(if dir { ENOTDIR } else { EISDIR } as u64); }
    empty(named(mount, if dir { Op::Rmdir } else { Op::Unlink }, parent, name, None)?)
}

pub fn rename(mount: u64, node: u64, parent: u64, old: &[u8], new_parent: u64, new: &[u8], trailing_slash: bool, cred: &Credentials) -> Result<(), u64> {
    let source = lookup(mount, parent, old)?;
    if source.node != node { return Err(ENOENT as u64); }
    parent_access(mount, parent, cred, Some(source.uid))?;
    let dest = match lookup(mount, new_parent, new) {
        Ok(attr) => Some(attr),
        Err(e) if e == ENOENT as u64 => None,
        Err(e) => return Err(e),
    };
    parent_access(mount, new_parent, cred, dest.map(|a| a.uid))?;
    if trailing_slash && !attrs(source).is_dir() { return Err(ENOTDIR as u64); }
    if let Some(dest) = dest {
        if attrs(source).is_dir() != attrs(dest).is_dir() {
            return Err(if attrs(source).is_dir() { ENOTDIR } else { EISDIR } as u64);
        }
    }
    if parent != new_parent && attrs(source).is_dir() && !cred.may(attrs(source), 2) { return Err(EACCES as u64); }
    let mut bytes = [0; 2 * NAME_MAX + 12];
    let mut w = Writer::new(&mut bytes);
    w.u64(new_parent).map_err(|_| EINVAL as u64)?;
    w.name(old).map_err(|_| EINVAL as u64)?;
    w.name(new).map_err(|_| EINVAL as u64)?;
    let len = w.len();
    empty(request(mount, Op::Rename, parent, &bytes[..len])?)
}

pub fn truncate(mount: u64, node: u64, length: u64, cred: &Credentials) -> Result<(), u64> {
    let mounted = super::get(mount)?;
    let mutation = mounted.mutation_gate(node)?;
    let _mutation = mutation.enter()?;
    let attr = getattr(mount, node)?;
    if attrs(attr).is_dir() { return Err(EISDIR as u64); }
    if !cred.may(attrs(attr), 2) { return Err(EACCES as u64); }
    if length > crate::syscall::resource::current_fsize() {
        crate::syscall::resource::signal_fsize();
        return Err(EFBIG as u64);
    }
    attribute(&request(mount, Op::Setattr, node, &length.to_le_bytes())?, Some(node))?;
    Ok(())
}

pub fn utimens(mount: u64, node: u64, times: SetTimes, owner_only: bool, cred: &Credentials) -> Result<(), u64> {
    let attr = getattr(mount, node)?;
    if !cred.privileged() && cred.euid != attr.uid {
        if owner_only { return Err(EPERM as u64); }
        if !cred.may(attrs(attr), 2) { return Err(EACCES as u64); }
    }
    let mut bytes = [0; SetTimes::SIZE];
    times.encode(&mut bytes).map_err(|_| EINVAL as u64)?;
    attribute(&request(mount, Op::Setattr, node, &bytes)?, Some(node))?;
    Ok(())
}

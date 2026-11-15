//! Retained namespace nodes and filesystem ancestry.
#[cfg(not(test))]
use super::NodeAttrs;
use super::{FileSystem, VNode};
use crate::fs::errno::Errno;
#[cfg(not(test))]
use crate::fs::namei::Resolved;
use crate::syscall::errno::ENOENT;
use alloc::{sync::Arc, vec::Vec};

/// Name one directory by its parent's current entry, using byte names.
/// Retain both incarnations across the separate parent and readdir operations.
pub fn parent_name(node: &VNode) -> Result<(Arc<VNode>, Vec<u8>), Errno> {
    let fs = &node.filesystem;
    let parent = fs.open(fs.parent(node.ino)?)?;
    let mut name = None;
    fs.readdir(parent.ino, 0, &mut |entry, _| {
        if entry.ino != node.ino || entry.name == b"." || entry.name == b".." {
            return Ok(true);
        }
        name = Some(entry.name);
        Ok(false)
    })?;
    let name = name.ok_or(Errno(ENOENT))?;
    if fs.parent(node.ino)? != parent.ino {
        return Err(Errno(ENOENT));
    }
    Ok((parent, name))
}

#[cfg(not(test))]
pub fn entry_dir(
    resolved: &Resolved<Vec<u8>>,
) -> Result<(Arc<dyn FileSystem>, Arc<VNode>, NodeAttrs), Errno> {
    let parent = resolved.entry_dir().ok_or(Errno(ENOENT))?.clone();
    let fs = parent.filesystem.clone();
    let attrs = fs.getattr(parent.ino)?;
    Ok((fs, parent, attrs))
}

#[cfg(not(test))]
pub fn target(resolved: &Resolved<Vec<u8>>) -> Result<(Arc<dyn FileSystem>, Arc<VNode>), Errno> {
    held(resolved.handle().ok_or(Errno(ENOENT))?)
}

#[cfg(not(test))]
pub fn lookup_parent(
    resolved: &Resolved<Vec<u8>>,
) -> Result<(Arc<dyn FileSystem>, Arc<VNode>), Errno> {
    held(resolved.lookup_parent().ok_or(Errno(ENOENT))?)
}

/// Reuse the retained incarnation without reopening by inode number.
pub fn held(node: &Arc<VNode>) -> Result<(Arc<dyn FileSystem>, Arc<VNode>), Errno> {
    Ok((node.filesystem.clone(), node.clone()))
}

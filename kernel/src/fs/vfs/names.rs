//! Held parents for namespace operations resolved by namei.
use super::{FileSystem, NodeAttrs, VNode};
use crate::fs::{errno::Errno, namei::Resolved};
use crate::syscall::errno::{EIO, ENOENT};
use alloc::{sync::Arc, vec::Vec};

/// Turn namei's entry-directory pin into the backend-neutral parent, retaining
/// its incarnation while the caller checks policy and changes the entry.
pub fn entry_dir(
    resolved: &Resolved<Vec<u8>>,
) -> Result<(Arc<dyn FileSystem>, Arc<VNode>, NodeAttrs), Errno> {
    let mount = resolved.mount().ok_or(Errno(EIO))?;
    let held = resolved.entry_dir().ok_or(Errno(EIO))?;
    let fs = super::mount::filesystem(mount.0)?;
    let parent = fs.open(held.object.key.inode.into())?;
    if parent.mount != held.object.mount.mount_id
        || parent.incarnation != held.object.key.incarnation
    {
        return Err(Errno(ENOENT));
    }
    let attrs = fs.getattr(parent.ino)?;
    Ok((fs, parent, attrs))
}

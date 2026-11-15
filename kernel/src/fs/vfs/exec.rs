//! Program images copied through the filesystem interface.
use crate::fs::vfs::{FileSystem, MountFlags, NodeKind, VNode};
use crate::fs::errno::Errno;
pub use crate::fs::permissions::Credentials;
use crate::process::credentials::ExecIdentity;
use crate::syscall::errno::{EACCES, EINTR, EIO, EISDIR, ENOMEM};
use alloc::vec::Vec;

/// Resolve and pin an image, returning bytes, set-ID identity and physical path.
/// Directories return EISDIR; syscall callers retain their historical EACCES.
#[cfg(not(test))]
pub fn load_image(
    path: impl AsRef<[u8]>,
    cred: &Credentials,
    killed: impl Fn() -> bool,
) -> Result<(Vec<u8>, ExecIdentity, Vec<u8>), Errno> {
    trace(b'1');
    let resolved = crate::fs::namei::resolve_bytes(path.as_ref(), true).map_err(|errno| {
        trace(b'!');
        Errno(errno as i32)
    })?;
    match resolved.target {
        crate::fs::namei::Target::Inode { .. } => {}
        crate::fs::namei::Target::Absent { .. } => {
            trace(b'!');
            return Err(Errno(crate::syscall::errno::ENOENT));
        }
        crate::fs::namei::Target::Virtual | crate::fs::namei::Target::User { .. } => {
            trace(b'!');
            return Err(Errno(EACCES));
        }
    }
    trace(b'2');
    let (fs, node) = super::names::target(&resolved).map_err(|errno| {
        trace(b'@');
        errno
    })?;
    trace(b'3');
    trace(b'4');
    // Refuse directories before acquiring the regular-file transfer lease.
    let attrs = fs.getattr(node.ino).map_err(|errno| {
        trace(b'@');
        errno
    })?;
    trace(b'5');
    image_kind(attrs.kind)?;
    // Retain mutation exclusion, then block new mmap bindings on this VNode.
    // Existing bindings retain their authority, checked before copying below.
    // The page-map lock is not held across I/O.
    let _image = fs.mapping_io(&node)?;
    let _bindings = node.map.block_mapping_bindings();
    let flags = super::mount::flags_at(&resolved.path);
    let (bytes, identity) = read_image(fs.as_ref(), &node, flags, cred, killed)?;
    Ok((bytes, identity, resolved.path))
}

fn image_kind(kind: NodeKind) -> Result<(), Errno> {
    match kind {
        NodeKind::Regular => Ok(()),
        NodeKind::Directory => {
            trace(b'#');
            Err(Errno(EISDIR))
        }
        _ => Err(Errno(EACCES)),
    }
}

/// The caller retains the image lease through this attribute snapshot and copy.
/// Disk-only host tests exercise the same policy and byte reads without mappings.
pub(crate) fn read_image(
    fs: &dyn FileSystem,
    node: &VNode,
    flags: MountFlags,
    cred: &Credentials,
    killed: impl Fn() -> bool,
) -> Result<(Vec<u8>, ExecIdentity), Errno> {
    let attrs = fs.getattr(node.ino).map_err(|errno| {
        trace(b'@');
        errno
    })?;
    image_kind(attrs.kind)?;
    let mut execution = attrs.attrs;
    // Shared write authority (even currently read-only) or dirty cache pages
    // can alter the copied bytes without a filesystem mutation. They confer
    // no set-ID identity, matching the former exclusive exec read.
    if node.map.may_change_exec_image() {
        execution.mode &= !0o6000;
    }
    let identity = cred.may_exec(execution, flags).map_err(|errno| {
        trace(b'$');
        Errno(errno)
    })?;
    trace(b'6');
    let size = usize::try_from(attrs.size).map_err(|_| Errno(ENOMEM))?;
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(size).map_err(|_| Errno(ENOMEM))?;
    while bytes.len() < size {
        if killed() {
            return Err(Errno(EINTR));
        }
        let offset = bytes.len();
        let end = size.min(offset.saturating_add(64 * 1024));
        bytes.resize(end, 0);
        let n = fs
            .read(node, offset as u64, &mut bytes[offset..end])
            .map_err(|errno| {
                trace(b'%');
                errno
            })?;
        if n == 0 || n > end - offset {
            trace(b'%');
            return Err(Errno(EIO));
        }
        bytes.truncate(offset + n);
    }
    trace(b'7');
    Ok((bytes, identity))
}

// Retain ARM's bounded exec breadcrumbs at the shared loader's actual stages.
fn trace(marker: u8) {
    #[cfg(all(target_arch = "aarch64", not(test)))]
    crate::arch_impl::aarch64::trace::trace_exec(marker);
    #[cfg(any(not(target_arch = "aarch64"), test))]
    let _ = marker;
}

//! Pin executable images and read them through the filesystem interface.
use crate::fs::vfs::{FileSystem, MountFlags, NodeKind, VNode};
use crate::fs::errno::Errno;
pub use crate::fs::permissions::Credentials;
use crate::process::credentials::ExecIdentity;
use crate::syscall::errno::{EACCES, EINTR, EIO, EISDIR};
#[cfg(not(test))]
use crate::exec_elf::{Image, MappedElf, Purpose};

/// Resolve and pin an image, returning its mapped ELF (or, for exec, its
/// `#!` line), the identity its set-ID bits confer, and its physical path.
/// Only the ELF headers pass through the kernel heap; segment bytes are read
/// straight into the new image's pages while the lease below is held.
/// Directories return EISDIR; syscall callers retain their historical EACCES.
#[cfg(not(test))]
pub fn load_image(
    path: impl AsRef<[u8]>,
    cred: &Credentials,
    killed: impl Fn() -> bool,
    purpose: Purpose,
) -> Result<(Image, ExecIdentity, alloc::vec::Vec<u8>), Errno> {
    let ((image, interpreter), identity, path) = with_image(
        path.as_ref(), cred, &killed, false, |size, read| {
            let mut header = [0; crate::exec_elf::ELF_HEADER];
            let prefix = size.min(header.len() as u64) as usize;
            read(0, &mut header[..prefix])?;
            if purpose == Purpose::Exec && header[..prefix].starts_with(b"#!") {
                let line = crate::exec_elf::read_shebang(&header[2..prefix], size, read)?;
                Ok((Image::Script(line), None))
            } else {
                let (elf, interpreter) = MappedElf::load(&header[..prefix], size, read, purpose)?;
                Ok((Image::Elf(elf), interpreter))
            }
        },
    )?;
    // Resolve the interpreter after the first lease is released. In
    // particular, a pathname can resolve back to the same inode.
    #[cfg(target_arch = "x86_64")]
    let image = match (image, interpreter) {
        (Image::Elf(elf), Some(program)) => {
            Image::Elf(elf.with_interpreter(&program, |interp, consume| {
                // An exec that confers an identity cannot use the caller's
                // working directory to pick the code running under that identity.
                if identity.any() && !interp.starts_with(b"/") {
                    return Err(Errno(EACCES));
                }
                // The program supplies the path and set-ID identity.
                with_image(interp, cred, &killed, true, consume).map(|(plan, _, _)| plan)
            })?)
        }
        (image, _) => image,
    };
    #[cfg(target_arch = "aarch64")]
    debug_assert!(interpreter.is_none());
    Ok((image, identity, path))
}

#[cfg(not(test))]
fn with_image<T>(
    path: &[u8],
    cred: &Credentials,
    killed: impl Fn() -> bool,
    require_read: bool,
    consume: impl FnOnce(u64, &mut dyn FnMut(u64, &mut [u8]) -> Result<(), Errno>) -> Result<T, Errno>,
) -> Result<(T, ExecIdentity, alloc::vec::Vec<u8>), Errno> {
    trace(b'1');
    let resolved = crate::fs::namei::resolve_bytes(path, true).map_err(|errno| {
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
    let _bindings = node.map.block_mapping_bindings().map_err(Errno)?;
    let flags = super::mount::flags_at(&resolved.path);
    let (image, identity) = read_image(fs.as_ref(), &node, flags, cred, killed, require_read, consume)?;
    Ok((image, identity, resolved.path))
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

/// The caller retains the image lease through this attribute snapshot and the reads `consume` makes.
/// Disk-only host tests exercise the same policy and byte reads without mappings.
pub(crate) fn read_image<T>(
    fs: &dyn FileSystem,
    node: &VNode,
    flags: MountFlags,
    cred: &Credentials,
    killed: impl Fn() -> bool,
    require_read: bool,
    consume: impl FnOnce(u64, &mut dyn FnMut(u64, &mut [u8]) -> Result<(), Errno>) -> Result<T, Errno>,
) -> Result<(T, ExecIdentity), Errno> {
    let attrs = fs.getattr(node.ino).map_err(|errno| {
        trace(b'@');
        errno
    })?;
    image_kind(attrs.kind)?;
    if require_read && !cred.may(attrs.attrs, 4) {
        return Err(Errno(EACCES));
    }
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
    let mut read = |mut offset: u64, bytes: &mut [u8]| {
        for mut chunk in bytes.chunks_mut(64 * 1024) {
            while !chunk.is_empty() {
                if killed() {
                    return Err(Errno(EINTR));
                }
                let n = fs.read(node, offset, chunk).map_err(|errno| {
                    trace(b'%');
                    errno
                })?;
                if n == 0 || n > chunk.len() {
                    trace(b'%');
                    return Err(Errno(EIO));
                }
                offset += n as u64;
                chunk = &mut chunk[n..];
            }
        }
        Ok(())
    };
    let image = consume(attrs.size, &mut read)?;
    trace(b'7');
    Ok((image, identity))
}

// Retain ARM's bounded exec breadcrumbs at the shared loader's actual stages.
fn trace(marker: u8) {
    #[cfg(all(target_arch = "aarch64", not(test)))]
    crate::arch_impl::aarch64::trace::trace_exec(marker);
    #[cfg(any(not(target_arch = "aarch64"), test))]
    let _ = marker;
}

//! Explicit disk mounts and native mount teardown.
//!
//! `unmount` and `mount_root_volume` also build in tools/vfs-host-tests, so
//! the host drives the same native entry the umount syscall calls.
use super::vfs::BreenFs;
#[cfg(not(test))]
use super::{mount_args, probe::probe};
#[cfg(not(test))]
use crate::block::BlockDevice;
#[cfg(not(test))]
use crate::block::cache_kernel::KernelWait as SystemWait;
#[cfg(test)]
use crate::block::cache_kernel::HostWait as SystemWait;
#[cfg(not(test))]
use crate::fs::{
    namei,
    permissions::Credentials,
    vfs::{mount as registry, MountFlags},
};
#[cfg(test)]
use crate::statfs_support::mount as registry;
use crate::fs::{namei::Target, vfs::{mount_table::InodeKey, FileSystem}};
use crate::syscall::errno::*;
#[cfg(not(test))]
use crate::syscall::{at_path::AT_FDCWD, metadata::resolve_at, userptr};
use alloc::{sync::Arc, vec::Vec};

#[cfg(not(test))]
pub fn mount(source: u64, target: u64, flags: u64, data: u64, cred: &Credentials) -> Result<(), u64> {
    mount_args::validate(cred.euid, flags)?;
    let data = if data == 0 { None } else {
        Some(userptr::copy_path_bytes_from_user(data)?)
    };
    mount_args::parse_root(data.as_deref())?;
    let source = userptr::copy_path_bytes_from_user(source)?;
    let raw = userptr::copy_path_bytes_from_user(target)?;
    if source == b"root-volume" {
        let name = mount_args::parse_root(data.as_deref())?.ok_or(EINVAL as u64)?;
        let (point, cover, parent) = resolve_cover(raw, cred)?;
        let path = point.path.clone();
        let custody: Arc<dyn Send + Sync> = Arc::try_new(point).map_err(|_| ENOMEM as u64)?;
        return mount_root_volume(name, flags, path, cover, parent, custody);
    }
    let device: Arc<dyn BlockDevice> = if source == b"boot" {
        let path = core::str::from_utf8(&raw).map_err(|_| EINVAL as u64)?;
        crate::block::provisioning_device(path).map_err(|_| ENODEV as u64)?
    } else {
        let disk = mount_args::parse(cred.euid, flags, &source)?;
        crate::fs::devfs::disk_device(disk).ok_or(ENXIO as u64)?
    }.into();
    if !probe(device.as_ref()).map_err(|e| e.0 as u64)? {
        return Err(EINVAL as u64);
    }

    let (point, cover, parent) = resolve_cover(raw, cred)?;
    let id = registry::allocate_id().map_err(|e| e.0 as u64)?;
    let cache = crate::block::cache_kernel::system_cache();
    let fs = BreenFs::mount_named_cached(device, cache, id, data.as_deref(), flags == 0)
    .map_err(|e| e.0 as u64)?;
    let root = fs.root().map_err(|e| e.0 as u64)?;
    let mut entry = registry::MountPoint::new(
        id,
        point.path.clone(),
        "breenfs",
        MountFlags {
            read_only: flags == 1,
            ..MountFlags::default()
        },
        registry::Backend::Fs(fs.clone()),
        root,
        Some(cover),
        Some(parent),
    );
    entry.source = source;
    entry.custody = Some(Arc::try_new(point).map_err(|_| ENOMEM as u64)?);
    // Publish after validating the source, recovery state and mount point.
    // Start before publication: a visible writable mount can receive fsync.
    fs.init_service().map_err(|e| e.0 as u64)?;
    if let Err(error) = registry::register(entry) {
        fs.shutdown("mount", u64::MAX).map_err(|e| e.0 as u64)?;
        return Err(error.0 as u64);
    }
    Ok(())
}

/// Resolve a mount point that is a directory and not already a mount, and
/// its parent. The walk is retained as the mount's custody.
#[cfg(not(test))]
fn resolve_cover(
    raw: Vec<u8>,
    cred: &Credentials,
) -> Result<(namei::Resolved<Vec<u8>>, InodeKey, InodeKey), u64> {
    let point = resolve_at(AT_FDCWD, &raw, true, cred)?;
    let Target::Inode {
        mount: covered_mount,
        ino: covered_ino,
        ..
    } = point.target
    else {
        return Err(EBUSY as u64);
    };
    if point.is_mount_point() || registry::path_is_mount(&point.path) {
        return Err(EBUSY as u64);
    }
    // Verify a directory and retain the resolved walk until after publication.
    namei::working_dir(&point)?;
    let mut parent_path = raw;
    parent_path.try_reserve(3).map_err(|_| ENOMEM as u64)?;
    parent_path.extend_from_slice(b"/..");
    let parent = resolve_at(AT_FDCWD, &parent_path, true, cred)?;
    let Target::Inode {
        mount: parent_mount,
        ino: parent_ino,
        ..
    } = parent.target
    else {
        return Err(EBUSY as u64);
    };
    Ok((point, (covered_mount.0, covered_ino), (parent_mount.0, parent_ino)))
}

/// `mount("root-volume", dir, "breenfs", 0, "root=<name>")`: serve writable
/// root `name` of the volume mounted at `/` at `path`, through the writer
/// that already owns it (docs/metal/paired-generations.md 3.2). `custody`
/// holds the covered directory until unmount.
pub fn mount_root_volume(
    name: &[u8],
    flags: u64,
    path: Vec<u8>,
    cover: InodeKey,
    parent: InodeKey,
    custody: Arc<dyn Send + Sync>,
) -> Result<(), u64> {
    // GenerationView already gives read-only access to any root.
    if flags != 0 {
        return Err(EINVAL as u64);
    }
    let writer_id = registry::find_mount("/").ok_or(ENODEV as u64)?;
    let writer = system_writer(writer_id).ok_or(ENODEV as u64)?;
    if writer.read_only().map_err(|e| e.0 as u64)? {
        return Err(ENODEV as u64);
    }
    let root = writer.attach_staging(name).map_err(|e| e.0 as u64)?;
    let published = (|| {
        let id = registry::allocate_id()?;
        let fs: Arc<dyn FileSystem> = writer.clone();
        let mut entry = registry::MountPoint::new(
            id,
            path,
            "breenfs",
            Default::default(),
            registry::Backend::Fs(fs),
            root,
            Some(cover),
            Some(parent),
        );
        entry.bind = Some((writer_id, root));
        entry.custody = Some(custody);
        entry.source.try_reserve_exact(name.len()).map_err(|_| crate::fs::errno::Errno(ENOMEM))?;
        entry.source.extend_from_slice(name);
        registry::register(entry)
    })();
    if let Err(error) = published {
        // Nothing reached the tree; a refused detach leaves it claimed until reboot.
        writer.detach_staging().map_err(|e| e.0 as u64)?;
        return Err(error.0 as u64);
    }
    Ok(())
}

/// The writable BreenFS writer registered as mount `id`, if it is one.
fn system_writer(id: usize) -> Option<Arc<BreenFs<SystemWait>>> {
    let fs: Arc<dyn core::any::Any + Send + Sync> = registry::filesystem(id).ok()?;
    fs.downcast::<BreenFs<SystemWait>>().ok()
}

/// The writer behind a removed staging mount. The caller drops its own walk
/// of the mount point, which holds the staging root, before `finish`.
#[must_use]
pub struct StagingDetach(Arc<BreenFs<SystemWait>>);

impl StagingDetach {
    pub fn finish(self) -> Result<(), u64> {
        self.0.detach_staging().map_err(|e| e.0 as u64)
    }
}

/// Explicitly registered native breenfs roots enter this path. Other
/// targets fall through to the caller's existing unmount behavior. A
/// staging mount returns its writer, for `StagingDetach::finish`.
pub fn unmount(target: &Target, flags: u64) -> Option<Result<Option<StagingDetach>, u64>> {
    let Target::Inode { mount, ino, .. } = target else {
        return None;
    };
    if registry::get_mount_info(mount.0)?.1 != "breenfs"
        || registry::root_inode(mount.0)? != *ino
        || registry::covered_directory(mount.0).is_none()
    {
        return None;
    }
    Some((|| {
        // A staging mount first (3.2): removal applies the table's busy check
        // before any change, then the caller drops its walk, then detach.
        if let Some(writer) = system_writer(mount.0).filter(|writer| writer.is_staging_root(*ino)) {
            registry::unmount(mount.0, flags != 0).map_err(|error| error.0 as u64)?;
            return Ok(Some(StagingDetach(writer)));
        }
        if registry::is_bind(mount.0) {
            return registry::unmount(mount.0, flags != 0).map(|()| None).map_err(|error| error.0 as u64);
        }
        let fs = registry::filesystem(mount.0).map_err(|e| e.0 as u64)?;
        let deadline = native_deadline();
        fs.shutdown("umount", deadline).map_err(|e| e.0 as u64)?;
        if let Err(error) = registry::unmount(mount.0, flags != 0) {
            // Custody can refuse removal; keep the registered mount usable.
            fs.init_service().map_err(|e| e.0 as u64)?;
            return Err(error.0 as u64);
        }
        Ok(None)
    })())
}

/// Ten seconds from now, for a native mount's shutdown.
#[cfg(not(test))]
fn native_deadline() -> u64 {
    let (sec, nsec) = crate::time::get_monotonic_time_ns();
    (sec as u64)
        .saturating_mul(1_000_000_000)
        .saturating_add(nsec as u64)
        .saturating_add(10_000_000_000)
}
#[cfg(test)]
fn native_deadline() -> u64 {
    u64::MAX
}

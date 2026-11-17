//! Explicit disk mounts and native mount teardown.
use super::{mount_args, probe::probe, vfs::BreenFs};
use crate::block::BlockDevice;
use crate::fs::{
    namei::{self, Target},
    permissions::Credentials,
    vfs::{mount as registry, FileSystem, MountFlags},
};
use crate::syscall::{at_path::AT_FDCWD, errno::*, metadata::resolve_at, userptr};
use alloc::sync::Arc;

pub fn mount(source: u64, target: u64, flags: u64, data: u64, cred: &Credentials) -> Result<(), u64> {
    mount_args::validate(cred.euid, flags)?;
    let data = if data == 0 { None } else {
        Some(userptr::copy_path_bytes_from_user(data)?)
    };
    mount_args::parse_root(data.as_deref())?;
    let source = userptr::copy_path_bytes_from_user(source)?;
    let disk = mount_args::parse(cred.euid, flags, &source)?;
    let device: Arc<dyn BlockDevice> = crate::fs::devfs::disk_device(disk)
        .ok_or(ENXIO as u64)?
        .into();
    if !probe(device.as_ref()).map_err(|e| e.0 as u64)? {
        return Err(EINVAL as u64);
    }

    let raw = userptr::copy_path_bytes_from_user(target)?;
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
        Some((covered_mount.0, covered_ino)),
        Some((parent_mount.0, parent_ino)),
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

/// Explicitly registered native breenfs roots enter this path. Other
/// targets fall through to the caller's existing unmount behavior.
pub fn unmount(target: &Target, flags: u64) -> Option<Result<(), u64>> {
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
        let fs = registry::filesystem(mount.0).map_err(|e| e.0 as u64)?;
        let (sec, nsec) = crate::time::get_monotonic_time_ns();
        let deadline = (sec as u64)
            .saturating_mul(1_000_000_000)
            .saturating_add(nsec as u64)
            .saturating_add(10_000_000_000);
        fs.shutdown("umount", deadline).map_err(|e| e.0 as u64)?;
        if let Err(error) = registry::unmount(mount.0, flags != 0) {
            // Custody can refuse removal; keep the registered mount usable.
            fs.init_service().map_err(|e| e.0 as u64)?;
            return Err(error.0 as u64);
        }
        Ok(())
    })())
}

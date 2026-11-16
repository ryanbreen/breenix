//! Explicit mounts only: neither root nor home boot probing enters here.
use super::{format::BLOCK_SIZE, mount_args, vfs::BreenFs};
use crate::block::BlockDevice;
use crate::fs::{
    namei::{self, Target},
    permissions::Credentials,
    vfs::{mount as registry, FileSystem, MountFlags},
};
use crate::syscall::{at_path::AT_FDCWD, errno::*, metadata::resolve_at, userptr};
use alloc::{sync::Arc, vec::Vec};

// Look in both rings: a valid filesystem can have only a surviving B copy.
// Recovery checks CRCs, feature bits, UUIDs and roots after this type check.
fn require_magic(device: &dyn BlockDevice) -> Result<(), u64> {
    let sector = device.block_size();
    if sector < 8 || sector > BLOCK_SIZE || BLOCK_SIZE % sector != 0 {
        return Err(EINVAL as u64);
    }
    let sectors_per_block = (BLOCK_SIZE / sector) as u64;
    let total = device.num_blocks() / sectors_per_block;
    if total < 24 {
        return Err(EINVAL as u64);
    }
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(sector).map_err(|_| ENOMEM as u64)?;
    bytes.resize(sector, 0);
    for start in [16, total - 4] {
        for slot in 0..4 {
            let sector = (start + slot)
                .checked_mul(sectors_per_block)
                .ok_or(EINVAL as u64)?;
            device
                .read_block(sector, &mut bytes)
                .map_err(|_| EIO as u64)?;
            if mount_args::has_magic(&bytes) {
                return Ok(());
            }
        }
    }
    Err(EINVAL as u64)
}

pub fn mount(source: u64, target: u64, flags: u64, cred: &Credentials) -> Result<(), u64> {
    mount_args::validate(cred.euid, flags)?;
    let source = userptr::copy_path_bytes_from_user(source)?;
    let disk = mount_args::parse(cred.euid, flags, &source)?;
    let device: Arc<dyn BlockDevice> = crate::fs::devfs::disk_device(disk)
        .ok_or(ENXIO as u64)?
        .into();
    require_magic(device.as_ref())?;

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
    let fs = BreenFs::mount_cached(device, crate::block::cache_kernel::system_cache(), id, 1)
        .map_err(|e| e.0 as u64)?;
    let root = fs.root().map_err(|e| e.0 as u64)?;
    let mut entry = registry::MountPoint::new(
        id,
        point.path.clone(),
        "breenfs",
        MountFlags {
            read_only: true,
            ..MountFlags::default()
        },
        registry::Backend::Fs(fs),
        root,
        Some((covered_mount.0, covered_ino)),
        Some((parent_mount.0, parent_ino)),
    );
    entry.source = source;
    entry.custody = Some(Arc::try_new(point).map_err(|_| ENOMEM as u64)?);
    // Publish after validating the source, recovery state and mount point.
    registry::register(entry).map_err(|e| e.0 as u64)
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
    Some(registry::unmount(mount.0, flags != 0).map_err(|e| e.0 as u64))
}

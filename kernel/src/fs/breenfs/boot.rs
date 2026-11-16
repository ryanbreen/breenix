//! Boot format registration. Device selection remains in VFS.
use super::{probe::probe, vfs::BreenFs};
use crate::{
    block::BlockDevice,
    fs::{
        errno::Errno,
        vfs::{mount as registry, BootMount, FileSystem, Format, MountFlags},
    },
    syscall::errno::*,
};
use alloc::{boxed::Box, sync::Arc};

pub const FORMAT: Format = Format {
    #[cfg(feature = "fs_fault_inject")]
    // The ext2 inode-corruption fixture has no breenfs geometry.
    fault_mount: |_| Err(Errno(EOPNOTSUPP)),
    kind: "breenfs",
    probe,
    mount,
};

fn mount(device: Box<dyn BlockDevice>, target: BootMount) -> Result<Arc<dyn FileSystem>, Errno> {
    // The read fixture remains an explicit mount, even when discovery offers
    // its disk as a possible home device. Boot-home integration is not part
    // of the writable test-root path.
    if target == BootMount::Home {
        return Err(Errno(ENODEV));
    }
    let id = registry::allocate_id()?;
    let cache = crate::block::cache_kernel::system_cache();
    let fs = BreenFs::mount_writable_cached(device.into(), cache, id, 1)?;
    registry::register(registry::MountPoint::new(
        id,
        b"/".to_vec(),
        "breenfs",
        MountFlags::default(),
        registry::Backend::Fs(fs.clone()),
        fs.root()?,
        None,
        None,
    ))?;
    // ARM64 mounts storage before scheduler initialization. The existing
    // VFS init_services pass starts this mount's worker once scheduling is ready.
    Ok(fs)
}

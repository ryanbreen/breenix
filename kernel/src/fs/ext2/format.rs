//! Ext2's boot format registration; device selection belongs to VFS.
use crate::{
    block::BlockDevice,
    fs::{
        errno::Errno,
        vfs::{BootMount, FileSystem, Format},
    },
};
use alloc::{boxed::Box, sync::Arc};

pub const FORMAT: Format = Format {
    #[cfg(feature = "fs_fault_inject")]
    fault_mount: super::fault::mount,
    kind: "ext2",
    probe,
    uuid,
    mount,
};

fn probe(device: &dyn BlockDevice) -> Result<bool, Errno> {
    if device.block_size() != 512 || device.num_blocks() < 3 {
        return Ok(false);
    }
    let mut sector = [0; 512];
    device
        .read_block(2, &mut sector)
        .map_err(|_| Errno(crate::syscall::errno::EIO))?;
    Ok(u16::from_le_bytes([sector[56], sector[57]]) == 0xef53)
}

fn uuid(device: &dyn BlockDevice) -> Result<[u8; 16], Errno> {
    let mut sector = [0; 512];
    device
        .read_block(2, &mut sector)
        .map_err(|_| Errno(crate::syscall::errno::EIO))?;
    Ok(sector[104..120].try_into().unwrap())
}

fn mount(
    device: Box<dyn BlockDevice>,
    target: BootMount<'_>,
) -> Result<Arc<dyn FileSystem>, Errno> {
    match target {
        BootMount::Root => crate::fs::ext2::mount_root_device(device),
        BootMount::Home => crate::fs::ext2::mount_home_device(device),
        #[cfg(not(test))]
        BootMount::Explicit(point) => return super::mount_explicit_device(device, point),
        #[cfg(test)]
        BootMount::Explicit(_) => crate::fs::ext2::mount_root_device(device),
    }
    .map_err(crate::fs::ext2::errno::errno)
}

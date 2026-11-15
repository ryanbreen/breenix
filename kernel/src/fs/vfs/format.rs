//! Boot-time on-disk format dispatch through read-only probes.
use super::FileSystem;
use crate::{
    block::BlockDevice,
    fs::errno::Errno,
    syscall::errno::{EIO, ENODEV},
};
use alloc::{boxed::Box, sync::Arc};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BootMount {
    Root,
    Home,
}

pub struct Format {
    #[cfg(feature = "fs_fault_inject")]
    pub fault_mount: fn(Box<dyn BlockDevice>) -> Result<super::fault::FaultImage, Errno>,
    pub kind: &'static str,
    pub probe: fn(&dyn BlockDevice) -> Result<bool, Errno>,
    pub mount: fn(Box<dyn BlockDevice>, BootMount) -> Result<Arc<dyn FileSystem>, Errno>,
}

pub fn probe_mount(
    formats: &[Format],
    device: Box<dyn BlockDevice>,
    target: BootMount,
) -> Result<Arc<dyn FileSystem>, Errno> {
    for format in formats {
        if (format.probe)(device.as_ref())? {
            let filesystem = (format.mount)(device, target)?;
            if filesystem.kind()? != format.kind {
                return Err(Errno(EIO));
            }
            return Ok(filesystem);
        }
    }
    Err(Errno(ENODEV))
}

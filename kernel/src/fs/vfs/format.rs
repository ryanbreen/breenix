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
        let detected = (format.probe)(device.as_ref()).map_err(|error| {
            #[cfg(not(test))]
            if target == BootMount::Root {
                super::boot::root_mount_receipt(Some(device.as_ref()), format_args!(
                    "stage=format-probe expected=readable-{}-magic found={:?}", format.kind, error));
            }
            error
        })?;
        if detected {
            let filesystem = (format.mount)(device, target)?;
            if filesystem.kind()? != format.kind {
                return Err(Errno(EIO));
            }
            return Ok(filesystem);
        }
    }
    #[cfg(not(test))]
    if target == BootMount::Root {
        super::boot::root_mount_receipt(Some(device.as_ref()), format_args!(
            "stage=format-probe expected=ext2-or-breenfs-magic found=unrecognized"));
    }
    Err(Errno(ENODEV))
}

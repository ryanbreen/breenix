//! Device discovery and boot consumers above the filesystem boundary.
use super::{probe_mount, BootMount, FileSystem, Report};
use crate::block::BlockDevice;
use crate::{fs::errno::Errno, syscall::errno::*};
#[cfg(any(
    target_arch = "aarch64",
    all(feature = "testing", not(feature = "interactive"))
))]
use alloc::boxed::Box;
use alloc::{string::String, sync::Arc, vec::Vec};
use core::fmt::{self, Write};

/// Emit one bounded record without allocating or waiting for serial ownership.
pub(crate) fn root_mount_receipt(device: Option<&dyn BlockDevice>, details: fmt::Arguments<'_>) {
    struct Line {
        bytes: [u8; 256],
        len: usize,
    }
    impl Write for Line {
        fn write_str(&mut self, text: &str) -> fmt::Result {
            let end = self.len + text.len();
            let slot = self.bytes.get_mut(self.len..end).ok_or(fmt::Error)?;
            slot.copy_from_slice(text.as_bytes());
            self.len = end;
            Ok(())
        }
    }
    let mut line = Line { bytes: [0; 256], len: 0 };
    let _ = line.write_str("\n[ROOT_MOUNT_FAILURE ");
    if let Some(device) = device {
        let _ = write!(line, "device={:?}/{:x} ", device.hardware_disk(), device.identity());
    } else {
        let _ = line.write_str("device=none ");
    }
    if write!(line, "{}]\n", details).is_err() {
        // Keep the framing even when a source error exceeds the receipt's budget.
        let end = line.len.min(line.bytes.len() - 5);
        line.bytes[end..end + 5].copy_from_slice(b"...]\n");
        line.len = end + 5;
    }
    if let Ok(text) = core::str::from_utf8(&line.bytes[..line.len]) {
        #[cfg(target_arch = "x86_64")]
        crate::serial::write_str_briefly(text);
        #[cfg(target_arch = "aarch64")]
        crate::serial_aarch64::write_str_bounded(text);
    }
}

/// A request available before the root mounts (fw_cfg or the boot loader's
/// intent). The on-disk boot-target file cannot be read on a failed mount.
pub fn suite_boot_requested() -> bool {
    let mut mode = [0u8; 256];
    #[cfg(target_arch = "x86_64")]
    let fw_cfg = true;
    #[cfg(target_arch = "aarch64")]
    let fw_cfg = crate::platform_config::is_qemu();
    if fw_cfg {
        let size = crate::drivers::fw_cfg::read_file("opt/breenix/mode", &mut mode);
        if size != 0 {
            return mode.get(..size)
                .and_then(|bytes| core::str::from_utf8(bytes).ok())
                .is_some_and(|text| text.trim_end_matches('\0').trim() == "suite");
        }
    }
    crate::boot::intent::get().is_some_and(|target| target.starts_with(b"suite "))
}

pub fn mount_root() -> Result<(), Errno> {
    #[cfg(target_arch = "x86_64")]
    let devices = crate::block::root_devices();
    #[cfg(target_arch = "aarch64")]
    let devices: Vec<Box<dyn BlockDevice>> = {
        use crate::block::virtio::VirtioBlockWrapper;
        let mut devices: Vec<Box<dyn BlockDevice>> = Vec::new();
        if let Some(dev) = VirtioBlockWrapper::new(0) {
            devices.push(Box::new(dev));
        } else {
            for i in 0..crate::drivers::ahci::sata_device_count() {
                if let Some(dev) = crate::drivers::ahci::get_block_device_by_index(i) {
                    devices.push(Box::new(dev));
                }
            }
        }
        devices
    };
    let mut error = Errno(ENODEV);
    if devices.is_empty() {
        root_mount_receipt(None, format_args!("stage=discovery expected=root-device found=absent"));
    }
    for device in devices {
        match probe_mount(crate::fs::FORMATS, device, BootMount::Root) {
            Ok(_) => return Ok(()),
            Err(e) => error = e,
        }
    }
    Err(error)
}

pub fn mount_home() -> Result<(), Errno> {
    #[cfg(target_arch = "x86_64")]
    let device = if crate::block::hardware_read_only() {
        crate::block::named_home()
    } else {
        crate::block::disk(3).or_else(|| crate::block::disk(1))
    };
    #[cfg(target_arch = "aarch64")]
    let device = {
        use crate::block::virtio::VirtioBlockWrapper;
        VirtioBlockWrapper::new(1)
            .map(|dev| Box::new(dev) as Box<dyn BlockDevice>)
    };
    #[cfg(all(feature = "testing", not(feature = "interactive")))]
    let device = device.or_else(|| {
        Some(Box::new(crate::block::testing_fixture::ReadOnlyFixture) as Box<dyn BlockDevice>)
    });
    probe_mount(
        crate::fs::FORMATS,
        device.ok_or(Errno(ENODEV))?,
        BootMount::Home,
    )?;
    Ok(())
}

fn storage() -> Result<Vec<(usize, Arc<dyn FileSystem>)>, Errno> {
    super::mount::storage_mounts()
        .into_iter()
        .map(|id| super::mount::filesystem(id).map(|fs| (id, fs)))
        .collect()
}

pub fn init_services() -> Result<(), Errno> {
    for (_, fs) in storage()? {
        fs.init_service()?;
    }
    Ok(())
}

pub fn shutdown() -> Result<(), Errno> {
    // The home mount shuts down before root, as it did before the trait move.
    let (sec, nsec) = crate::time::get_monotonic_time_ns();
    let deadline = (sec as u64)
        .saturating_mul(1_000_000_000)
        .saturating_add(nsec as u64)
        .saturating_add(10_000_000_000);
    let mut mounts = storage()?;
    mounts.reverse();
    for (id, fs) in mounts {
        let name = super::mount::get_mount_info(id).ok_or(Errno(EIO))?;
        fs.shutdown(if name.0 == "/" { "root" } else { "home" }, deadline)?;
    }
    Ok(())
}

pub fn report(report: Report) -> Result<String, Errno> {
    let mut text = String::new();
    for (id, fs) in storage()? {
        let info = super::mount::get_mount_info(id).ok_or(Errno(EIO))?;
        fs.report(
            report,
            if info.0 == "/" { "root" } else { "home" },
            &mut text,
        )?;
    }
    Ok(text)
}

/// Counter transport used by failure observations, without backend state locks.
pub fn mutation_costs() -> Result<String, Errno> {
    let mut text = String::new();
    for (id, fs) in storage()? {
        let info = super::mount::get_mount_info(id).ok_or(Errno(EIO))?;
        fs.mutation_costs(if info.0 == "/" { "root" } else { "home" }, &mut text)?;
    }
    Ok(text)
}

/// Configuration reads retain the resolved VNode and use only filesystem ops.
pub fn read_config(path: &str, max: usize) -> Result<Vec<u8>, Errno> {
    let (_, _, node, _) = crate::fs::namei::resolve_file(path).map_err(|e| Errno(e as i32))?;
    read_config_node(&node, max)
}

pub fn read_config_node(node: &super::VNode, max: usize) -> Result<Vec<u8>, Errno> {
    let attrs = node.filesystem.getattr(node.ino)?;
    if attrs.kind == super::NodeKind::Directory {
        return Err(Errno(EISDIR));
    }
    if attrs.kind != super::NodeKind::Regular {
        return Err(Errno(EINVAL));
    }
    if attrs.size > max as u64 {
        return Err(Errno(EFBIG));
    }
    let size = attrs.size as usize;
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(size).map_err(|_| Errno(ENOMEM))?;
    bytes.resize(size, 0);
    let mut at = 0;
    while at < size {
        let read = node.filesystem.read(node, at as u64, &mut bytes[at..])?;
        if read == 0 {
            return Err(Errno(EIO));
        }
        at += read;
    }
    Ok(bytes)
}

/// Retain the mounted root backend without holding the table during I/O.
pub fn root_filesystem() -> Result<Arc<dyn FileSystem>, Errno> {
    let id = super::mount::find_mount("/").ok_or(Errno(ENODEV))?;
    super::mount::filesystem(id)
}

/// Boot diagnostics name the backend retained by the mounted root.
pub fn root_kind() -> &'static str {
    root_filesystem().and_then(|fs| fs.kind()).unwrap_or("unmounted")
}

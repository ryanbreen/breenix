//! Device discovery and boot consumers above the filesystem boundary.
#[cfg(not(test))]
use crate::fs::vfs::{mount as registry, probe_mount, Report};
#[cfg(test)]
use crate::statfs_support::mount as registry;
use crate::{
    block::BlockDevice,
    boot::intent::Captured,
    fs::{
        errno::Errno,
        vfs::{
            format::{detect, ExplicitMount},
            mount_table::InodeKey,
            BootMount, FileSystem, Format,
        },
    },
    syscall::errno::*,
};
#[cfg(not(test))]
use alloc::string::String;
use alloc::{boxed::Box, sync::Arc, vec::Vec};
use core::fmt;
#[cfg(not(test))]
use core::fmt::Write;

pub(crate) struct MountFailure<'a> {
    source: Option<(&'a str, &'a str)>,
    reason: &'static str,
}

impl fmt::Debug for MountFailure<'_> {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, out)
    }
}

impl fmt::Display for MountFailure<'_> {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(out, "stage=boot-mount")?;
        if let Some((guid, path)) = self.source {
            write!(out, " guid={guid} path={path}")?;
        }
        write!(out, " reason={}", self.reason)
    }
}

pub(crate) struct MountLine<'a> {
    guid: &'a str,
    path: &'a str,
    writable: bool,
    kind: &'static str,
    device: Box<dyn BlockDevice>,
}

/// Resolve and probe the whole set before publishing a mount or a write grant.
pub(crate) fn prepare_mounts<'a>(
    intent: Option<&'a Captured>,
    formats: &[Format],
    mut resolve: impl FnMut(&str, &str, bool) -> Result<Box<dyn BlockDevice>, &'static str>,
) -> Result<Option<Vec<MountLine<'a>>>, MountFailure<'a>> {
    let Some(intent) = intent else {
        return Ok(None);
    };
    if !intent.explicit_mounts().map_err(|reason| MountFailure {
        source: None,
        reason,
    })? {
        return Ok(None);
    }

    let mut mounts: Vec<MountLine<'_>> = Vec::new();
    for (guid, path, writable) in intent.mounts() {
        let fail = |reason| MountFailure {
            source: Some((guid, path)),
            reason,
        };
        if path != "/" && path[1..].split('/').any(str::is_empty) {
            return Err(fail("the mount path is not canonical"));
        }
        if mounts
            .iter()
            .any(|line| (line.writable || writable) && line.guid.eq_ignore_ascii_case(guid))
        {
            return Err(fail("a writable source has more than one mount line"));
        }
        let device = resolve(guid, path, writable).map_err(fail)?;
        let format = detect(formats, device.as_ref())
            .map_err(|_| fail("the filesystem magic is unreadable"))?
            .ok_or_else(|| fail("the filesystem magic is unrecognized"))?;
        mounts.push(MountLine {
            guid,
            path,
            writable,
            kind: format.kind,
            device,
        });
    }
    // The cover for a nested mount belongs to the closest mounted ancestor.
    mounts.sort_by_key(|line| {
        (
            line.path != "/",
            line.path.bytes().filter(|&b| b == b'/').count(),
        )
    });
    Ok(Some(mounts))
}

impl<'a> MountLine<'a> {
    pub(crate) fn point(
        &self,
        id: usize,
        cover: Option<InodeKey>,
        parent: Option<InodeKey>,
    ) -> ExplicitMount<'a> {
        let mut flags = if self.path == "/home" {
            registry::HOME_FLAGS
        } else {
            Default::default()
        };
        flags.read_only = !self.writable;
        ExplicitMount {
            id,
            path: self.path,
            flags,
            cover,
            parent,
        }
    }

    pub(crate) fn mount(
        self,
        formats: &[Format],
        point: &ExplicitMount<'_>,
    ) -> Result<Arc<dyn FileSystem>, Errno> {
        let device = if self.writable {
            if self.device.is_read_only() {
                return Err(Errno(EROFS));
            }
            self.device
        } else {
            crate::block::read_only::device(self.device)
        };
        let format = formats
            .iter()
            .find(|format| format.kind == self.kind)
            .ok_or(Errno(ENODEV))?;
        (format.mount)(device, BootMount::Explicit(point))
    }
}

impl ExplicitMount<'_> {
    pub(crate) fn entry(&self, fs: Arc<dyn FileSystem>) -> Result<registry::MountPoint, Errno> {
        Ok(registry::MountPoint::new(
            self.id,
            self.path.as_bytes().to_vec(),
            fs.kind()?,
            self.flags,
            registry::Backend::Fs(fs.clone()),
            fs.root()?,
            self.cover,
            self.parent,
        ))
    }
}

#[cfg(not(test))]
fn explicit_covers(path: &str) -> Result<(Option<InodeKey>, Option<InodeKey>), Errno> {
    use crate::fs::namei::{resolve, working_dir, Target};

    if path == "/" {
        return Ok((None, None));
    }
    let point = resolve(path, false).map_err(|error| Errno(error as i32))?;
    if point.path != path || point.is_mount_point() {
        return Err(Errno(EBUSY));
    }
    working_dir(&point).map_err(|error| Errno(error as i32))?;
    let Target::Inode { mount, ino, .. } = point.target else {
        return Err(Errno(ENOTDIR));
    };
    let (parent_path, _) = path.rsplit_once('/').ok_or(Errno(EINVAL))?;
    let parent = resolve(
        if parent_path.is_empty() {
            "/"
        } else {
            parent_path
        },
        true,
    )
    .map_err(|error| Errno(error as i32))?;
    let Target::Inode {
        mount: parent_mount,
        ino: parent_ino,
        ..
    } = parent.target
    else {
        return Err(Errno(ENOTDIR));
    };
    Ok((Some((mount.0, ino)), Some((parent_mount.0, parent_ino))))
}

#[cfg(not(test))]
fn mount_explicit() -> Result<(), Errno> {
    use crate::{block, boot::intent, fs::FORMATS};

    #[cfg(target_arch = "x86_64")]
    let (mut fence, mut root_role) = (block::Fence::CLOSED, 0);
    let plan = prepare_mounts(intent::captured(), FORMATS, |guid, _path, _writable| {
        #[cfg(target_arch = "x86_64")]
        {
            block::explicit_mount_device(guid, _path, _writable, &mut fence, &mut root_role)
        }
        #[cfg(target_arch = "aarch64")]
        {
            block::explicit_mount_device(guid)
        }
    })
    .map_err(|failure| {
        root_mount_receipt(None, format_args!("{failure}"));
        Errno(ENODEV)
    })?
    .ok_or(Errno(EINVAL))?;

    #[cfg(target_arch = "x86_64")]
    block::publish_mount_fence(fence, root_role).map_err(|reason| {
        root_mount_receipt(None, format_args!("stage=boot-mount-fence reason={reason}"));
        Errno(EIO)
    })?;

    for line in plan {
        let (path, guid) = (line.path, line.guid);
        let mounted = (|| {
            let (cover, parent) = explicit_covers(path)?;
            let point = line.point(registry::allocate_id()?, cover, parent);
            line.mount(FORMATS, &point)
        })();
        if let Err(error) = mounted {
            root_mount_receipt(
                None,
                format_args!("stage=boot-mount path={path} guid={guid} errno={}", error.0),
            );
            return Err(error);
        }
    }
    Ok(())
}

/// Emit one bounded record without allocating or waiting for serial ownership.
#[cfg(not(test))]
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
#[cfg(not(test))]
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

#[cfg(not(test))]
pub fn mount_root() -> Result<(), Errno> {
    if crate::boot::intent::explicit_mounts_requested() {
        return mount_explicit();
    }
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

#[cfg(not(test))]
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

#[cfg(not(test))]
fn storage() -> Result<Vec<(usize, Arc<dyn FileSystem>)>, Errno> {
    super::mount::storage_mounts()
        .into_iter()
        .map(|id| super::mount::filesystem(id).map(|fs| (id, fs)))
        .collect()
}

#[cfg(not(test))]
pub fn init_services() -> Result<(), Errno> {
    for (_, fs) in storage()? {
        fs.init_service()?;
    }
    Ok(())
}

#[cfg(not(test))]
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

#[cfg(not(test))]
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
#[cfg(not(test))]
pub fn mutation_costs() -> Result<String, Errno> {
    let mut text = String::new();
    for (id, fs) in storage()? {
        let info = super::mount::get_mount_info(id).ok_or(Errno(EIO))?;
        fs.mutation_costs(if info.0 == "/" { "root" } else { "home" }, &mut text)?;
    }
    Ok(text)
}

/// Configuration reads retain the resolved VNode and use only filesystem ops.
#[cfg(not(test))]
pub fn read_config(path: &str, max: usize) -> Result<Vec<u8>, Errno> {
    let (_, _, node, _) = crate::fs::namei::resolve_file(path).map_err(|e| Errno(e as i32))?;
    read_config_node(&node, max)
}

#[cfg(not(test))]
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
#[cfg(not(test))]
pub fn root_filesystem() -> Result<Arc<dyn FileSystem>, Errno> {
    let id = super::mount::find_mount("/").ok_or(Errno(ENODEV))?;
    super::mount::filesystem(id)
}

/// Boot diagnostics name the backend retained by the mounted root.
#[cfg(not(test))]
pub fn root_kind() -> &'static str {
    root_filesystem().and_then(|fs| fs.kind()).unwrap_or("unmounted")
}

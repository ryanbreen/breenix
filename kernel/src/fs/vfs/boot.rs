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
    generation: Option<&'a str>,
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
    let userland = intent.userland();
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
        if writable && format.kind != "breenfs" {
            return Err(fail("format-3 mounts require read-only access to non-BreenFS volumes"));
        }
        mounts.push(MountLine {
            guid,
            path,
            writable,
            kind: format.kind,
            generation: userland.filter(|_| path == "/" && writable && format.kind == "breenfs"),
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
        flags.read_only = !self.writable || self.generation.is_some();
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
        if let Some(generation) = self.generation {
            return mount_cutover(point, device.into(), generation);
        }
        let format = formats
            .iter()
            .find(|format| format.kind == self.kind)
            .ok_or(Errno(ENODEV))?;
        (format.mount)(device, BootMount::Explicit(point))
    }
}

/// Recover the default root's writer before attaching its generation view.
fn mount_cutover(
    point: &ExplicitMount<'_>,
    device: Arc<dyn BlockDevice>,
    generation: &str,
) -> Result<Arc<dyn FileSystem>, Errno> {
    use crate::fs::{breenfs::vfs::BreenFs, vfs::NodeKind};

    // /root is the writer's native inode identity. The other binds use this
    // same identity, distinct from the generation's overlapping inode ids.
    let writer_id = registry::allocate_id()?;
    let writer = BreenFs::mount_writable_cached(
        device,
        crate::block::cache_kernel::system_cache(),
        writer_id,
        1,
    )?;
    let name = alloc::format!("gen-{generation}");
    let view = writer.generation_view(point.id, name.as_bytes())?;
    let root = view.root()?;
    let default = writer.root()?;
    let shared = writer.lookup(default, b".breenix-shared")?;
    let mut entries = Vec::new();
    entries.push(point.entry(view.clone())?);
    let writer: Arc<dyn FileSystem> = writer;
    let mut bind = |path: &str, source, cover_parent, cover| -> Result<(), Errno> {
        if writer.getattr(source)?.kind != NodeKind::Directory
            || view.getattr(cover)?.kind != NodeKind::Directory
        {
            return Err(Errno(ENOTDIR));
        }
        let id = if path == "/root" { writer_id } else { registry::allocate_id()? };
        let mut entry = registry::MountPoint::new(
            id,
            path.as_bytes().to_vec(),
            "breenfs",
            Default::default(),
            registry::Backend::Fs(writer.clone()),
            source,
            Some((point.id, cover)),
            Some((point.id, cover_parent)),
        );
        entry.bind = Some((writer_id, source));
        entry.custody = Some(writer.open(source)?);
        entries.push(entry);
        Ok(())
    };
    for (path, source_parent) in [
        ("/root", default),
        ("/etc", shared),
        ("/var", shared),
        ("/opt", shared),
        ("/srv", shared),
        ("/tmp", shared),
        ("/run", shared),
    ] {
        let name = &path.as_bytes()[1..];
        let source = writer.lookup(source_parent, name)?;
        let cover = view.lookup(root, name)?;
        bind(path, source, root, cover)?;
    }
    // A rollback may select an old image without a cover. New generations
    // reserve it, and an existing cover also works with a kernel-only flash.
    match view.lookup(root, b"usr").and_then(|usr| {
        view.lookup(usr, b"local").map(|cover| (usr, cover))
    }) {
        Ok((usr, cover)) => {
            let source = ensure_usr_local(writer.as_ref(), shared)?;
            bind("/usr/local", source, usr, cover)?;
        }
        Err(Errno(ENOENT)) => {}
        Err(error) => return Err(error),
    }
    registry::register_cutover(entries, writer_id)?;
    Ok(view)
}

/// Initialize the live layer in the writer's default root, outside OS snapshots.
fn ensure_usr_local(writer: &dyn FileSystem, shared: u64) -> Result<u64, Errno> {
    use crate::fs::{
        attrs::InodeAttrs,
        vfs::{NewNode, NodeKind},
    };

    // The system account layout assigns wheel GID 10.
    const WHEEL_GID: u32 = 10;
    let ensure_directory = |parent, name: &[u8], attrs| match writer.lookup(parent, name) {
        Ok(ino) if writer.getattr(ino)?.kind == NodeKind::Directory => Ok(ino),
        Ok(_) => Err(Errno(ENOTDIR)),
        Err(Errno(ENOENT)) => writer.create(parent, name, NewNode::Directory { attrs }),
        Err(error) => Err(error),
    };
    let local = ensure_directory(shared, b"usr-local", InodeAttrs::new(0o755, 0, 0))?;
    for name in [b"bin".as_slice(), b"sbin", b"lib"] {
        ensure_directory(local, name, InodeAttrs::new(0o2775, 0, WHEEL_GID))?;
    }
    // The normal commit worker starts in init_services, after root publication.
    Ok(local)
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

/// Format a fresh, zeroed boot device using the regular BreenFS writer.
pub(crate) fn format_volatile(device: &dyn BlockDevice) -> Result<(), Errno> {
    use crate::fs::breenfs::{
        commit::{partition_blocks, Committer, DeviceStore},
        format::{Superblock, Time},
        ops::Files,
    };

    let mut seed = [0; 32];
    #[cfg(not(test))]
    crate::random::fill(&mut seed);
    #[cfg(test)]
    seed.fill(1);
    let time = Time {
        seconds: crate::time::current_unix_time() as i64,
        nanos: 0,
    };
    let sb = Superblock {
        fs_uuid: seed[..16].try_into().unwrap(),
        dirhash_key: seed[16..].try_into().unwrap(),
        generation: 1,
        slot: 1,
        total_blocks: partition_blocks(device).map_err(|_| Errno(EIO))?,
        global_reserve: 4096,
        next_root_id: 2,
        mkfs_time: time,
        commit_time: time,
        ..Superblock::default()
    };
    let mut files = Files::format(DeviceStore(device), &sb).map_err(|_| Errno(EIO))?;
    for (name, mode) in [(b"tmp".as_slice(), 0o1777), (b"run", 0o755)] {
        files.mkdir(2, name, mode, time).map_err(|_| Errno(EIO))?;
    }
    files.fsync(device, &mut Committer::format(sb)).map_err(|_| Errno(EIO))?;
    Ok(())
}

pub(crate) fn mount_volatile_if_needed(
    intent: &Captured,
    device: impl FnOnce() -> Result<Arc<dyn BlockDevice>, Errno>,
) -> Result<(), Errno> {
    use crate::fs::{breenfs::vfs::BreenFs, vfs::NodeKind};

    if !intent.explicit_mounts().map_err(|_| Errno(EINVAL))? {
        return Ok(());
    }
    let root_id = registry::find_mount("/").ok_or(Errno(ENODEV))?;
    let root = registry::filesystem(root_id)?;
    let writable_system = intent.mounts().any(|(_, path, writable)| path == "/" && writable)
        && root.kind()? == "breenfs";
    if writable_system || !root.read_only()? {
        return Ok(());
    }
    let root_ino = registry::root_inode(root_id).ok_or(Errno(EIO))?;
    let cover = |name: &[u8]| {
        let ino = root.lookup(root_ino, name)?;
        if root.getattr(ino)?.kind != NodeKind::Directory {
            return Err(Errno(ENOTDIR));
        }
        Ok((root_id, ino))
    };
    let (tmp_cover, run_cover) = (cover(b"tmp")?, cover(b"run")?);

    let device = device()?;
    format_volatile(device.as_ref())?;
    let (tmp_id, run_id) = (registry::allocate_id()?, registry::allocate_id()?);
    let fs = BreenFs::mount_writable_cached(
        device,
        crate::block::cache_kernel::system_cache(),
        tmp_id,
        1,
    )?;
    let source = fs.root()?;
    let make_entry = |id, path: &[u8], cover| {
        let ino = fs.lookup(source, &path[1..])?;
        let mut entry = registry::MountPoint::new(
            id,
            path.to_vec(),
            fs.kind()?,
            Default::default(),
            registry::Backend::Fs(fs.clone()),
            ino,
            Some(cover),
            Some((root_id, root_ino)),
        );
        entry.custody = Some(fs.open(ino)?);
        Ok::<_, Errno>(entry)
    };
    let tmp = make_entry(tmp_id, b"/tmp", tmp_cover)?;
    let run = make_entry(run_id, b"/run", run_cover)?;
    registry::register_volatile_mounts(tmp, run)
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
    let captured = intent::captured().ok_or(Errno(EINVAL))?;
    let plan = prepare_mounts(Some(captured), FORMATS, |guid, _path, _writable| {
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
    mount_volatile_if_needed(captured, || {
        let disk = block::memory_device::MemoryDevice::new().map_err(|_| Errno(ENOMEM))?;
        let disk = Arc::try_new(disk).map_err(|_| Errno(ENOMEM))?;
        Ok(disk)
    }).inspect_err(|error| {
        root_mount_receipt(None, format_args!("stage=boot-volatile-mount errno={}", error.0));
    })
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
        let name = match info.0.as_str() {
            "/" => "root",
            "/home" => "home",
            path => path,
        };
        fs.mutation_costs(name, &mut text)?;
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

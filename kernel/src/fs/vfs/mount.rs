//! Kernel mount registry. Backends and covers share one identity space.
use super::{
    mount_table::{Entry, InodeKey, MountTable},
    FileSystem,
};
pub use crate::fs::attrs::MountFlags;
use crate::fs::errno::Errno;
use alloc::{string::String, sync::Arc, vec::Vec};
use spin::Mutex;

pub enum Backend {
    Fs(Arc<dyn FileSystem>),
    User(u64),
    Virtual,
}
pub type MountPoint = Entry<Backend>;
static MOUNT_TABLE: Mutex<MountTable<Backend>> = Mutex::new(MountTable::new());

pub const HOME_FLAGS: MountFlags = MountFlags {
    read_only: false,
    nosuid: true,
    nodev: true,
    noexec: false,
};

pub fn allocate_id() -> Result<usize, Errno> {
    MOUNT_TABLE.lock().allocate_id()
}
pub fn register(entry: MountPoint) -> Result<(), Errno> {
    MOUNT_TABLE.lock().insert(entry)
}

/// Virtual filesystems retain their boot registration order and ids.
pub fn mount(path: &str, kind: &'static str) -> usize {
    let mut table = MOUNT_TABLE.lock();
    let id = table.allocate_id().expect("mount id exhausted");
    let parent = table.find_mount(path.as_bytes()).map(|id| (id, 0));
    table
        .insert(Entry::new(
            id,
            path.as_bytes().to_vec(),
            kind,
            MountFlags::default(),
            Backend::Virtual,
            0,
            None,
            parent,
        ))
        .expect("virtual mount registration");
    id
}

pub fn bind_home_if_unmounted() {
    if find_mount("/home").is_some_and(|id| get_mount_info(id).is_some_and(|m| m.0 == "/home")) {
        return;
    }
    // Resolve and retain the root filesystem's directory before taking the
    // table lock. No backend operation is called while that lock is held.
    let Some((native, backend)) = ({
        let table = MOUNT_TABLE.lock();
        table
            .find_mount(b"/")
            .and_then(|id| table.get(id))
            .and_then(|m| match &m.backend {
                Backend::Fs(fs) => Some((m.id, fs.clone())),
                _ => None,
            })
    }) else {
        return;
    };
    let Ok(root) = backend.root() else { return };
    let Ok(home) = backend.lookup(root, b"home") else {
        return;
    };
    let Ok(pin) = backend.open(home) else { return };
    let mut table = MOUNT_TABLE.lock();
    if table.iter().any(|m| m.path == b"/home") {
        return;
    }
    let id = table.allocate_id().expect("mount id exhausted");
    let mut entry = Entry::new(
        id,
        b"/home".to_vec(),
        "ext2",
        HOME_FLAGS,
        Backend::Fs(backend),
        home,
        Some((native, home)),
        Some((native, root)),
    );
    entry.custody = Some(pin);
    table.insert(entry).expect("home bind registration");
    table
        .set_bind(id, (native, home))
        .expect("home bind target");
}

pub fn flags_at(path: &[u8]) -> MountFlags {
    MOUNT_TABLE.lock().flags_at(path)
}
pub fn find_mount(path: &str) -> Option<usize> {
    MOUNT_TABLE.lock().find_mount(path.as_bytes())
}
pub fn get_mount_info(id: usize) -> Option<(String, &'static str)> {
    MOUNT_TABLE
        .lock()
        .get(id)
        .map(|m| (String::from_utf8_lossy(&m.path).into_owned(), m.kind))
}
pub fn list_mounts() -> Vec<(usize, String, &'static str, MountFlags)> {
    MOUNT_TABLE
        .lock()
        .iter()
        .map(|m| {
            (
                m.id,
                String::from_utf8_lossy(&m.path).into_owned(),
                m.kind,
                m.flags,
            )
        })
        .collect()
}
pub fn pin(id: usize) -> Result<Arc<()>, Errno> {
    MOUNT_TABLE.lock().pin(id)
}
pub fn unmount(id: usize, force: bool) -> Result<(), Errno> {
    let removed = MOUNT_TABLE.lock().remove(id, force)?;
    drop(removed);
    Ok(())
}
pub fn covering(key: InodeKey) -> Option<u64> {
    let table = MOUNT_TABLE.lock();
    let (id, _) = table.enter(key)?;
    match table.get(id)?.backend {
        Backend::User(id) => Some(id),
        _ => None,
    }
}
pub fn covered_directory(id: usize) -> Option<InodeKey> {
    MOUNT_TABLE.lock().get(id)?.cover
}
pub fn covered_parent(id: usize) -> Option<InodeKey> {
    MOUNT_TABLE.lock().get(id)?.parent
}
pub fn register_user(mut entry: MountPoint) -> Result<(), Errno> {
    // The bridge id is the table id, assigned before publication.
    entry.backend = Backend::User(entry.id as u64);
    register(entry)
}

/// Preserve the existing proc text, including byte escaping for daemon mounts.
pub fn mounts_text() -> Vec<u8> {
    use alloc::format;
    let table = MOUNT_TABLE.lock();
    let mut out = Vec::new();
    for m in table
        .iter()
        .filter(|m| !matches!(m.backend, Backend::User(_)))
    {
        out.extend_from_slice(
            format!(
                "none {} {} {} 0 0\n",
                String::from_utf8_lossy(&m.path),
                m.kind,
                m.flags.options()
            )
            .as_bytes(),
        );
    }
    for m in table
        .iter()
        .filter(|m| matches!(m.backend, Backend::User(_)))
    {
        out.extend_from_slice(&crate::fs::ufs::mounts_line::format_mount_line(
            &m.source, &m.path,
        ));
    }
    out
}
pub fn remount_read_only(id: usize) -> Result<(), &'static str> {
    let mut table = MOUNT_TABLE.lock();
    table
        .get_mut(id)
        .ok_or("Mount disappeared")?
        .flags
        .read_only = true;
    Ok(())
}

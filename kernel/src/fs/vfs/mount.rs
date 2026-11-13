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
    let mut table = MOUNT_TABLE.lock();
    table.insert(entry)?;
    attach_virtual_parents(&mut table);
    Ok(())
}

/// Virtual filesystems retain their boot registration order and ids.
pub fn mount(path: &str, kind: &'static str) -> usize {
    let mut table = MOUNT_TABLE.lock();
    let id = table.allocate_id().expect("mount id exhausted");
    let parent = None;
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
    attach_virtual_parents(&mut table);
    id
}

// x86 registers virtual filesystems before storage; ARM registers them after it.
// Fill their named covers from registered parent roots in either boot order.
fn attach_virtual_parents(table: &mut MountTable<Backend>) {
    let parents: Vec<_> = table.iter().filter(|m| matches!(m.backend, Backend::Virtual))
        .filter_map(|m| {
            let slash = m.path.iter().rposition(|b| *b == b'/')?;
            let parent_path = if slash == 0 { b"/".as_slice() } else { &m.path[..slash] };
            let parent = table.iter().find(|p| p.path == parent_path)?;
            Some((m.id, (parent.id, parent.root)))
        }).collect();
    for (id, parent) in parents {
        table.get_mut(id).expect("registered virtual mount").parent = Some(parent);
    }
}

/// The single legacy accessor; no filesystem lock is taken under the table lock.
pub fn ext2_adapter(id: usize) -> Result<Arc<crate::fs::ext2::adapter::Ext2Adapter>, Errno> {
    let backend = {
        let table = MOUNT_TABLE.lock();
        match &table.get(id).ok_or(Errno(crate::syscall::errno::ENOENT))?.backend {
            Backend::Fs(fs) => fs.clone(),
            _ => return Err(Errno(crate::syscall::errno::EOPNOTSUPP)),
        }
    };
    let backend: Arc<dyn core::any::Any + Send + Sync> = backend;
    backend.downcast().map_err(|_| Errno(crate::syscall::errno::EOPNOTSUPP))
}

pub fn root_inode(id: usize) -> Option<u64> {
    Some(MOUNT_TABLE.lock().get(id)?.root)
}

#[derive(Clone, Copy)]
pub enum Entered {
    Fs(usize, u64),
    User(u64, u64),
    Virtual,
}
fn entered(entry: &MountPoint) -> Entered {
    match entry.backend {
        Backend::Fs(_) => Entered::Fs(entry.id, entry.root),
        Backend::User(id) => Entered::User(id, entry.root),
        Backend::Virtual => Entered::Virtual,
    }
}

/// Inode covers retain their identity when an ancestor is renamed.
pub fn enter(key: InodeKey) -> Option<Entered> {
    let table = MOUNT_TABLE.lock();
    let (id, _) = table.enter(key)?;
    Some(entered(table.get(id)?))
}

/// Virtual covers have no storage inode. Their parent identity and registered
/// final component select them without locking an unrelated storage backend.
pub fn virtual_cover(parent: InodeKey, name: &[u8]) -> Option<Entered> {
    let table = MOUNT_TABLE.lock();
    let found = table.iter().find(|m| matches!(m.backend, Backend::Virtual)
        && m.parent == Some(parent)
        && m.path.rsplit(|b| *b == b'/').next() == Some(name)).map(entered);
    found
}

pub fn is_mount_root(id: usize, ino: u64) -> bool {
    let table = MOUNT_TABLE.lock();
    table.get(id).is_some_and(|m| m.root == ino) || table.enter((id, ino)).is_some()
}

pub fn path_is_mount(path: &[u8]) -> bool {
    MOUNT_TABLE.lock().iter().any(|m| m.path == path)
}

/// Bind entries share the adapter's storage identity.
pub fn storage_mounts() -> Vec<usize> {
    MOUNT_TABLE.lock().iter().filter(|m| matches!(m.backend, Backend::Fs(_)) && m.bind.is_none())
        .map(|m| m.id).collect()
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

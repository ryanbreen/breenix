//! Hash-bucketed Unix names. Bind allocates before filesystem mutation and
//! retains the inode created under the mount write guard; names are raw bytes.
use super::addr::UnixName;
use super::object::Socket;
use crate::fs::{
    ext2::live_inode::FileHandle,
    ext2::FileType,
    namei::{Last, Target},
    permissions::Credentials,
};
use crate::irq_safe_mutex::IrqSafeMutex;
use crate::syscall::errno::*;
use alloc::boxed::Box;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;

const BUCKETS: usize = 256;
const NAME_BYTES: usize = 108;
enum Key {
    Abstract { bytes: [u8; NAME_BYTES], len: usize },
    Path(FileHandle),
}
impl Key {
    fn matches(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Abstract { bytes: a, len: al }, Self::Abstract { bytes: b, len: bl }) => {
                al == bl && a[..*al] == b[..*bl]
            }
            (Self::Path(a), Self::Path(b)) => {
                a.object.mount == b.object.mount && a.object.key == b.object.key
            }
            _ => false,
        }
    }
    fn bucket(&self) -> usize {
        let mut hash = 0xcbf29ce484222325u64;
        let mut feed = |b: u8| {
            hash = (hash ^ b as u64).wrapping_mul(0x100000001b3);
        };
        match self {
            Self::Abstract { bytes, len } => {
                feed(0);
                for b in &bytes[..*len] {
                    feed(*b);
                }
            }
            Self::Path(handle) => {
                feed(1);
                for b in handle
                    .object
                    .mount
                    .mount_id
                    .to_ne_bytes()
                    .into_iter()
                    .chain(handle.object.key.inode.to_ne_bytes())
                {
                    feed(b);
                }
            }
        }
        hash as usize % BUCKETS
    }
}
const PER_UID_BINDINGS: usize = 4096;
static OWNERS: IrqSafeMutex<Vec<(u32, usize)>> = IrqSafeMutex::new(Vec::new());
struct OwnerLease(u32);
impl OwnerLease {
    fn reserve(uid: u32) -> Result<Self, i32> {
        let mut owners = OWNERS.lock();
        if let Some((_, count)) = owners.iter_mut().find(|(owner, _)| *owner == uid) {
            if *count == PER_UID_BINDINGS {
                return Err(ENOBUFS);
            }
            *count += 1;
        } else {
            owners.try_reserve(1).map_err(|_| ENOMEM)?;
            owners.push((uid, 1));
        }
        Ok(Self(uid))
    }
}
impl Drop for OwnerLease {
    fn drop(&mut self) {
        let mut owners = OWNERS.lock();
        if let Some(index) = owners.iter().position(|(owner, _)| *owner == self.0) {
            owners[index].1 -= 1;
            if owners[index].1 == 0 {
                owners.swap_remove(index);
            }
        }
    }
}
struct Binding {
    _owner: OwnerLease,
    key: Key,
    socket: Weak<Socket>,
    next: Option<Box<Binding>>,
}
pub struct UnixSocketRegistry {
    buckets: [IrqSafeMutex<Option<Box<Binding>>>; BUCKETS],
}
impl UnixSocketRegistry {
    pub const fn new() -> Self {
        Self {
            buckets: [const { IrqSafeMutex::new(None) }; BUCKETS],
        }
    }
}
pub static UNIX_SOCKET_REGISTRY: UnixSocketRegistry = UnixSocketRegistry::new();

fn abstract_key(bytes: &[u8]) -> Result<Key, i32> {
    if bytes.len() > NAME_BYTES - 1 {
        return Err(EINVAL);
    }
    let mut key = [0; NAME_BYTES];
    key[..bytes.len()].copy_from_slice(bytes);
    Ok(Key::Abstract {
        bytes: key,
        len: bytes.len(),
    })
}
fn path_key(bytes: &[u8]) -> Result<Key, i32> {
    let cred = Credentials::current(false);
    let resolved =
        crate::fs::namei::resolve_socket_path(bytes, false, &cred).map_err(|e| e as i32)?;
    if matches!(resolved.target, Target::Absent { .. }) || resolved.virtual_absent {
        return Err(ENOENT);
    }
    let Target::Inode {
        mount,
        file_type: FileType::Socket,
        ..
    } = resolved.target
    else {
        return Err(ECONNREFUSED);
    };
    let handle = resolved.handle().ok_or(EIO)?.clone();
    let guard = mount.read().map_err(|e| e as i32)?;
    let fs = guard.as_ref().ok_or(EIO)?;
    let inode = fs
        .read_inode(handle.verify(fs).map_err(|_| ENOENT)?)
        .map_err(|e| crate::fs::namei::fs_errno(e) as i32)?;
    if !cred.permits(&inode, 2) {
        return Err(EACCES);
    }
    Ok(Key::Path(handle))
}
fn create_path(bytes: &[u8]) -> Result<Key, i32> {
    if bytes.is_empty() || bytes.len() > NAME_BYTES {
        return Err(EINVAL);
    }
    let cred = Credentials::current(false);
    let resolved =
        crate::fs::namei::resolve_socket_path(bytes, true, &cred).map_err(|e| e as i32)?;
    if resolved.last != Last::Name || matches!(resolved.target, Target::Inode { .. }) {
        return Err(EADDRINUSE);
    }
    let Target::Absent { mount, .. } = resolved.target else {
        return Err(EPERM);
    };
    if resolved.trailing_slash {
        return Err(ENOENT);
    }
    let parent = resolved.entry_dir().ok_or(EIO)?;
    let name = resolved.path.rsplit(|b| *b == b'/').next().ok_or(EINVAL)?;
    let mut guard = mount.write().map_err(|e| e as i32)?;
    let fs = guard.as_mut().ok_or(EIO)?;
    let parent = parent.verify(fs).map_err(|_| ENOENT)?;
    let inode = fs
        .read_inode(parent)
        .map_err(|e| crate::fs::namei::fs_errno(e) as i32)?;
    let gid = crate::syscall::fifo_policy::check_parent_access(&inode, &cred, None)
        .map_err(|e| e as i32)?;
    let mode = 0o140000 | crate::syscall::fifo_policy::creation_mode(0o777, &cred, gid, false);
    fs.create_node_prepared(parent, name, mode, cred.euid, gid, 0, |fs, ino| {
        fs.pin_loaded_inode(ino, 0)
    })
    .map(Key::Path)
    .map_err(|error| {
        if error.contains("already exists") {
            EADDRINUSE
        } else if error.contains("not found") {
            ENOENT
        } else if error.contains("No free") || error.contains("No space") {
            ENOSPC
        } else {
            crate::fs::namei::fs_errno(error) as i32
        }
    })
}
pub fn bind_name(name: &UnixName, socket: Weak<Socket>) -> Result<(), i32> {
    // Allocate the complete binding before creating any on-disk node. Publication
    // cannot fail for a freshly created inode and does not allocate.
    let mut binding = Box::try_new(Binding {
        _owner: OwnerLease::reserve(Credentials::current(false).euid)?,
        key: abstract_key(&[])?,
        socket,
        next: None,
    })
    .map_err(|_| ENOMEM)?;
    binding.key = match name {
        UnixName::Abstract(bytes) => abstract_key(bytes)?,
        UnixName::Path(bytes) => create_path(bytes)?,
        UnixName::Unnamed => return Err(EINVAL),
    };
    let bucket = binding.key.bucket();
    let mut head = UNIX_SOCKET_REGISTRY.buckets[bucket].lock();
    let mut cursor = head.as_deref();
    while let Some(old) = cursor {
        if old.key.matches(&binding.key) && old.socket.strong_count() != 0 {
            return Err(EADDRINUSE);
        }
        cursor = old.next.as_deref();
    }
    binding.next = head.take();
    *head = Some(binding);
    Ok(())
}
pub fn lookup(name: &UnixName) -> Result<Arc<Socket>, i32> {
    let key = match name {
        UnixName::Abstract(bytes) => abstract_key(bytes)?,
        UnixName::Path(bytes) => path_key(bytes)?,
        UnixName::Unnamed => return Err(EINVAL),
    };
    let head = UNIX_SOCKET_REGISTRY.buckets[key.bucket()].lock();
    let mut cursor = head.as_deref();
    while let Some(binding) = cursor {
        if binding.key.matches(&key) {
            if let Some(socket) = binding.socket.upgrade() {
                return Ok(socket);
            }
        }
        cursor = binding.next.as_deref();
    }
    Err(ECONNREFUSED)
}
pub fn unbind(socket: &Socket) {
    unbind_id(socket as *const Socket as usize);
}
pub fn unbind_id(socket_id: usize) {
    for bucket in &UNIX_SOCKET_REGISTRY.buckets {
        let removed = {
            let mut head = bucket.lock();
            let mut cursor = &mut *head;
            loop {
                match cursor {
                    Some(binding) if binding.socket.as_ptr() as usize == socket_id => {
                        let mut removed = cursor.take().unwrap();
                        *cursor = removed.next.take();
                        break Some(removed);
                    }
                    Some(binding) => cursor = &mut binding.next,
                    None => break None,
                }
            }
        };
        drop(removed); // Inode pins are dropped with IRQs restored.
    }
}

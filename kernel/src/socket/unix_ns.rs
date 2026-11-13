//! Hash-bucketed Unix names. Bind allocates before filesystem mutation and
//! retains the socket's VNode; names are raw bytes.
use super::addr::UnixName;
use super::object::Socket;
use crate::fs::{
    attrs::InodeAttrs,
    namei::{Last, Target},
    permissions::Credentials,
    vfs::{mount, names, NodeKind, VNode},
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
    Path(Arc<VNode>),
}
impl Key {
    fn matches(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Abstract { bytes: a, len: al }, Self::Abstract { bytes: b, len: bl }) => {
                al == bl && a[..*al] == b[..*bl]
            }
            (Self::Path(a), Self::Path(b)) => {
                a.mount == b.mount && a.ino == b.ino && a.incarnation == b.incarnation
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
            Self::Path(node) => {
                feed(1);
                for b in node
                    .mount
                    .to_ne_bytes()
                    .into_iter()
                    .chain(node.ino.to_ne_bytes())
                    .chain(node.incarnation.to_ne_bytes())
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
    let cred = Credentials::current(false).map_err(|errno| errno as i32)?;
    let resolved =
        crate::fs::namei::resolve_socket_path(bytes, false, &cred).map_err(|e| e as i32)?;
    if matches!(resolved.target, Target::Absent { .. }) || resolved.virtual_absent {
        return Err(ENOENT);
    }
    let Target::Inode { mount, ino, .. } = resolved.target else {
        return Err(ECONNREFUSED);
    };
    let fs = mount::filesystem(mount.0).map_err(|e| e.0)?;
    let node = fs.open(ino).map_err(|e| e.0)?;
    let attrs = fs.getattr(node.ino).map_err(|e| e.0)?;
    if attrs.kind != NodeKind::Socket {
        return Err(ECONNREFUSED);
    }
    if !cred.may(attrs.attrs, 2) {
        return Err(EACCES);
    }
    Ok(Key::Path(node))
}
fn create_path(bytes: &[u8]) -> Result<Key, i32> {
    if bytes.is_empty() || bytes.len() > NAME_BYTES {
        return Err(EINVAL);
    }
    let cred = Credentials::current(false).map_err(|errno| errno as i32)?;
    let resolved =
        crate::fs::namei::resolve_socket_path(bytes, true, &cred).map_err(|e| e as i32)?;
    if resolved.last != Last::Name || matches!(resolved.target, Target::Inode { .. }) {
        return Err(EADDRINUSE);
    }
    let Target::Absent { .. } = resolved.target else {
        return Err(EPERM);
    };
    if resolved.trailing_slash {
        return Err(ENOENT);
    }
    let (fs, parent, _) = names::entry_dir(&resolved).map_err(|e| e.0)?;
    let mut key = None;
    fs.create_open(parent.ino, resolved.final_name(), NodeKind::Socket, &|parent| {
        let gid = crate::syscall::fifo_policy::check_parent_access(
            parent.attrs, parent.nlink, &cred, None,
        ).map_err(|error| crate::fs::errno::Errno(error as i32))?;
        let mode = crate::syscall::fifo_policy::creation_mode(0o777, &cred, gid, false);
        Ok(InodeAttrs::new(mode.into(), cred.euid, gid))
    }, &mut |node| {
        key = Some(Key::Path(node));
        Ok(())
    }).map_err(|error| if error.0 == EEXIST { EADDRINUSE } else { error.0 })?;
    Ok(key.expect("create_open did not prepare its socket"))
}
pub fn bind_name(name: &UnixName, socket: Weak<Socket>) -> Result<(), i32> {
    // Allocate the registry entry before creating the on-disk node. Publishing
    // the entry in its bucket does not allocate.
    let mut binding = Box::try_new(Binding {
        _owner: OwnerLease::reserve(Credentials::current(false).map_err(|errno| errno as i32)?.euid)?,
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

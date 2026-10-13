//! Unix names are reserved at bind, independently of listen. Path bindings
//! retain the resolved inode so unlink/recreate cannot redirect an old socket.
//! Namespace locks cover only the fixed table, never filesystem or PM work.

use super::addr::UnixName;
use super::object::Socket;
use crate::fs::{
    ext2::live_inode::FileHandle, ext2::FileType, namei::Target, permissions::Credentials,
};
use crate::irq_safe_mutex::IrqSafeMutex;
use crate::syscall::{at_path::AT_FDCWD, errno::*, SyscallResult};
use alloc::sync::{Arc, Weak};

const MAX_BINDINGS: usize = 4096;
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
}

enum Slot {
    Reserved,
    Bound { key: Key, socket: Weak<Socket> },
}

pub struct UnixSocketRegistry {
    slots: IrqSafeMutex<[Option<Slot>; MAX_BINDINGS]>,
}

impl UnixSocketRegistry {
    pub const fn new() -> Self {
        Self {
            slots: IrqSafeMutex::new([const { None }; MAX_BINDINGS]),
        }
    }
}

pub static UNIX_SOCKET_REGISTRY: UnixSocketRegistry = UnixSocketRegistry::new();

struct Reservation(usize);
impl Drop for Reservation {
    fn drop(&mut self) {
        let mut slots = UNIX_SOCKET_REGISTRY.slots.lock();
        if matches!(slots[self.0], Some(Slot::Reserved)) {
            slots[self.0] = None;
        }
    }
}

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

fn path_key(bytes: &[u8], writing: bool) -> Result<Key, i32> {
    let path = core::str::from_utf8(bytes).map_err(|_| EINVAL)?;
    let cred = Credentials::current(false);
    let resolved =
        crate::syscall::metadata::resolve_at(AT_FDCWD, path, true, &cred).map_err(|e| e as i32)?;
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
    if writing {
        let guard = mount.read().map_err(|e| e as i32)?;
        let fs = guard.as_ref().ok_or(EIO)?;
        let inode = fs
            .read_inode(handle.verify(fs).map_err(|_| ENOENT)?)
            .map_err(|e| crate::fs::namei::fs_errno(e) as i32)?;
        if !cred.permits(&inode, 2) {
            return Err(EACCES);
        }
    }
    Ok(Key::Path(handle))
}

pub fn bind_name(name: &UnixName, socket: Weak<Socket>) -> Result<(), i32> {
    let key = match name {
        UnixName::Abstract(bytes) => Some(abstract_key(bytes)?),
        UnixName::Path(bytes) => {
            if bytes.is_empty() || bytes.len() > NAME_BYTES {
                return Err(EINVAL);
            }
            None
        }
        UnixName::Unnamed => return Err(EINVAL),
    };
    // Reserve space before any filesystem mutation. Published entries never
    // allocate, and concurrent bind calls cannot steal this slot.
    let reservation = {
        let mut slots = UNIX_SOCKET_REGISTRY.slots.lock();
        if let Some(key) = &key {
            if slots.iter().flatten().any(|s| {
                matches!(s,
                Slot::Bound { key: old, socket } if old.matches(key) && socket.strong_count() != 0)
            }) {
                return Err(EADDRINUSE);
            }
        }
        let index = slots.iter().position(Option::is_none).ok_or(ENOBUFS)?;
        slots[index] = Some(Slot::Reserved);
        Reservation(index)
    };
    let key = match (key, name) {
        (Some(key), _) => key,
        (None, UnixName::Path(bytes)) => {
            let path = core::str::from_utf8(bytes).map_err(|_| EINVAL)?;
            let mode = crate::syscall::fs::S_IFSOCK | (0o777 & !Credentials::current(false).umask);
            match crate::syscall::fifo::mknodat_path(AT_FDCWD, path, mode, 0) {
                SyscallResult::Ok(_) => {}
                SyscallResult::Err(e) => {
                    return Err(if e as i32 == EEXIST {
                        EADDRINUSE
                    } else {
                        e as i32
                    })
                }
            }
            path_key(bytes, false)?
        }
        _ => return Err(EINVAL),
    };
    let mut slots = UNIX_SOCKET_REGISTRY.slots.lock();
    // Two abstract reservations can be made before either publishes.
    if slots.iter().flatten().any(|s| {
        matches!(s,
        Slot::Bound { key: old, socket } if old.matches(&key) && socket.strong_count() != 0)
    }) {
        drop(slots);
        return Err(EADDRINUSE);
    }
    slots[reservation.0] = Some(Slot::Bound { key, socket });
    drop(slots);
    Ok(())
}

pub fn lookup(name: &UnixName) -> Result<Arc<Socket>, i32> {
    let key = match name {
        UnixName::Abstract(bytes) => abstract_key(bytes)?,
        UnixName::Path(bytes) => path_key(bytes, true)?,
        UnixName::Unnamed => return Err(EINVAL),
    };
    let slots = UNIX_SOCKET_REGISTRY.slots.lock();
    slots
        .iter()
        .flatten()
        .find_map(|s| match s {
            Slot::Bound { key: old, socket } if old.matches(&key) => socket.upgrade(),
            _ => None,
        })
        .ok_or(ECONNREFUSED)
}

/// Remove by socket identity, not by a pathname that may have been unlinked.
pub fn unbind(socket: &Socket) {
    unbind_id(socket as *const Socket as usize);
}

/// A deferred release moves protocol state to a carcass. The registry Weak
/// retains the original allocation's identity until this unbind.
pub fn unbind_id(socket_id: usize) {
    // Move inode handles out before dropping them: their finalizer does not
    // run with the namespace lock held.
    loop {
        let removed = {
            let mut slots = UNIX_SOCKET_REGISTRY.slots.lock();
            let index = slots.iter().position(|s| {
                matches!(s,
                Some(Slot::Bound { socket: weak, .. }) if weak.as_ptr() as usize == socket_id)
            });
            index.and_then(|i| slots[i].take())
        };
        if removed.is_none() {
            break;
        }
        drop(removed);
    }
}

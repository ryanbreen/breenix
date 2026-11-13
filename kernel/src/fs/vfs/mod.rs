//! Filesystem operations at the VFS boundary (cowfs design section 1.1).
//! Paths, mount traversal and credential decisions belong to the VFS caller.
//! Backends take their own locks; inode numbers crossing this boundary are u64.

use crate::fs::{attrs::InodeAttrs, errno::Errno};
use crate::syscall::errno::EOPNOTSUPP;
use alloc::{sync::Arc, vec::Vec};
use core::{any::Any, sync::atomic::AtomicU64};
use spin::Mutex;

pub use crate::fs::attrs::MountFlags;
pub mod file;
pub mod mount_table;
#[cfg(not(test))]
pub mod mount;
#[cfg(not(test))]
pub use mount::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeKind {
    Regular,
    Directory,
    Symlink,
    CharDevice,
    BlockDevice,
    Fifo,
    Socket,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Timestamp {
    pub sec: i64,
    pub nsec: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NodeTimes {
    pub atime: Timestamp,
    pub mtime: Timestamp,
    pub ctime: Timestamp,
    pub btime: Option<Timestamp>,
}

/// An open inode. The opaque backend pin retains its incarnation; dropping it
/// must do no I/O. MapState stays in ext2 until the mmap migration in 24.6.
pub struct VNode {
    pub mount: usize,
    pub ino: u64,
    pub incarnation: u64,
    pub size: AtomicU64,
    pub times: Mutex<NodeTimes>,
    backend_pin: Arc<dyn Any + Send + Sync>,
}
impl VNode {
    pub fn new(
        mount: usize,
        ino: u64,
        incarnation: u64,
        size: u64,
        times: NodeTimes,
        pin: Arc<dyn Any + Send + Sync>,
    ) -> Self {
        Self {
            mount,
            ino,
            incarnation,
            size: AtomicU64::new(size),
            times: Mutex::new(times),
            backend_pin: pin,
        }
    }

    /// Borrow the backend's retained inode handle without changing its lifetime.
    pub fn pin<T: Any>(&self) -> Option<&T> {
        self.backend_pin.downcast_ref()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirEnt {
    pub ino: u64,
    pub kind: NodeKind,
    pub name: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NodeAttrs {
    pub ino: u64,
    pub kind: NodeKind,
    pub attrs: InodeAttrs,
    pub nlink: u64,
    pub size: u64,
    /// Allocated sectors of 512 bytes (st_blocks).
    pub blocks: u64,
    pub rdev: u64,
    pub times: NodeTimes,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SetAttr {
    pub mode: Option<u16>,
    pub uid: Option<u32>,
    pub gid: Option<u32>,
    pub atime: Option<Timestamp>,
    pub mtime: Option<Timestamp>,
}

/// Permission bits and ownership have already been decided by the VFS.
#[derive(Clone, Copy, Debug)]
pub enum NewNode<'a> {
    Regular { attrs: InodeAttrs },
    Directory { attrs: InodeAttrs },
    Symlink { attrs: InodeAttrs, target: &'a [u8] },
    CharDevice { attrs: InodeAttrs, rdev: u64 },
    BlockDevice { attrs: InodeAttrs, rdev: u64 },
    Fifo { attrs: InodeAttrs },
    Socket { attrs: InodeAttrs },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Remove {
    Dir,
    NonDir,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenameMode {
    Replace,
    NoReplace,
    Exchange,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteAt {
    Offset(u64),
    Append,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum XattrSet {
    Upsert,
    Create,
    Replace,
}

pub enum Xattr<'a> {
    Get {
        name: &'a [u8],
        value: &'a mut [u8],
    },
    List {
        names: &'a mut [u8],
    },
    Set {
        name: &'a [u8],
        value: &'a [u8],
        mode: XattrSet,
    },
    Remove {
        name: &'a [u8],
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StatFs {
    pub magic: u64,
    pub fsid: u64,
    pub block_size: u64,
    pub blocks: u64,
    pub free_blocks: u64,
    pub available_blocks: u64,
    pub inodes: u64,
    pub free_inodes: u64,
    pub max_name_len: u64,
    pub flags: MountFlags,
}

/// Object-safe filesystem interface. Unsupported groups refuse operations
/// explicitly while an adapter's staged migration is in progress.
pub trait FileSystem: core::any::Any + Send + Sync {
    // Mount
    fn kind(&self) -> Result<&'static str, Errno>;
    fn root(&self) -> Result<u64, Errno>;
    fn statfs(&self) -> Result<StatFs, Errno>;
    fn sync(&self) -> Result<(), Errno>;
    fn read_only(&self) -> Result<bool, Errno>;
    fn max_file_size(&self) -> Result<u64, Errno>;
    // Inode
    fn getattr(&self, ino: u64) -> Result<NodeAttrs, Errno>;
    fn setattr(&self, ino: u64, attrs: SetAttr) -> Result<(), Errno>;
    fn open(&self, ino: u64) -> Result<Arc<VNode>, Errno>;
    // Names
    fn lookup(&self, dir: u64, name: &[u8]) -> Result<u64, Errno>;
    /// Parent inode exposed by `..`, including the backend's selected namespace.
    fn parent(&self, dir: u64) -> Result<u64, Errno>;
    /// The cookie returned is after the last accepted entry. A sink returning
    /// false leaves that entry for the next call. Dot entries belong to VFS.
    fn readdir(
        &self,
        dir: u64,
        cookie: u64,
        sink: &mut dyn FnMut(DirEnt) -> Result<bool, Errno>,
    ) -> Result<u64, Errno>;
    fn create(&self, dir: u64, name: &[u8], node: NewNode<'_>) -> Result<u64, Errno>;
    fn link(&self, ino: u64, dir: u64, name: &[u8]) -> Result<(), Errno>;
    fn remove(&self, dir: u64, name: &[u8], which: Remove) -> Result<(), Errno>;
    fn rename(
        &self,
        from_dir: u64,
        from: &[u8],
        to_dir: u64,
        to: &[u8],
        mode: RenameMode,
    ) -> Result<(), Errno>;
    fn readlink(&self, ino: u64) -> Result<Vec<u8>, Errno>;
    // Data
    fn read(&self, _node: &VNode, _off: u64, _buf: &mut [u8]) -> Result<usize, Errno> {
        Err(Errno(EOPNOTSUPP))
    }
    fn write(&self, _node: &VNode, _at: WriteAt, _buf: &[u8]) -> Result<usize, Errno> {
        Err(Errno(EOPNOTSUPP))
    }
    fn resize(&self, _node: &VNode, _size: u64) -> Result<(), Errno> {
        Err(Errno(EOPNOTSUPP))
    }
    fn fsync(&self, _node: &VNode, _data_only: bool) -> Result<(), Errno> {
        Err(Errno(EOPNOTSUPP))
    }
    fn writeback_pages(&self, _node: &VNode, _page: u64, _bytes: &[u8]) -> Result<(), Errno> {
        Err(Errno(EOPNOTSUPP))
    }
    // Xattrs (no syscall consumer yet).
    fn xattr(&self, _ino: u64, _op: Xattr<'_>) -> Result<usize, Errno> {
        Err(Errno(EOPNOTSUPP))
    }
}

#![no_std]
#![cfg_attr(test, feature(allocator_api))]

extern crate alloc;
#[cfg(test)]
extern crate std;

#[path = "../../../kernel/src/fs/breenfs/abi.rs"]
pub mod abi;
#[path = "../../../kernel/src/fs/breenfs/btree.rs"]
pub mod btree;
#[path = "../../../kernel/src/fs/breenfs/commit.rs"]
pub mod commit;
#[path = "../../../kernel/src/fs/breenfs/commit_policy.rs"]
pub mod commit_policy;
#[path = "../../../kernel/src/fs/breenfs/crc32c.rs"]
pub mod crc32c;
#[path = "../../../kernel/src/fs/breenfs/format.rs"]
pub mod format;
#[path = "../../../kernel/src/fs/breenfs/node_cache.rs"]
pub mod node_cache;
#[path = "../../../kernel/src/fs/breenfs/recover.rs"]
pub mod recover;
#[path = "../../../kernel/src/fs/breenfs/reserve.rs"]
pub mod reserve;
#[path = "../../../kernel/src/fs/breenfs/scrub.rs"]
pub mod scrub;
#[path = "../../../kernel/src/fs/breenfs/siphash.rs"]
pub mod siphash;
#[path = "../../../kernel/src/fs/breenfs/space.rs"]
pub mod space;
#[path = "../../../kernel/src/fs/breenfs/txn.rs"]
pub mod txn;

pub mod corrupt;
pub mod crash;
pub mod minimize;
pub mod model;
#[path = "../../../kernel/src/fs/breenfs/ops.rs"]
pub mod ops;
pub mod recording;
pub mod stream;

/// Host fixture for the synchronous kernel block-device surface.
pub mod block {
    pub use crate::buffer_cache as cache;
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum BlockError {
        IoError,
        OutOfMemory,
        OutOfBounds,
    }

    pub trait BlockDevice: Send + Sync {
        fn identity(&self) -> usize {
            self as *const Self as *const () as usize
        }
        fn read_block(&self, block: u64, buf: &mut [u8]) -> Result<(), BlockError>;
        fn read_blocks(&self, start: u64, count: usize, buf: &mut [u8]) -> Result<(), BlockError> {
            let size = self.block_size();
            if buf.len() < count.saturating_mul(size) {
                return Err(BlockError::IoError);
            }
            for i in 0..count {
                self.read_block(start + i as u64, &mut buf[i * size..(i + 1) * size])?;
            }
            Ok(())
        }
        fn write_block(&self, block: u64, buf: &[u8]) -> Result<(), BlockError>;
        /// Mirrors kernel/src/block/mod.rs: one write_block per block by default.
        fn write_blocks(&self, start: u64, count: usize, buf: &[u8]) -> Result<(), BlockError> {
            let size = self.block_size();
            if buf.len() < count.checked_mul(size).ok_or(BlockError::IoError)? {
                return Err(BlockError::IoError);
            }
            for i in 0..count {
                self.write_block(start + i as u64, &buf[i * size..(i + 1) * size])?;
            }
            Ok(())
        }
        fn block_size(&self) -> usize;
        fn num_blocks(&self) -> u64;
        fn flush(&self) -> Result<(), BlockError>;
    }
}

pub fn arch_without_interrupts<R>(f: impl FnOnce() -> R) -> R {
    f()
}

#[path = "../../../kernel/src/block/cache.rs"]
pub mod buffer_cache;

use alloc::{collections::BTreeMap, vec::Vec};

pub type Ino = u64;
pub const ROOT: Ino = 2;
pub const MAX_FILE_SIZE: u64 = (1u64 << 63) - 4096;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Errno {
    EPERM,
    ENOENT,
    ENODATA,
    EEXIST,
    ENOTDIR,
    EISDIR,
    EINVAL,
    EFBIG,
    EMLINK,
    ENAMETOOLONG,
    ENOTEMPTY,
    EIO,
    EROFS,
    EBUSY,
    ENOSPC,
}

/// Adapter absence is separate from a filesystem's POSIX error response.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Errno(Errno),
    Unimplemented,
}
pub type Result<T> = core::result::Result<T, Error>;
impl From<Errno> for Error {
    fn from(value: Errno) -> Self {
        Self::Errno(value)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Time {
    pub seconds: i64,
    pub nanos: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Metadata {
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub nlink: u32,
    pub atime: Time,
    pub mtime: Time,
    pub ctime: Time,
    pub btime: Time,
}

/// Sparse representation omits bytes whose value is 0, including holes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Contents {
    File {
        size: u64,
        bytes: BTreeMap<u64, u8>,
    },
    Directory {
        parent: Ino,
        entries: BTreeMap<Vec<u8>, Ino>,
    },
    Symlink(Vec<u8>),
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Inode {
    pub metadata: Metadata,
    pub contents: Contents,
    pub xattrs: BTreeMap<Vec<u8>, Vec<u8>>,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct State {
    pub inodes: BTreeMap<Ino, Inode>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SetAttr {
    pub mode: Option<u32>,
    pub uid: Option<u32>,
    pub gid: Option<u32>,
    pub atime: Option<Time>,
    pub mtime: Option<Time>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Action {
    Create {
        dir: Ino,
        name: Vec<u8>,
        mode: u32,
    },
    Write {
        ino: Ino,
        offset: u64,
        bytes: Vec<u8>,
    },
    Truncate {
        ino: Ino,
        size: u64,
    },
    Link {
        ino: Ino,
        dir: Ino,
        name: Vec<u8>,
    },
    Unlink {
        dir: Ino,
        name: Vec<u8>,
    },
    Rename {
        from_dir: Ino,
        from: Vec<u8>,
        to_dir: Ino,
        to: Vec<u8>,
    },
    Mkdir {
        dir: Ino,
        name: Vec<u8>,
        mode: u32,
    },
    Rmdir {
        dir: Ino,
        name: Vec<u8>,
    },
    Symlink {
        dir: Ino,
        name: Vec<u8>,
        target: Vec<u8>,
    },
    Setattr {
        ino: Ino,
        attrs: SetAttr,
    },
    Setxattr {
        ino: Ino,
        name: Vec<u8>,
        value: Vec<u8>,
    },
    Removexattr {
        ino: Ino,
        name: Vec<u8>,
    },
    Fsync {
        ino: Ino,
    },
    Remount,
    Snapshot {
        name: Vec<u8>,
        read_only: bool,
    },
    Clone {
        name: Vec<u8>,
    },
    DeleteSnapshot {
        name: Vec<u8>,
    },
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Operation {
    pub at: Time,
    pub action: Action,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Reply {
    Done,
    Inode(Ino),
    Written(usize),
}

/// Logical root contents and the snapshot's write policy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RootState {
    pub read_only: bool,
    pub state: State,
}
pub type Roots = BTreeMap<Vec<u8>, RootState>;

/// Paths, credentials, umask and symlink traversal belong to the VFS (design 1.1).
/// Adapters use the supplied logical clock and expose logical bytes and metadata.
pub trait Fs {
    fn execute(&mut self, op: &Operation) -> Result<Reply>;
    fn state(&self) -> Result<State>;
    fn roots(&self) -> Result<Roots> {
        Ok(BTreeMap::from([(
            b"default".to_vec(),
            RootState {
                read_only: false,
                state: self.state()?,
            },
        )]))
    }
    fn select_root(&mut self, _name: &[u8]) -> Result<()> {
        Err(Error::Unimplemented)
    }
}

pub mod writer;
pub use writer::WriterAdapter;
#[cfg(test)]
mod test_image;

#[cfg(test)]
mod vfs_support;
#[cfg(test)]
pub use vfs_support::{
    attrs, credentials, errno, fs, memory, permissions, process, syscall, syscall_errno, task,
    test_control, time, vfs,
};
#[cfg(test)]
#[path = "../../../kernel/src/fs/breenfs/commit_signal.rs"]
mod commit_signal;
#[cfg(test)]
#[path = "../../../kernel/src/fs/breenfs/dirty.rs"]
pub mod dirty;
#[cfg(test)]
#[path = "../../../kernel/src/fs/breenfs/mount_lock.rs"]
mod mount_lock;
#[cfg(test)]
#[path = "../../../kernel/src/fs/breenfs/vfs.rs"]
pub mod vfs_adapter;
#[cfg(test)]
#[path = "../../../kernel/src/fs/breenfs/mount_args.rs"]
pub mod mount_args;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Difference {
    pub root: Vec<u8>,
    pub expected_read_only: Option<bool>,
    pub actual_read_only: Option<bool>,
    pub ino: Ino,
    pub expected: Option<Inode>,
    pub actual: Option<Inode>,
}

/// Diff root presence/flags and each live root's names, bytes and metadata.
pub fn compare(model: &impl Fs, fs: &impl Fs) -> Result<Vec<Difference>> {
    let expected = model.roots()?;
    let actual = fs.roots()?;
    let mut names: Vec<_> = expected.keys().chain(actual.keys()).cloned().collect();
    names.sort_unstable();
    names.dedup();
    let mut differences = Vec::new();
    for name in names {
        let a = expected.get(&name);
        let b = actual.get(&name);
        let expected_read_only = a.map(|root| root.read_only);
        let actual_read_only = b.map(|root| root.read_only);
        let empty = State::default();
        let a = a.map(|root| &root.state).unwrap_or(&empty);
        let b = b.map(|root| &root.state).unwrap_or(&empty);
        let mut ids: Vec<_> = a.inodes.keys().chain(b.inodes.keys()).copied().collect();
        ids.sort_unstable();
        ids.dedup();
        for ino in ids {
            let a = a.inodes.get(&ino);
            let b = b.inodes.get(&ino);
            if a != b || (ino == ROOT && expected_read_only != actual_read_only) {
                differences.push(Difference {
                    root: name.clone(),
                    expected_read_only,
                    actual_read_only,
                    ino,
                    expected: a.cloned(),
                    actual: b.cloned(),
                });
            }
        }
    }
    Ok(differences)
}

#[path = "../../../kernel/src/fs/breenfs/preempt.rs"]
pub mod preempt;

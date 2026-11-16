#![no_std]
#![cfg_attr(test, feature(allocator_api))]

extern crate alloc;
#[cfg(test)]
extern crate std;

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

/// Paths, credentials, umask and symlink traversal belong to the VFS (design 1.1).
/// Adapters use the supplied logical clock and expose logical bytes and metadata.
pub trait Fs {
    fn execute(&mut self, op: &Operation) -> Result<Reply>;
    fn state(&self) -> Result<State>;
}

pub mod writer;
pub use writer::WriterAdapter;
#[cfg(test)]
mod test_image;

#[cfg(test)]
mod vfs_support;
#[cfg(test)]
pub use vfs_support::{attrs, credentials, errno, fs, memory, permissions, process, syscall,
    syscall_errno, task, test_control, time, vfs};
#[cfg(test)]
#[path = "../../../kernel/src/fs/breenfs/commit_signal.rs"]
mod commit_signal;
#[cfg(test)]
#[path = "../../../kernel/src/fs/breenfs/dirty.rs"]
mod dirty;
#[cfg(test)]
#[path = "../../../kernel/src/fs/breenfs/vfs.rs"]
pub mod vfs_adapter;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Difference {
    pub ino: Ino,
    pub expected: Option<Inode>,
    pub actual: Option<Inode>,
}

/// Diff inode presence, names, hard links, bytes and metadata.
pub fn compare(model: &impl Fs, fs: &impl Fs) -> Result<Vec<Difference>> {
    let expected = model.state()?;
    let actual = fs.state()?;
    let mut ids: Vec<_> = expected
        .inodes
        .keys()
        .chain(actual.inodes.keys())
        .copied()
        .collect();
    ids.sort_unstable();
    ids.dedup();
    Ok(ids
        .into_iter()
        .filter_map(|ino| {
            let a = expected.inodes.get(&ino);
            let b = actual.inodes.get(&ino);
            (a != b).then(|| Difference {
                ino,
                expected: a.cloned(),
                actual: b.cloned(),
            })
        })
        .collect())
}

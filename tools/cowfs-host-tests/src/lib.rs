#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

#[path = "../../../kernel/src/fs/cowfs/btree.rs"]
pub mod btree;
#[path = "../../../kernel/src/fs/cowfs/crc32c.rs"]
pub mod crc32c;
#[path = "../../../kernel/src/fs/cowfs/format.rs"]
pub mod format;
#[path = "../../../kernel/src/fs/cowfs/siphash.rs"]
pub mod siphash;

pub mod model;
pub mod stream;

use alloc::{collections::BTreeMap, vec::Vec};

pub type Ino = u64;
pub const ROOT: Ino = 2;
pub const MAX_FILE_SIZE: u64 = (1u64 << 63) - 4096;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Errno {
    EPERM,
    ENOENT,
    EEXIST,
    ENOTDIR,
    EISDIR,
    EINVAL,
    EFBIG,
    EMLINK,
    ENAMETOOLONG,
    ENOTEMPTY,
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

/// Deliberately red until a later unit supplies the transaction writer adapter.
pub struct WriterAdapter;
impl Fs for WriterAdapter {
    fn execute(&mut self, _op: &Operation) -> Result<Reply> {
        Err(Error::Unimplemented)
    }
    fn state(&self) -> Result<State> {
        Err(Error::Unimplemented)
    }
}

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

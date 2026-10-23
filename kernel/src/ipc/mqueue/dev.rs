//! /dev/mqueue: the queue names as a directory, as Linux's mqueue
//! filesystem shows them. Listing it lists the queues, opening a name opens
//! (or with O_CREAT creates) that queue as mq_open would, without forcing
//! close-on-exec, reading the descriptor gives the queue's status text, and
//! unlinking a name is mq_unlink.

use super::store;
use super::{MqFile, MQ_NAMES};
use crate::fs::devfs::entries::MQUEUE_DIRECTORY_INO;
use crate::fs::permissions::Credentials;
use crate::ipc::fd::{FdKind, FileDescriptor};
use crate::ipc::object::IpcFile;
use crate::syscall::errno::{EACCES, EFAULT, EINVAL, EISDIR, EMFILE, ENOENT, ENOMEM, ENOTDIR, ESRCH};
use crate::syscall::fs::{Stat, O_DIRECTORY, O_TRUNC};
use crate::syscall::SyscallResult;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

/// The directory's mode: sticky and writable by all, as Linux mounts it.
const DIRECTORY_MODE: u32 = 0o041777;
const O_CREAT: u32 = 0o100;
const DT_DIR: u8 = 4;
const DT_REG: u8 = 8;

/// What `path` names under /dev/mqueue: the directory (an empty name), a
/// queue name, or nothing (a deeper path); `None` for a path outside it.
enum Entry<'a> {
    Directory,
    Name(&'a [u8]),
    Nothing,
}

fn entry(path: &str) -> Option<Entry<'_>> {
    if path == "/dev/mqueue" || path == "/dev/mqueue/" {
        return Some(Entry::Directory);
    }
    let name = path.strip_prefix("/dev/mqueue/")?;
    Some(if name.contains('/') { Entry::Nothing } else { Entry::Name(name.as_bytes()) })
}

fn result(r: Result<u64, u64>) -> SyscallResult {
    match r {
        Ok(value) => SyscallResult::Ok(value),
        Err(errno) => SyscallResult::Err(errno),
    }
}

fn device() -> u64 {
    crate::fs::vfs::mount::find_mount("/dev").unwrap_or(0) as u64
}

fn fill_directory(st: &mut Stat) {
    st.st_dev = device();
    st.st_ino = MQUEUE_DIRECTORY_INO;
    st.st_mode = DIRECTORY_MODE;
    st.st_nlink = 2;
    st.st_blksize = 4096;
}

/// The queue `name` names, if any.
fn lookup(name: &[u8]) -> Option<Arc<super::MessageQueue>> {
    MQ_NAMES.lock().get(name).cloned()
}

pub fn virtual_entry(path: &str) -> Option<Option<bool>> {
    Some(match entry(path)? {
        Entry::Directory => Some(true),
        Entry::Name(name) => lookup(name).map(|_| false),
        Entry::Nothing => None,
    })
}

/// open(2) of the directory or of a queue name.
pub fn open_path(path: &str, flags: u32, mode: u32) -> Option<SyscallResult> {
    Some(result(match entry(path)? {
        Entry::Directory => open_directory(flags),
        Entry::Name(_) if flags & O_DIRECTORY != 0 => Err(ENOTDIR as u64),
        Entry::Name(name) => store::check_name(name)
            .and_then(|()| super::calls::open_descriptor(name, flags, mode, None, flags)),
        Entry::Nothing => Err(ENOENT as u64),
    }))
}

fn open_directory(flags: u32) -> Result<u64, u64> {
    if flags & 3 != 0 || flags & (O_CREAT | O_TRUNC) != 0 {
        return Err(EISDIR as u64);
    }
    let file: Arc<dyn IpcFile> =
        Arc::try_new(MqDirectory { position: AtomicU64::new(0) }).map_err(|_| ENOMEM as u64)?;
    let descriptor = FileDescriptor::try_opened(FdKind::Ipc(file), flags)?;
    let tid = crate::task::scheduler::current_thread_id().ok_or(ESRCH as u64)?;
    crate::process::with_process_manager(|manager| {
        let (_, process) = manager.find_process_by_thread_mut(tid).ok_or(ESRCH as u64)?;
        let slot = process.fd_table.reserve_slot().ok_or(EMFILE as u64)?;
        process.fd_table.install_reserved(slot, descriptor).map(|fd| fd as u64).map_err(|_| EMFILE as u64)
    })
    .ok_or(ESRCH as u64)?
}

/// unlink(2) of a queue name is mq_unlink.
pub fn unlink_path(path: &str) -> Option<SyscallResult> {
    Some(result(match entry(path)? {
        Entry::Directory => Err(EISDIR as u64),
        Entry::Name(name) => {
            store::check_name(name)
                .and_then(|()| Credentials::current(false))
                .and_then(|cred| super::unlink(name, &cred).map(|()| 0))
        }
        Entry::Nothing => Err(ENOENT as u64),
    }))
}

pub fn stat_path(path: &str, statbuf: u64) -> Option<SyscallResult> {
    let mut st = Stat::zeroed();
    let filled = match entry(path)? {
        Entry::Directory => {
            fill_directory(&mut st);
            Ok(())
        }
        Entry::Name(name) => lookup(name).ok_or(ENOENT as u64).map(|queue| {
            MqFile::new(queue).fill_stat(&mut st);
            st.st_dev = device();
        }),
        Entry::Nothing => Err(ENOENT as u64),
    };
    Some(result(filled.and_then(|()| {
        crate::syscall::userptr::copy_to_user(statbuf as *mut Stat, &st).map_err(|_| EFAULT as u64)?;
        Ok(0)
    })))
}

/// access(2): the directory's mode, or the queue's owner and mode.
pub fn access_path(path: &str, mode: u32, cred: &Credentials) -> Option<SyscallResult> {
    let (uid, gid, bits) = match entry(path)? {
        Entry::Directory => (0, 0, DIRECTORY_MODE),
        Entry::Name(name) => match lookup(name) {
            Some(queue) => {
                let inner = queue.inner.lock();
                (inner.uid, inner.gid, inner.mode)
            }
            None => return Some(SyscallResult::Err(ENOENT as u64)),
        },
        Entry::Nothing => return Some(SyscallResult::Err(ENOENT as u64)),
    };
    Some(if cred.may(crate::fs::attrs::InodeAttrs::new(bits, uid, gid), mode) { SyscallResult::Ok(0) } else { SyscallResult::Err(EACCES as u64) })
}

/// An open /dev/mqueue directory and its position. Entry positions are
/// stable across creates and unlinks: `.` is 0, `..` is 1 and a queue is its
/// inode number plus 2, and the listing runs in that order.
struct MqDirectory {
    position: AtomicU64,
}

impl core::fmt::Debug for MqDirectory {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("MqDirectory")
    }
}

/// Append one linux_dirent64 record for the entry at position `at` to `out`
/// if it fits in `count` bytes and in `out`'s capacity, which is reserved
/// beforehand so this allocates nothing.
fn push_dirent(out: &mut Vec<u8>, count: usize, ino: u64, at: u64, kind: u8, name: &[u8]) -> bool {
    let len = (19 + name.len() + 1 + 7) & !7;
    if out.len() + len > count.min(out.capacity()) {
        return false;
    }
    out.extend_from_slice(&ino.to_ne_bytes());
    out.extend_from_slice(&(at + 1).to_ne_bytes());
    out.extend_from_slice(&(len as u16).to_ne_bytes());
    out.push(kind);
    out.extend_from_slice(name);
    out.resize(out.len() + len - 19 - name.len(), 0);
    true
}

/// What fstat reports as /dev's inode number, for `..`.
const DEV_DIRECTORY_INO: u64 = 1;

impl MqDirectory {
    /// The records from position `from` that fit in `count` bytes, the
    /// position after the last, and whether an entry was left out. Built
    /// under MQ_NAMES, with no user memory touched.
    fn batch(from: u64, count: usize) -> Result<(Vec<u8>, u64, bool), u64> {
        let mut out = Vec::new();
        out.try_reserve_exact(count.min(64 * 1024)).map_err(|_| ENOMEM as u64)?;
        let mut at = from;
        for (position, ino, name) in [(0, MQUEUE_DIRECTORY_INO, &b"."[..]), (1, DEV_DIRECTORY_INO, &b".."[..])] {
            if at == position {
                if !push_dirent(&mut out, count, ino, position, DT_DIR, name) {
                    return Ok((out, at, true));
                }
                at = position + 1;
            }
        }
        let names = MQ_NAMES.lock();
        loop {
            let next = names.iter().filter(|(_, q)| q.ino() + 2 >= at).min_by_key(|(_, q)| q.ino());
            let Some((name, queue)) = next else {
                return Ok((out, at, false));
            };
            let position = queue.ino() + 2;
            if !push_dirent(&mut out, count, queue.ino(), position, DT_REG, name) {
                return Ok((out, at, true));
            }
            at = position + 1;
        }
    }
}

impl IpcFile for MqDirectory {
    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    fn fill_stat(&self, st: &mut Stat) {
        fill_directory(st);
    }

    fn statfs_magic(&self) -> u64 {
        0x19800202
    }

    /// The records are built and copied out, then the position is advanced
    /// from where they began; a concurrent read of the same description that
    /// moved it first makes this one start again from there. A buffer too
    /// small for the next record is EINVAL.
    fn getdents(&self, dirp: u64, count: usize) -> Result<u64, u64> {
        loop {
            let from = self.position.load(Ordering::Acquire);
            let (records, next, more) = Self::batch(from, count)?;
            if records.is_empty() {
                return if more { Err(EINVAL as u64) } else { Ok(0) };
            }
            crate::syscall::userptr::write_user_bytes(dirp, records.as_ptr(), records.len())?;
            if self.position.compare_exchange(from, next, Ordering::AcqRel, Ordering::Acquire).is_ok() {
                return Ok(records.len() as u64);
            }
        }
    }
}

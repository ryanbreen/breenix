//! Open daemon handles. Only the short state updates hold spin locks.
use super::{
    io_split::{pack_dirent, Transfer},
    path, FileLease, Pending, Release,
};
use crate::ipc::object::IpcFile;
use crate::ipc::wait::{wait_on, IpcDeadline, WaitEnd};
use crate::syscall::{errno::*, userptr};
use crate::task::waitqueue::WaitQueueHead;
use alloc::{boxed::Box, sync::Arc, vec::Vec};
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;
use ufs_abi::{
    payload::{Attr, Reader, MAX_WRITE},
    Op, Request,
};

/// Sleeping serialization for an operation that waits on a daemon reply.
pub(super) struct Gate {
    busy: AtomicBool,
    waiters: WaitQueueHead,
}
pub(super) struct Guard<'a>(&'a Gate);
impl Gate {
    pub fn new() -> Self {
        Self {
            busy: AtomicBool::new(false),
            waiters: WaitQueueHead::new(),
        }
    }
    pub(super) fn enter(&self) -> Result<Guard<'_>, u64> {
        loop {
            if self
                .busy
                .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                return Ok(Guard(self));
            }
            match wait_on(&self.waiters, &IpcDeadline::Never, || {
                self.busy.load(Ordering::Acquire)
            }) {
                WaitEnd::Interrupted(e) => return Err(e),
                WaitEnd::Woken | WaitEnd::TimedOut => {}
            }
        }
    }
}
impl Drop for Guard<'_> {
    fn drop(&mut self) {
        self.0.busy.store(false, Ordering::Release);
        self.0.waiters.wake_up();
    }
}

pub struct UfsFile {
    pub mount: u64,
    pub node: u64,
    pub directory: bool,
    parent: u64,
    handle: u64,
    position: Mutex<u64>,
    dots: Mutex<u8>,
    serial: Gate,
    mutation: Arc<Gate>,
    lease: FileLease,
    release: Option<Box<Release>>,
}
impl core::fmt::Debug for UfsFile {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("UfsFile")
    }
}

fn empty(bytes: Vec<u8>) -> Result<(), u64> {
    if bytes.is_empty() {
        Ok(())
    } else {
        Err(EIO as u64)
    }
}
fn buffer(len: usize) -> Result<Vec<u8>, u64> {
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(len).map_err(|_| ENOMEM as u64)?;
    bytes.resize(len, 0);
    Ok(bytes)
}

impl UfsFile {
    pub fn open(
        mount: u64,
        node: u64,
        parent: Option<u64>,
        name: &[u8],
        flags: u32,
        mode: u32,
        cred: &crate::fs::permissions::Credentials,
    ) -> Result<Arc<Self>, u64> {
        Self::open_with_access(mount, node, parent, name, flags, mode, cred, None)
    }

    pub fn working_directory(
        mount: u64, node: u64, parent: Option<u64>,
        cred: &crate::fs::permissions::Credentials,
    ) -> Result<Arc<Self>, u64> {
        Self::open_with_access(mount, node, parent, b"", crate::syscall::fs::O_DIRECTORY, 0, cred, Some(1))
    }

    fn open_with_access(
        mount: u64, node: u64, parent: Option<u64>, name: &[u8], flags: u32, mode: u32,
        cred: &crate::fs::permissions::Credentials, access_override: Option<u32>,
    ) -> Result<Arc<Self>, u64> {
        use crate::syscall::fs::{O_CREAT, O_DIRECTORY, O_EXCL, O_TRUNC};
        let lease = super::open_file(mount)?;
        let creating = node == 0;
        let mut created = false;
        if flags & O_TRUNC != 0 && flags & 3 == 0 {
            return Err(EACCES as u64);
        }
        if flags & 3 == 3 {
            return Err(EINVAL as u64);
        }
        let attr = if creating {
            if flags & O_CREAT == 0 {
                return Err(ENOENT as u64);
            }
            if flags & O_DIRECTORY != 0 {
                return Err(ENOTDIR as u64);
            }
            let parent = parent.ok_or(ENOENT as u64)?;
            let dir = path::parent_access(mount, parent, cred, None)?;
            let gid = if dir.mode & 0o2000 != 0 {
                dir.gid
            } else {
                cred.egid
            };
            let mode = crate::syscall::fifo_policy::creation_mode(mode, cred, gid, false) as u32;
            match path::named(mount, Op::Create, parent, name, Some(mode)) {
                Ok(bytes) => {
                    created = true;
                    Attr::decode(&bytes).map_err(|_| EIO as u64)?
                }
                Err(e) if e == EEXIST as u64 && flags & O_EXCL == 0 => {
                    path::lookup(mount, parent, name)?
                }
                Err(e) => return Err(e),
            }
        } else {
            if flags & (O_CREAT | O_EXCL) == O_CREAT | O_EXCL {
                return Err(EEXIST as u64);
            }
            path::getattr(mount, node)?
        };
        let directory = attr.mode & 0o170000 == 0o040000;
        if attr.node == 0
            || attr.size > i64::MAX as u64
            || (!directory && attr.mode & 0o170000 != 0o100000)
        {
            return Err(EIO as u64);
        }
        if directory && (flags & 3 != 0 || flags & O_TRUNC != 0) {
            return Err(EISDIR as u64);
        }
        if !directory && flags & O_DIRECTORY != 0 {
            return Err(ENOTDIR as u64);
        }
        let access = access_override.unwrap_or(match flags & 3 {
            0 => 4,
            1 => 2,
            2 => 6,
            _ => return Err(EINVAL as u64),
        });
        if !created && !cred.may(path::attrs(attr), access) {
            return Err(EACCES as u64);
        }
        let mutation = lease.0.mutation_gate(attr.node)?;
        // Allocate the close record before acquiring the daemon handle.
        let pending = Arc::try_new(Pending {
            answer: Mutex::new(None),
        })
        .map_err(|_| ENOMEM as u64)?;
        let bytes = buffer(ufs_abi::REQUEST_HEADER + 8)?;
        let release = Box::try_new(Release {
            bytes,
            pending,
            next: None,
        })
        .map_err(|_| ENOMEM as u64)?;
        // O_TRUNC is performed below, after the handle has an owning object.
        let reply = path::request(
            mount,
            Op::Open,
            attr.node,
            &(flags & !O_TRUNC).to_le_bytes(),
        )?;
        let mut r = Reader::new(&reply);
        let handle = r.u64().map_err(|_| EIO as u64)?;
        r.finish().map_err(|_| EIO as u64)?;
        let mut file = Self {
            mount,
            node: attr.node,
            directory,
            parent: parent.unwrap_or(attr.node),
            handle,
            position: Mutex::new(0),
            dots: Mutex::new(0),
            serial: Gate::new(),
            mutation,
            lease,
            release: Some(release),
        };
        let release = file.release.as_mut().ok_or(EIO as u64)?;
        Request {
            id: 0,
            op: Op::Release,
            node: file.node,
            payload: &handle.to_le_bytes(),
        }
        .encode(&mut release.bytes)
        .map_err(|_| EIO as u64)?;
        // A failed allocation drops file and queues the acquired handle too.
        let file = Arc::try_new(file).map_err(|_| ENOMEM as u64)?;
        if flags & O_TRUNC != 0 {
            file.truncate(0)?;
        }
        Ok(file)
    }

    fn transfer_read(&self, buf: u64, count: usize, offset: u64) -> Result<u64, u64> {
        if self.directory {
            return Err(EISDIR as u64);
        }
        let mut transfer = Transfer::new(count.min(i64::MAX as usize), ufs_abi::MAX_DATA);
        let mut total = 0;
        while let Some(len) = transfer.next() {
            let result = (|| {
                let at = offset
                    .checked_add(total)
                    .filter(|n| *n <= i64::MAX as u64)
                    .ok_or(EOVERFLOW as u64)?;
                let mut payload = [0; 20];
                payload[..8].copy_from_slice(&self.handle.to_le_bytes());
                payload[8..16].copy_from_slice(&at.to_le_bytes());
                payload[16..].copy_from_slice(&(len as u32).to_le_bytes());
                let bytes = path::request(self.mount, Op::Read, self.node, &payload)?;
                transfer.complete(bytes.len()).map_err(|_| EIO as u64)?;
                userptr::write_user_bytes(
                    buf.checked_add(total).ok_or(EFAULT as u64)?,
                    bytes.as_ptr(),
                    bytes.len(),
                )?;
                Ok(bytes.len() as u64)
            })();
            match result {
                Ok(n) => total += n,
                Err(e) => return if total == 0 { Err(e) } else { Ok(total) },
            }
        }
        Ok(total)
    }

    fn transfer_write(&self, bytes: &[u8], offset: u64) -> Result<u64, u64> {
        if self.directory {
            return Err(EISDIR as u64);
        }
        let count = match crate::syscall::resource::write_length(
            offset,
            bytes.len(),
            crate::syscall::resource::current_fsize(),
        ) {
            Ok(n) => n,
            Err(e) => {
                crate::syscall::resource::signal_fsize();
                return Err(e);
            }
        };
        let mut transfer = Transfer::new(count, MAX_WRITE);
        let mut total = 0usize;
        while let Some(len) = transfer.next() {
            let result = (|| {
                let at = offset
                    .checked_add(total as u64)
                    .filter(|n| *n <= i64::MAX as u64)
                    .ok_or(EFBIG as u64)?;
                let mut payload = buffer(16 + len)?;
                payload[..8].copy_from_slice(&self.handle.to_le_bytes());
                payload[8..16].copy_from_slice(&at.to_le_bytes());
                payload[16..].copy_from_slice(&bytes[total..total + len]);
                let reply = path::request(self.mount, Op::Write, self.node, &payload)?;
                let mut r = Reader::new(&reply);
                let n = r.u32().map_err(|_| EIO as u64)? as usize;
                r.finish().map_err(|_| EIO as u64)?;
                transfer.complete(n).map_err(|_| EIO as u64)?;
                Ok(n)
            })();
            match result {
                Ok(n) => total += n,
                Err(e) => return if total == 0 { Err(e) } else { Ok(total as u64) },
            }
        }
        Ok(total as u64)
    }

    pub fn pread(&self, buf: u64, count: usize, offset: u64) -> Result<u64, u64> {
        self.transfer_read(buf, count, offset)
    }
    pub fn pwrite(&self, buf: u64, count: usize, offset: u64, append: bool) -> Result<u64, u64> {
        let _mutation = self.mutation.enter()?;
        let offset = if append {
            path::getattr(self.mount, self.node)?.size
        } else {
            offset
        };
        // Snapshot each chunk, rather than allocating count bytes in the kernel.
        let mut total = 0;
        let mut transfer = Transfer::new(count, MAX_WRITE);
        while let Some(len) = transfer.next() {
            let result = (|| {
                let mut bytes = buffer(len)?;
                userptr::read_user_bytes(
                    bytes.as_mut_ptr(),
                    buf.checked_add(total).ok_or(EFAULT as u64)?,
                    len,
                )?;
                self.transfer_write(&bytes, offset.checked_add(total).ok_or(EFBIG as u64)?)
            })();
            match result {
                Ok(n) => {
                    transfer.complete(n as usize).map_err(|_| EIO as u64)?;
                    total += n;
                }
                Err(e) => return if total == 0 { Err(e) } else { Ok(total) },
            }
        }
        Ok(total)
    }
    pub fn sync(&self) -> Result<(), u64> {
        empty(path::request(
            self.mount,
            Op::Fsync,
            self.node,
            &self.handle.to_le_bytes(),
        )?)
    }
}

impl IpcFile for UfsFile {
    fn as_any(&self) -> &dyn core::any::Any {
        self
    }
    fn fill_stat(&self, stat: &mut crate::syscall::fs::Stat) {
        // fstat obtains size, ownership and times through its fallible daemon path.
        stat.st_dev = 0x8000_0000_0000_0000 | self.mount;
        stat.st_ino = self.node;
        stat.st_mode = if self.directory { 0o040000 } else { 0o100000 };
    }
    fn statfs_magic(&self) -> u64 {
        ufs_abi::payload::MAGIC as u64
    }
    fn poll_events(&self) -> i16 {
        1 | 4
    }
    fn truncate(&self, len: u64) -> Result<(), u64> {
        if self.directory {
            return Err(EISDIR as u64);
        }
        let _mutation = self.mutation.enter()?;
        if len > crate::syscall::resource::current_fsize() {
            crate::syscall::resource::signal_fsize();
            return Err(EFBIG as u64);
        }
        let bytes = path::request(self.mount, Op::Setattr, self.node, &len.to_le_bytes())?;
        let attr = Attr::decode(&bytes).map_err(|_| EIO as u64)?;
        if attr.node != self.node || attr.size != len {
            return Err(EIO as u64);
        }
        Ok(())
    }
    fn read(&self, buf: u64, count: usize) -> Result<u64, u64> {
        let _serial = self.serial.enter()?;
        let pos = *self.position.lock();
        let n = self.transfer_read(buf, count, pos)?;
        *self.position.lock() = pos + n;
        Ok(n)
    }
    fn write(&self, bytes: &[u8], append: bool) -> Result<u64, u64> {
        let _serial = self.serial.enter()?;
        let _mutation = self.mutation.enter()?;
        let pos = if append {
            path::getattr(self.mount, self.node)?.size
        } else {
            *self.position.lock()
        };
        let n = self.transfer_write(bytes, pos)?;
        *self.position.lock() = pos + n;
        Ok(n)
    }
    fn seek(&self, offset: i64, whence: i32) -> Result<u64, u64> {
        let _serial = self.serial.enter()?;
        let base = match whence {
            0 => 0,
            1 => *self.position.lock(),
            2 if !self.directory => path::getattr(self.mount, self.node)?.size,
            _ => return Err(EINVAL as u64),
        };
        let pos = (base as i128 + offset as i128)
            .try_into()
            .map_err(|_| EINVAL as u64)?;
        if !self.directory && pos > i64::MAX as u64 {
            return Err(EINVAL as u64);
        }
        *self.position.lock() = pos;
        if self.directory {
            *self.dots.lock() = if pos == 0 { 0 } else { 2 };
        }
        Ok(pos)
    }
    fn getdents(&self, dirp: u64, count: usize) -> Result<u64, u64> {
        if !self.directory {
            return Err(ENOTDIR as u64);
        }
        if count == 0 {
            return Err(EINVAL as u64);
        }
        let _serial = self.serial.enter()?;
        let original = *self.position.lock();
        // Dot progress is separate: daemon cookies are opaque and unmodified.
        let mut cookie = original;
        let mut dots = *self.dots.lock();
        let mut out = buffer(count.min(ufs_abi::MAX_DATA))?;
        let mut used = 0;
        while dots < 2 {
            let (ino, name): (u64, &[u8]) = if dots == 0 {
                (self.node, b".")
            } else {
                (self.parent, b"..")
            };
            let Some(n) = pack_dirent(&mut out[used..], ino, cookie, 4, name) else {
                break;
            };
            used += n;
            dots += 1;
        }
        if dots >= 2 {
            let reply = path::request(self.mount, Op::Readdir, self.node, &cookie.to_le_bytes());
            match reply {
                Ok(bytes) => {
                    let mut r = Reader::new(&bytes);
                    let mut consumed = 0;
                    while consumed < bytes.len() {
                        let next = r.u64().map_err(|_| EIO as u64)?;
                        let node = r.u64().map_err(|_| EIO as u64)?;
                        let mode = r.u32().map_err(|_| EIO as u64)?;
                        let name = r.name().map_err(|_| EIO as u64)?;
                        consumed += 22 + name.len();
                        if next == cookie {
                            return Err(EIO as u64);
                        }
                        let resume = next;
                        let kind = match mode & 0o170000 {
                            0o040000 => 4,
                            0o100000 => 8,
                            _ => return Err(EIO as u64),
                        };
                        if node == 0 {
                            return Err(EIO as u64);
                        }
                        let Some(n) = pack_dirent(&mut out[used..], node, resume, kind, name)
                        else {
                            break;
                        };
                        used += n;
                        cookie = resume;
                    }
                    if used == 0 && !bytes.is_empty() {
                        return Err(EINVAL as u64);
                    }
                }
                Err(e) if used == 0 => return Err(e),
                Err(_) => {}
            }
        }
        if used == 0 && dots < 2 {
            return Err(EINVAL as u64);
        }
        userptr::write_user_bytes(dirp, out.as_ptr(), used)?;
        *self.position.lock() = cookie;
        *self.dots.lock() = dots;
        Ok(used as u64)
    }
}
impl Drop for UfsFile {
    fn drop(&mut self) {
        let Some(mut release) = self.release.take() else {
            return;
        };
        let mount = &self.lease.0;
        {
            let mut state = mount.state.lock();
            if state.dead || state.detached {
                return;
            }
            release.next = state.releases.take();
            state.releases = Some(release);
        }
        mount.defer();
    }
}

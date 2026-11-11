use super::*;
use std::time::{Duration, Instant};
use ufs_abi::payload::{SetTimes, MAX_WRITE};

/// Leave one second of the UFS request timeout for dispatch and reply delivery.
pub const OPERATION_TIMEOUT: Duration =
    Duration::from_millis(ufs_abi::payload::REQUEST_TIMEOUT_MS - 1000);
pub const O_WRONLY: u32 = 1;
pub const O_RDWR: u32 = 2;
pub const O_CREAT: u32 = 0x40;
pub const O_EXCL: u32 = 0x80;
pub const O_TRUNC: u32 = 0x200;
pub const O_DIRECTORY: u32 = 0x10000;
pub(super) const DISCONNECTED: u32 = 0xffff_ffff;
pub(super) const PROTOCOL: u32 = 0xffff_fffe;

pub(super) fn invalid_session(status: u32) -> bool {
    disconnected(status) || status == PROTOCOL
}


pub(super) fn disconnected(status: u32) -> bool {
    matches!(
        status,
        DISCONNECTED | 0xc000_020c | 0xc000_020d | 0xc000_00c9 | 0xc000_0203 | 0xc000_035c
    )
}
pub(super) struct Handle {
    pub path: Option<String>,
    access: u32,
    options: u32,
    pub file_id: Option<FileId>,
    identity: Option<(u64, u64, bool)>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Statfs {
    pub bsize: u64,
    pub blocks: u64,
    pub bfree: u64,
    pub bavail: u64,
    pub files: u64,
    pub ffree: u64,
    pub namelen: u64,
}
fn filetime(ns: u64) -> u64 {
    ns / 100 + 116_444_736_000_000_000
}

// MS-FSCC 2.1.9 and 2.4.27: index_number == 0 denotes an unsupported file ID.
fn file_identity(info: &FileAllInformation) -> Option<(u64, u64, bool)> {
    (info.index_number != 0).then_some((info.index_number, info.creation_time, info.directory))
}

impl<B: Backend> Namespace<B> {
    fn disconnect(&mut self) {
        self.connected = false;
        self.invalidate();
        for handle in self.handles.values_mut() {
            handle.file_id = None;
        }
    }
    fn recover(&mut self, deadline: Instant) -> Result<(), i32> {
        self.disconnect();
        self.backend.reconnect(deadline).map_err(|_| 5)?;
        let mut reopened = Vec::new();
        for (&key, handle) in &self.handles {
            // No stable identity or surviving path means this handle stays stale.
            let (Some(path), Some(identity)) = (&handle.path, handle.identity) else {
                continue;
            };
            let result = self.backend
                .open(path, handle.access, CreateDisposition::Open, handle.options)
                .and_then(|id| {
                    match self.backend.query_file(id) {
                        Ok(info) if file_identity(&info) == Some(identity) => Ok(Some(id)),
                        Ok(_) => self.backend.close(id).map(|()| None),
                        Err(status) => {
                            if !invalid_session(status) {
                                let _ = self.backend.close(id);
                            }
                            Err(status)
                        }
                    }
                });
            match result {
                Ok(Some(id)) => reopened.push((key, id)),
                Ok(None) => {},
                Err(status) if !invalid_session(status) => {},
                Err(_) => {
                    // Discard the failed session before publishing reopened handles.
                    return Err(5);
                }
            }
        }
        for (key, id) in reopened {
            self.handles.get_mut(&key).unwrap().file_id = Some(id);
        }
        self.connected = true;
        Ok(())
    }
    pub(super) fn operation<T>(
        &mut self,
        mut f: impl FnMut(&mut Self) -> Result<T, u32>,
    ) -> Result<T, i32> {
        let deadline = Instant::now() + OPERATION_TIMEOUT;
        self.backend.set_deadline(deadline).map_err(errno)?;
        let mut retried = false;
        if !self.connected {
            self.recover(deadline)?;
            retried = true;
        }
        match f(self) {
            Ok(value) => Ok(value),
            Err(status) if disconnected(status) => {
                self.disconnect();
                if retried {
                    return Err(5);
                }
                self.recover(deadline)?;
                match f(self) {
                    Ok(value) => Ok(value),
                    Err(status) => {
                        if invalid_session(status) {
                            self.disconnect();
                        }
                        Err(errno(status))
                    }
                }
            }
            Err(status) => {
                if invalid_session(status) {
                    self.disconnect();
                }
                Err(errno(status))
            },
        }
    }
    fn file_id(&self, handle: u64) -> Result<FileId, u32> {
        self.handles
            .get(&handle)
            .and_then(|h| h.file_id)
            .ok_or(0xffff_fffd)
    }
    pub fn open(&mut self, node: u64, flags: u32) -> Result<u64, i32> {
        let path = self.path(node)?.to_owned();
        let access = access::READ_ATTRIBUTES
            | access::SYNCHRONIZE
            | match flags & 3 {
                0 => access::READ_DATA,
                O_WRONLY => access::WRITE_DATA,
                O_RDWR => access::READ_DATA | access::WRITE_DATA,
                _ => return Err(22),
            };
        if flags & O_TRUNC != 0 && flags & 3 == 0 {
            return Err(22);
        }
        let opts = if flags & O_DIRECTORY != 0 {
            options::DIRECTORY_FILE
        } else {
            0
        };
        let disp = if flags & O_TRUNC != 0 {
            CreateDisposition::Overwrite
        } else {
            CreateDisposition::Open
        };
        if flags & O_TRUNC != 0 {
            self.invalidate();
        }
        let handle = self.next_handle;
        let next = self.next_handle.checked_add(1).ok_or(24)?;
        let (id, identity) = self.operation(|s| {
            let id = s.backend.open(&path, access, disp, opts)?;
            match s.backend.query_file(id) {
                Ok(info) => Ok((id, file_identity(&info))),
                Err(status) => {
                    if !invalid_session(status) {
                        let _ = s.backend.close(id);
                    }
                    Err(status)
                }
            }
        })?;
        self.next_handle = next;
        self.handles.insert(
            handle,
            Handle {
                path: Some(path),
                access,
                options: opts,
                file_id: Some(id),
                identity,
            },
        );
        Ok(handle)
    }
    /// Create returns attributes; the UFS caller sends Open separately.
    pub fn create(&mut self, parent: u64, name: &str, flags: u32, now: u64) -> Result<Attr, i32> {
        let path = self.child(parent, name)?;
        self.invalidate();
        let disp = if flags & O_EXCL != 0 {
            CreateDisposition::Create
        } else if flags & O_TRUNC != 0 {
            CreateDisposition::OverwriteIf
        } else {
            CreateDisposition::OpenIf
        };
        let info = self.with_handle(
            &path,
            access::WRITE_DATA | access::READ_ATTRIBUTES,
            disp,
            options::NON_DIRECTORY_FILE,
            |b, h| b.query_file(h),
        )?;
        let node = self.node(path, parent);
        let a = attr(node, &info);
        self.cache.insert(node, (now, a));
        Ok(a)
    }
    pub fn read(&mut self, handle: u64, offset: u64, length: usize) -> Result<Vec<u8>, i32> {
        if !self.handles.contains_key(&handle) {
            return Err(9);
        }
        offset.checked_add(length as u64).ok_or(22)?;
        self.operation(|s| {
            let id = s.file_id(handle)?;
            let mut data = Vec::new();
            while data.len() < length {
                let count = (length - data.len()).min(ufs_abi::MAX_DATA);
                let part = s.backend.read_at(id, offset + data.len() as u64, count)?;
                if part.len() > count {
                    return Err(0xffff_fffe);
                }
                let short = part.len() < count;
                data.extend_from_slice(&part);
                if short {
                    break;
                }
            }
            Ok(data)
        })
    }
    pub fn write(&mut self, handle: u64, offset: u64, data: &[u8]) -> Result<usize, i32> {
        if !self.handles.contains_key(&handle) {
            return Err(9);
        }
        offset.checked_add(data.len() as u64).ok_or(22)?;
        self.invalidate();
        self.operation(|s| {
            let id = s.file_id(handle)?;
            for (chunk, part) in data.chunks(MAX_WRITE).enumerate() {
                s.backend
                    .write_at(id, offset + (chunk * MAX_WRITE) as u64, part)?;
            }
            Ok(data.len())
        })
    }
    pub fn setattr(
        &mut self,
        node: u64,
        size: Option<u64>,
        times: Option<SetTimes>,
        now: u64,
    ) -> Result<Attr, i32> {
        let path = self.path(node)?.to_owned();
        self.invalidate();
        let access = access::READ_ATTRIBUTES
            | access::WRITE_ATTRIBUTES
            | if size.is_some() {
                access::WRITE_DATA
            } else {
                0
            };
        let info = self.with_handle(&path, access, CreateDisposition::Open, 0, |b, id| {
            if let Some(size) = size {
                b.set_info(id, SetInfo::EndOfFile(size))?;
            }
            if let Some(times) = times {
                b.set_info(
                    id,
                    SetInfo::Basic {
                        creation_time: 0,
                        last_access_time: times.atime_ns.map_or(0, filetime),
                        last_write_time: times.mtime_ns.map_or(0, filetime),
                        change_time: 0,
                        attributes: 0,
                    },
                )?;
            }
            b.query_file(id)
        })?;
        let a = attr(node, &info);
        self.cache.insert(node, (now, a));
        Ok(a)
    }
    pub fn fsync(&mut self, handle: u64) -> Result<(), i32> {
        let held = self.handles.get(&handle).ok_or(9)?;
        if held.access & (access::WRITE_DATA | access::APPEND_DATA) == 0 {
            // MS-SMB2 3.3.5.11 also requires add-file/subdirectory rights on directories.
            let path = held.path.clone().ok_or(5)?;
            let options = held.options;
            return self.with_handle(
                &path,
                access::WRITE_DATA | access::SYNCHRONIZE,
                CreateDisposition::Open,
                options,
                |backend, id| backend.flush(id),
            );
        }
        self.operation(|s| s.backend.flush(s.file_id(handle)?))
    }
    pub fn statfs(&mut self, node: u64) -> Result<Statfs, i32> {
        let path = self.path(node)?.to_owned();
        let info = self.with_handle(
            &path,
            access::READ_ATTRIBUTES,
            CreateDisposition::Open,
            0,
            |b, id| b.query_fs_size(id),
        )?;
        let bsize = u64::from(info.sectors_per_allocation_unit) * u64::from(info.bytes_per_sector);
        if bsize == 0 {
            return Err(5);
        }
        Ok(Statfs {
            bsize,
            blocks: info.total_allocation_units,
            bfree: info.actual_available_allocation_units,
            bavail: info.caller_available_allocation_units,
            files: 0,
            ffree: 0,
            namelen: ufs_abi::NAME_MAX as u64,
        })
    }
    pub fn release(&mut self, handle: u64) -> Result<(), i32> {
        let Some(h) = self.handles.remove(&handle) else {
            return Ok(());
        };
        let Some(id) = h.file_id else {
            return Ok(());
        };
        self.backend
            .set_deadline(Instant::now() + OPERATION_TIMEOUT)
            .map_err(errno)?;
        match self.backend.close(id) {
            Ok(()) => Ok(()),
            Err(status) => {
                if invalid_session(status) {
                    self.disconnect();
                }
                Err(errno(status))
            }
        }
    }
}

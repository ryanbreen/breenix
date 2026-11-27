//! File operations for the SSH "sftp" subsystem.
//!
//! The caller enters the authenticated account before reading SFTP data.
//! File operations run under the worker's process credentials.

#[path = "bsshd_sftp/codec.rs"]
mod codec;

use self::codec::{
    Attrs, Name, Reply, Request, Status, APPEND, CREATE, EXCLUSIVE, READ, TRUNCATE, WRITE,
};
use std::collections::BTreeMap;
use std::ffi::{c_char, CString, OsStr};
use std::fs::{self, File, FileTimes, Metadata, OpenOptions, ReadDir};
use std::io::{self, ErrorKind, Read, Write};
use std::os::fd::IntoRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{
    self as unix, DirBuilderExt, FileExt, MetadataExt, OpenOptionsExt, PermissionsExt,
};
use std::path::Path;
use std::time::{Duration, UNIX_EPOCH};

const MAX_HANDLES: usize = 128;
const READDIR_BATCH: usize = 32;

extern "C" {
    fn close(fd: i32) -> i32;
    fn truncate(path: *const c_char, length: i64) -> i32;
    fn utime(path: *const c_char, times: *const [i64; 2]) -> i32;
}

/// Serve sequential requests over the single SSH channel byte stream.
/// `target_first` selects the SYMLINK operand order used by OpenSSH clients.
pub fn serve(stream: &mut (impl Read + Write), target_first: bool) -> io::Result<()> {
    let Some(init) = codec::read_frame(stream)? else {
        return Ok(());
    };
    codec::negotiate(stream, &init)?;

    let mut session = Session {
        handles: BTreeMap::new(),
        next_handle: 1,
        target_first,
    };
    while let Some(frame) = codec::read_frame(stream)? {
        let (kind, id, fields) = codec::request_header(&frame)?;
        let reply = if let Some(request) = codec::decode(kind, fields) {
            session
                .handle(request)
                .unwrap_or_else(|error| Reply::Status(fs_status(&error)))
        } else {
            Reply::Status(Status::BadMessage)
        };
        reply.send(stream, id)?;
    }
    Ok(())
}

enum Handle {
    File { file: File, append: bool },
    Directory(ReadDir),
}

struct Session {
    handles: BTreeMap<u64, Handle>,
    next_handle: u64,
    target_first: bool,
}

impl Session {
    fn handle(&mut self, request: Request<'_>) -> io::Result<Reply> {
        match request {
            Request::Open(name, flags, attrs) => self.allocate(|| {
                let file = open(path(name), flags, attrs)?;
                Ok(Handle::File {
                    file,
                    append: flags & APPEND != 0,
                })
            }),
            Request::Close(id) => {
                let handle = self
                    .handles
                    .remove(&handle_id(id)?)
                    .ok_or_else(invalid_handle)?;
                if let Handle::File { file, .. } = handle {
                    // SAFETY: into_raw_fd transfers ownership to this close call.
                    syscall_result(unsafe { close(file.into_raw_fd()) })?;
                }
                Ok(Reply::Status(Status::Ok))
            }
            Request::Read(id, offset, length) => {
                let (file, _) = self.file(id)?;
                read_file(file, offset, length)
            }
            Request::Write(id, offset, data) => {
                let (file, append) = self.file(id)?;
                if append {
                    // Use the open file's O_APPEND semantics for each write.
                    (&*file).write_all(data)?;
                } else {
                    check_range(offset, data.len())?;
                    file.write_all_at(data, offset)?;
                }
                Ok(Reply::Status(Status::Ok))
            }
            Request::Stat(name, follow) => {
                let metadata = if follow {
                    fs::metadata(path(name))?
                } else {
                    fs::symlink_metadata(path(name))?
                };
                Ok(Reply::Attrs(attributes(&metadata)))
            }
            Request::Fstat(id) => {
                let (file, _) = self.file(id)?;
                Ok(Reply::Attrs(attributes(&file.metadata()?)))
            }
            Request::Setstat(name, attrs) => {
                set_path_attributes(path(name), attrs)?;
                Ok(Reply::Status(Status::Ok))
            }
            Request::Fsetstat(id, attrs) => {
                let (file, _) = self.file(id)?;
                set_file_attributes(file, attrs)?;
                Ok(Reply::Status(Status::Ok))
            }
            Request::Opendir(name) => {
                self.allocate(|| Ok(Handle::Directory(fs::read_dir(path(name))?)))
            }
            Request::Readdir(id) => {
                let Some(Handle::Directory(entries)) = self.handles.get_mut(&handle_id(id)?) else {
                    return Err(invalid_handle());
                };
                read_directory(entries)
            }
            Request::Remove(name) => {
                fs::remove_file(path(name))?;
                Ok(Reply::Status(Status::Ok))
            }
            Request::Mkdir(name, mut attrs) => {
                if attrs.size.is_some() {
                    return Err(ErrorKind::InvalidInput.into());
                }
                let mode = attrs.mode.take().unwrap_or(0o777) & 0o7777;
                let name = path(name);
                fs::DirBuilder::new().mode(mode).create(name)?;
                set_path_attributes(name, attrs)?;
                Ok(Reply::Status(Status::Ok))
            }
            Request::Rmdir(name) => {
                fs::remove_dir(path(name))?;
                Ok(Reply::Status(Status::Ok))
            }
            Request::Realpath(name) => {
                let path = path(name);
                fs::metadata(path)?;
                Ok(path_reply(&fs::canonicalize(path)?))
            }
            Request::Rename(old, new) => {
                rename_no_replace(path(old), path(new))?;
                Ok(Reply::Status(Status::Ok))
            }
            Request::Readlink(name) => Ok(path_reply(&fs::read_link(path(name))?)),
            Request::Symlink(first, second) => {
                let (target, link) = if self.target_first {
                    (first, second)
                } else {
                    (second, first)
                };
                unix::symlink(OsStr::from_bytes(target), path(link))?;
                Ok(Reply::Status(Status::Ok))
            }
            Request::Unsupported => Ok(Reply::Status(Status::Unsupported)),
        }
    }

    fn allocate(&mut self, open: impl FnOnce() -> io::Result<Handle>) -> io::Result<Reply> {
        if self.handles.len() == MAX_HANDLES {
            return Ok(Reply::Status(Status::Failure));
        }
        let id = self.next_handle;
        let next = id.checked_add(1).ok_or_else(invalid_handle)?;
        self.handles.insert(id, open()?);
        self.next_handle = next;
        Ok(Reply::Handle(id))
    }

    fn file(&self, id: &[u8]) -> io::Result<(&File, bool)> {
        match self.handles.get(&handle_id(id)?) {
            Some(Handle::File { file, append }) => Ok((file, *append)),
            _ => Err(invalid_handle()),
        }
    }
}

fn path(bytes: &[u8]) -> &Path {
    Path::new(OsStr::from_bytes(if bytes.is_empty() {
        b"."
    } else {
        bytes
    }))
}

fn path_reply(path: &Path) -> Reply {
    let bytes = path.as_os_str().as_bytes();
    Reply::Name(vec![Name {
        short: bytes.to_vec(),
        long: bytes.to_vec(),
        attrs: Attrs::default(),
    }])
}

fn handle_id(bytes: &[u8]) -> io::Result<u64> {
    bytes
        .try_into()
        .map(u64::from_be_bytes)
        .map_err(|_| invalid_handle())
}

fn invalid_handle() -> io::Error {
    io::Error::new(ErrorKind::InvalidInput, "invalid SFTP handle")
}

fn fs_status(error: &io::Error) -> Status {
    match error.kind() {
        ErrorKind::NotFound | ErrorKind::NotADirectory => Status::NoSuchFile,
        ErrorKind::PermissionDenied => Status::PermissionDenied,
        ErrorKind::Unsupported => Status::Unsupported,
        _ => Status::Failure,
    }
}

fn open(path: &Path, flags: u32, mut attrs: Attrs) -> io::Result<File> {
    let append = flags & APPEND != 0;
    let truncate = flags & TRUNCATE != 0;
    let mut options = OpenOptions::new();
    options
        .read(flags & READ != 0)
        .write(flags & WRITE != 0)
        .append(append);
    // A client opening a named pipe must not block the connection during open,
    // and opening a terminal must not acquire it as a controlling terminal.
    #[cfg(target_os = "breenix")]
    options.custom_flags(libbreenix::fs::O_NONBLOCK as i32 | libbreenix::pty::O_NOCTTY);

    if flags & CREATE != 0 {
        let mut create = options.clone();
        create
            .create_new(true)
            .mode(attrs.mode.take().unwrap_or(0o666) & 0o7777);
        match create.open(path) {
            Ok(file) => {
                set_file_attributes(&file, attrs)?;
                return Ok(file);
            }
            Err(error) if error.kind() == ErrorKind::AlreadyExists && flags & EXCLUSIVE == 0 => {}
            Err(error) => return Err(error),
        }
    }
    let file = options.truncate(truncate && !append).open(path)?;
    if !file.metadata()?.is_file() {
        return Err(ErrorKind::InvalidInput.into());
    }
    // OpenOptions rejects append and truncate together. Truncate the opened
    // inode before returning its handle and keep O_APPEND for later writes.
    if append && truncate {
        file.set_len(0)?;
    }
    Ok(file)
}

fn offset(value: u64) -> io::Result<i64> {
    i64::try_from(value).map_err(|_| ErrorKind::InvalidInput.into())
}

fn check_range(start: u64, length: usize) -> io::Result<()> {
    let end = start
        .checked_add(length as u64)
        .ok_or(ErrorKind::InvalidInput)?;
    offset(end)?;
    Ok(())
}

fn read_file(file: &File, start: u64, length: u32) -> io::Result<Reply> {
    let length = (length as usize).min(codec::MAX_READ);
    check_range(start, length)?;
    let mut data = vec![0; length];
    let mut filled = 0;
    while filled < length {
        match file.read_at(&mut data[filled..], start + filled as u64) {
            Ok(0) => break,
            Ok(count) => filled += count,
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
    if filled == 0 && length != 0 {
        return Ok(Reply::Status(Status::Eof));
    }
    data.truncate(filled);
    Ok(Reply::Data(data))
}

fn attributes(metadata: &Metadata) -> Attrs {
    let times = u32::try_from(metadata.atime())
        .ok()
        .zip(u32::try_from(metadata.mtime()).ok());
    Attrs {
        size: Some(metadata.size()),
        owner: Some((metadata.uid(), metadata.gid())),
        mode: Some(metadata.mode()),
        times,
    }
}

fn set_file_attributes(file: &File, attrs: Attrs) -> io::Result<()> {
    if let Some(size) = attrs.size {
        offset(size)?;
        file.set_len(size)?;
    }
    if let Some((uid, gid)) = attrs.owner {
        unix::fchown(file, Some(uid), Some(gid))?;
    }
    if let Some(mode) = attrs.mode {
        file.set_permissions(fs::Permissions::from_mode(mode & 0o7777))?;
    }
    if let Some((accessed, modified)) = attrs.times {
        file.set_times(
            FileTimes::new()
                .set_accessed(UNIX_EPOCH + Duration::from_secs(accessed.into()))
                .set_modified(UNIX_EPOCH + Duration::from_secs(modified.into())),
        )?;
    }
    Ok(())
}

fn set_path_attributes(path: &Path, attrs: Attrs) -> io::Result<()> {
    fs::metadata(path)?;
    let raw = CString::new(path.as_os_str().as_bytes()).map_err(|_| ErrorKind::InvalidInput)?;
    if let Some(size) = attrs.size {
        let length = offset(size)?;
        // SAFETY: raw is a valid C path for the duration of the syscall.
        syscall_result(unsafe { truncate(raw.as_ptr(), length) })?;
    }
    if let Some((uid, gid)) = attrs.owner {
        unix::chown(path, Some(uid), Some(gid))?;
    }
    if let Some(mode) = attrs.mode {
        fs::set_permissions(path, fs::Permissions::from_mode(mode & 0o7777))?;
    }
    if let Some((accessed, modified)) = attrs.times {
        let times = [i64::from(accessed), i64::from(modified)];
        // SAFETY: raw and the two time_t values live across the call.
        syscall_result(unsafe { utime(raw.as_ptr(), &times) })?;
    }
    Ok(())
}

fn rename_no_replace(old: &Path, new: &Path) -> io::Result<()> {
    let old = CString::new(old.as_os_str().as_bytes()).map_err(|_| ErrorKind::InvalidInput)?;
    let new = CString::new(new.as_os_str().as_bytes()).map_err(|_| ErrorKind::InvalidInput)?;
    rename_exclusive(&old, &new)
}

#[cfg(target_os = "breenix")]
fn rename_exclusive(old: &std::ffi::CStr, new: &std::ffi::CStr) -> io::Result<()> {
    use libbreenix::syscall::{nr, raw::syscall5};
    const AT_FDCWD: u64 = -100i64 as u64;
    const RENAME_NOREPLACE: u64 = 1;
    // SAFETY: both C strings are live and the syscall only reads them.
    let result = unsafe {
        syscall5(
            nr::RENAMEAT2,
            AT_FDCWD,
            old.as_ptr() as u64,
            AT_FDCWD,
            new.as_ptr() as u64,
            RENAME_NOREPLACE,
        )
    } as i64;
    if result < 0 {
        let errno = -result as i32;
        if errno == libbreenix::errno::Errno::ENOSYS as i32 {
            // An older kernel provides only replacing rename. Fail closed.
            return Err(ErrorKind::Unsupported.into());
        }
        return Err(io::Error::from_raw_os_error(errno));
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn rename_exclusive(old: &std::ffi::CStr, new: &std::ffi::CStr) -> io::Result<()> {
    extern "C" {
        fn renamex_np(old: *const c_char, new: *const c_char, flags: u32) -> i32;
    }
    const RENAME_EXCL: u32 = 4;
    // SAFETY: both C strings are live and renamex_np only reads them.
    syscall_result(unsafe { renamex_np(old.as_ptr(), new.as_ptr(), RENAME_EXCL) })
}

#[cfg(all(not(target_os = "breenix"), not(target_os = "macos")))]
fn rename_exclusive(_old: &std::ffi::CStr, _new: &std::ffi::CStr) -> io::Result<()> {
    Err(ErrorKind::Unsupported.into())
}

fn syscall_result(result: i32) -> io::Result<()> {
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn read_directory(entries: &mut ReadDir) -> io::Result<Reply> {
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry?;
        let metadata = match entry.metadata() {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        let name = entry.file_name().as_bytes().to_vec();
        let display = long_name(&name, &metadata);
        names.push(Name {
            short: name,
            long: display.into_bytes(),
            attrs: attributes(&metadata),
        });
        if names.len() == READDIR_BATCH {
            break;
        }
    }
    if names.is_empty() {
        Ok(Reply::Status(Status::Eof))
    } else {
        Ok(Reply::Name(names))
    }
}

fn long_name(name: &[u8], metadata: &Metadata) -> String {
    let mode = metadata.mode();
    let mut permissions = String::from(match mode & 0o170000 {
        0o040000 => "d",
        0o120000 => "l",
        0o020000 => "c",
        0o060000 => "b",
        0o010000 => "p",
        0o140000 => "s",
        _ => "-",
    });
    for (shift, special, label) in [(6, 0o4000, 's'), (3, 0o2000, 's'), (0, 0o1000, 't')] {
        permissions.push(if mode & (4 << shift) != 0 { 'r' } else { '-' });
        permissions.push(if mode & (2 << shift) != 0 { 'w' } else { '-' });
        let executable = mode & (1 << shift) != 0;
        permissions.push(match (mode & special != 0, executable) {
            (true, true) => label,
            (true, false) => label.to_ascii_uppercase(),
            (false, true) => 'x',
            (false, false) => '-',
        });
    }
    format!(
        "{permissions} {:>3} {:>8} {:>8} {:>10} {:>12} {}",
        metadata.nlink(),
        metadata.uid(),
        metadata.gid(),
        metadata.size(),
        metadata.mtime(),
        String::from_utf8_lossy(name).escape_debug()
    )
}

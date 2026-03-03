//! File descriptor types and table
//!
//! This module provides the unified file descriptor abstraction for POSIX-like I/O.
//! Each process has its own file descriptor table that maps small integers
//! to underlying file objects (pipes, stdio, sockets, etc.).
//!
//! Hardware-dependent FdKind variants are behind feature gates:
//! - `net`: UdpSocket, TcpSocket, TcpListener, TcpConnection
//! - `pty`: PtyMaster, PtySlave, DevptsDirectory
//! - `unix_socket`: UnixStream, UnixSocket, UnixListener
//! - `epoll`: Epoll

use alloc::sync::Arc;
use alloc::boxed::Box;
use alloc::string::String;
use spin::Mutex;
use super::pipe::PipeBuffer;
use crate::fs::devfs::DeviceType;

/// Maximum number of file descriptors per process
pub const MAX_FDS: usize = 256;

/// Standard file descriptor numbers
pub const STDIN: i32 = 0;
pub const STDOUT: i32 = 1;
pub const STDERR: i32 = 2;

/// File descriptor flags (for F_GETFD/F_SETFD)
pub mod flags {
    /// Close-on-exec flag (used by fcntl F_SETFD)
    pub const FD_CLOEXEC: u32 = 1;
}

/// File status flags (for F_GETFL/F_SETFL and open/pipe2)
pub mod status_flags {
    /// Non-blocking I/O mode
    pub const O_NONBLOCK: u32 = 0x800; // 2048
    /// Append mode (writes always append)
    pub const O_APPEND: u32 = 0x400; // 1024
    /// Close-on-exec (used in open/pipe2, but stored as FD_CLOEXEC)
    pub const O_CLOEXEC: u32 = 0x80000; // 524288
}

/// fcntl command constants
pub mod fcntl_cmd {
    /// Duplicate file descriptor
    pub const F_DUPFD: i32 = 0;
    /// Get file descriptor flags
    pub const F_GETFD: i32 = 1;
    /// Set file descriptor flags
    pub const F_SETFD: i32 = 2;
    /// Get file status flags
    pub const F_GETFL: i32 = 3;
    /// Set file status flags
    pub const F_SETFL: i32 = 4;
    /// Duplicate fd with close-on-exec set
    pub const F_DUPFD_CLOEXEC: i32 = 1030;
}

/// Regular file descriptor
#[derive(Clone, Debug)]
pub struct RegularFile {
    pub inode_num: u64,
    pub mount_id: usize,
    pub position: u64,
    pub flags: u32,
}

/// Directory file descriptor (for getdents)
#[derive(Clone, Debug)]
pub struct DirectoryFile {
    pub inode_num: u64,
    pub mount_id: usize,
    pub position: u64,  // Current offset in directory entries
}

/// Types of file descriptors
///
/// This unified enum supports all fd types in Breenix.
/// Hardware-specific variants are behind feature gates.
#[derive(Clone)]
pub enum FdKind {
    /// Standard I/O (stdin, stdout, stderr)
    StdIo(i32),
    /// Read end of a pipe
    PipeRead(Arc<Mutex<PipeBuffer>>),
    /// Write end of a pipe
    PipeWrite(Arc<Mutex<PipeBuffer>>),
    /// Regular file descriptor
    RegularFile(Arc<Mutex<RegularFile>>),
    /// Directory file descriptor (for getdents)
    Directory(Arc<Mutex<DirectoryFile>>),
    /// Device file (/dev/null, /dev/zero, /dev/console, /dev/tty)
    Device(DeviceType),
    /// /dev directory (virtual directory for listing devices)
    DevfsDirectory { position: u64 },
    /// FIFO (named pipe) read end - path is stored for cleanup on close
    FifoRead(String, Arc<Mutex<PipeBuffer>>),
    /// FIFO (named pipe) write end - path is stored for cleanup on close
    FifoWrite(String, Arc<Mutex<PipeBuffer>>),
    /// Procfs virtual file (content generated at open time)
    ProcfsFile { content: String, position: usize },
    /// Procfs directory listing (for /proc and /proc/[pid])
    ProcfsDirectory { path: String, position: u64 },
}

impl core::fmt::Debug for FdKind {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            FdKind::StdIo(n) => write!(f, "StdIo({})", n),
            FdKind::PipeRead(_) => write!(f, "PipeRead"),
            FdKind::PipeWrite(_) => write!(f, "PipeWrite"),
            FdKind::RegularFile(_) => write!(f, "RegularFile"),
            FdKind::Directory(_) => write!(f, "Directory"),
            FdKind::Device(dt) => write!(f, "Device({:?})", dt),
            FdKind::DevfsDirectory { position } => write!(f, "DevfsDirectory(pos={})", position),
            FdKind::FifoRead(path, _) => write!(f, "FifoRead({})", path),
            FdKind::FifoWrite(path, _) => write!(f, "FifoWrite({})", path),
            FdKind::ProcfsFile { content, position } => write!(f, "ProcfsFile(len={}, pos={})", content.len(), position),
            FdKind::ProcfsDirectory { path, position } => write!(f, "ProcfsDirectory(path={}, pos={})", path, position),
        }
    }
}

/// A file descriptor entry in the per-process table
#[derive(Clone)]
pub struct FileDescriptor {
    /// What kind of file this descriptor refers to
    pub kind: FdKind,
    /// File descriptor flags (FD_CLOEXEC) - per-fd, not inherited on dup
    pub flags: u32,
    /// File status flags (O_NONBLOCK, O_APPEND) - per-fd for pipes
    pub status_flags: u32,
}

impl core::fmt::Debug for FileDescriptor {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("FileDescriptor")
            .field("kind", &self.kind)
            .field("flags", &self.flags)
            .field("status_flags", &self.status_flags)
            .finish()
    }
}

impl FileDescriptor {
    /// Create a new file descriptor
    pub fn new(kind: FdKind) -> Self {
        FileDescriptor {
            kind,
            flags: 0,
            status_flags: 0,
        }
    }

    /// Create with specific flags (used by pipe2, etc.)
    pub fn with_flags(kind: FdKind, flags: u32, status_flags: u32) -> Self {
        FileDescriptor {
            kind,
            flags,
            status_flags,
        }
    }
}

/// Per-process file descriptor table
///
/// Uses Box to allocate the fd array on the heap (~6KB).
pub struct FdTable {
    /// The file descriptors (None = unused slot)
    fds: Box<[Option<FileDescriptor>; MAX_FDS]>,
}

impl Default for FdTable {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for FdTable {
    fn clone(&self) -> Self {
        let cloned_fds = self.fds.clone();

        // Increment reference counts for all cloned fds that need it
        for fd_opt in cloned_fds.iter() {
            if let Some(fd_entry) = fd_opt {
                match &fd_entry.kind {
                    FdKind::PipeRead(buffer) => buffer.lock().add_reader(),
                    FdKind::PipeWrite(buffer) => buffer.lock().add_writer(),
                    FdKind::FifoRead(_, buffer) => {
                        buffer.lock().add_reader();
                    }
                    FdKind::FifoWrite(_, buffer) => {
                        buffer.lock().add_writer();
                    }
                    _ => {}
                }
            }
        }

        FdTable { fds: cloned_fds }
    }
}

impl FdTable {
    /// Create a new file descriptor table with standard I/O pre-allocated
    pub fn new() -> Self {
        let mut fds = Box::new(core::array::from_fn(|_| None));

        // Pre-allocate stdin, stdout, stderr
        fds[STDIN as usize] = Some(FileDescriptor::new(FdKind::StdIo(STDIN)));
        fds[STDOUT as usize] = Some(FileDescriptor::new(FdKind::StdIo(STDOUT)));
        fds[STDERR as usize] = Some(FileDescriptor::new(FdKind::StdIo(STDERR)));

        FdTable { fds }
    }

    /// Take all file descriptor entries out of the table, leaving it empty.
    ///
    /// Returns a Vec of (fd_number, FileDescriptor) pairs for deferred cleanup.
    pub fn take_all(&mut self) -> alloc::vec::Vec<(usize, FileDescriptor)> {
        let mut entries = alloc::vec::Vec::new();
        for fd in 0..MAX_FDS {
            if let Some(entry) = self.fds[fd].take() {
                entries.push((fd, entry));
            }
        }
        entries
    }

    /// Allocate a new file descriptor with the given kind
    /// Returns the fd number on success, or an error code
    pub fn alloc(&mut self, kind: FdKind) -> Result<i32, i32> {
        self.alloc_at_least(0, kind)
    }

    /// Allocate a new file descriptor >= min_fd
    pub fn alloc_at_least(&mut self, min_fd: i32, kind: FdKind) -> Result<i32, i32> {
        let start = min_fd.max(0) as usize;
        for i in start..MAX_FDS {
            if self.fds[i].is_none() {
                self.fds[i] = Some(FileDescriptor::new(kind));
                return Ok(i as i32);
            }
        }
        Err(24) // EMFILE - too many open files
    }

    /// Allocate a new file descriptor with a pre-configured FileDescriptor entry
    /// This allows setting flags at allocation time (used by pipe2)
    pub fn alloc_with_entry(&mut self, entry: FileDescriptor) -> Result<i32, i32> {
        for i in 0..MAX_FDS {
            if self.fds[i].is_none() {
                self.fds[i] = Some(entry);
                return Ok(i as i32);
            }
        }
        Err(24) // EMFILE - too many open files
    }

    /// Get a reference to a file descriptor
    pub fn get(&self, fd: i32) -> Option<&FileDescriptor> {
        if fd < 0 || fd as usize >= MAX_FDS {
            return None;
        }
        self.fds[fd as usize].as_ref()
    }

    /// Get a mutable reference to a file descriptor (used by fcntl)
    pub fn get_mut(&mut self, fd: i32) -> Option<&mut FileDescriptor> {
        if fd < 0 || fd as usize >= MAX_FDS {
            return None;
        }
        self.fds[fd as usize].as_mut()
    }

    /// Close a file descriptor
    /// Returns the closed FileDescriptor on success, or an error code
    pub fn close(&mut self, fd: i32) -> Result<FileDescriptor, i32> {
        if fd < 0 || fd as usize >= MAX_FDS {
            return Err(9); // EBADF - bad file descriptor
        }
        self.fds[fd as usize].take().ok_or(9) // EBADF
    }

    /// Duplicate a file descriptor to a specific slot
    /// Used for dup2() syscall
    pub fn dup2(&mut self, old_fd: i32, new_fd: i32) -> Result<i32, i32> {
        if old_fd < 0 || old_fd as usize >= MAX_FDS {
            return Err(9); // EBADF
        }
        if new_fd < 0 || new_fd as usize >= MAX_FDS {
            return Err(9); // EBADF
        }

        // Per POSIX: if old_fd == new_fd, just verify old_fd is valid and return it
        if old_fd == new_fd {
            if self.fds[old_fd as usize].is_none() {
                return Err(9); // EBADF
            }
            return Ok(new_fd);
        }

        let fd_entry = self.fds[old_fd as usize].clone().ok_or(9)?;

        // If new_fd is open, close it and decrement ref counts
        if let Some(old_entry) = self.fds[new_fd as usize].take() {
            match old_entry.kind {
                FdKind::PipeRead(buffer) => buffer.lock().close_read(),
                FdKind::PipeWrite(buffer) => buffer.lock().close_write(),
                FdKind::FifoRead(_, ref buffer) => {
                    buffer.lock().close_read();
                }
                FdKind::FifoWrite(_, ref buffer) => {
                    buffer.lock().close_write();
                }
                _ => {}
            }
        }

        // Increment ref counts for the duplicated fd
        match &fd_entry.kind {
            FdKind::PipeRead(buffer) => buffer.lock().add_reader(),
            FdKind::PipeWrite(buffer) => buffer.lock().add_writer(),
            FdKind::FifoRead(_, buffer) => {
                buffer.lock().add_reader();
            }
            FdKind::FifoWrite(_, buffer) => {
                buffer.lock().add_writer();
            }
            _ => {}
        }

        self.fds[new_fd as usize] = Some(fd_entry);
        Ok(new_fd)
    }

    /// Duplicate a file descriptor to the lowest available slot
    /// Used for dup() syscall
    pub fn dup(&mut self, old_fd: i32) -> Result<i32, i32> {
        self.dup_at_least(old_fd, 0, false)
    }

    /// Duplicate a file descriptor to slot >= min_fd
    /// Used for fcntl F_DUPFD and F_DUPFD_CLOEXEC
    pub fn dup_at_least(&mut self, old_fd: i32, min_fd: i32, set_cloexec: bool) -> Result<i32, i32> {
        if old_fd < 0 || old_fd as usize >= MAX_FDS {
            return Err(9); // EBADF
        }
        if min_fd < 0 || min_fd as usize >= MAX_FDS {
            return Err(22); // EINVAL
        }

        let mut fd_entry = self.fds[old_fd as usize].clone().ok_or(9)?;

        // POSIX: dup and F_DUPFD clear FD_CLOEXEC, F_DUPFD_CLOEXEC sets it
        fd_entry.flags = if set_cloexec { flags::FD_CLOEXEC } else { 0 };

        // Increment reference counts for the duplicated fd
        match &fd_entry.kind {
            FdKind::PipeRead(buffer) => buffer.lock().add_reader(),
            FdKind::PipeWrite(buffer) => buffer.lock().add_writer(),
            FdKind::FifoRead(_, buffer) => {
                buffer.lock().add_reader();
            }
            FdKind::FifoWrite(_, buffer) => {
                buffer.lock().add_writer();
            }
            _ => {}
        }

        // Find lowest available slot >= min_fd
        for i in (min_fd as usize)..MAX_FDS {
            if self.fds[i].is_none() {
                self.fds[i] = Some(fd_entry);
                return Ok(i as i32);
            }
        }

        // No slot found - need to decrement the counts we just added
        match &fd_entry.kind {
            FdKind::PipeRead(buffer) => buffer.lock().close_read(),
            FdKind::PipeWrite(buffer) => buffer.lock().close_write(),
            FdKind::FifoRead(_, buffer) => {
                buffer.lock().close_read();
            }
            FdKind::FifoWrite(_, buffer) => {
                buffer.lock().close_write();
            }
            _ => {}
        }
        Err(24) // EMFILE
    }

    /// Get file descriptor flags (for F_GETFD)
    pub fn get_fd_flags(&self, fd: i32) -> Result<u32, i32> {
        self.get(fd).map(|e| e.flags).ok_or(9) // EBADF
    }

    /// Set file descriptor flags (for F_SETFD)
    pub fn set_fd_flags(&mut self, fd: i32, new_flags: u32) -> Result<(), i32> {
        self.get_mut(fd).map(|e| e.flags = new_flags).ok_or(9) // EBADF
    }

    /// Get file status flags (for F_GETFL)
    pub fn get_status_flags(&self, fd: i32) -> Result<u32, i32> {
        self.get(fd).map(|e| e.status_flags).ok_or(9) // EBADF
    }

    /// Count the number of open file descriptors
    pub fn open_fd_count(&self) -> usize {
        self.fds.iter().filter(|slot| slot.is_some()).count()
    }

    /// Close all file descriptors marked with FD_CLOEXEC.
    /// Called during exec() per POSIX semantics.
    pub fn close_cloexec(&mut self) {
        for i in 0..MAX_FDS {
            let should_close = self.fds[i]
                .as_ref()
                .map(|fd| (fd.flags & flags::FD_CLOEXEC) != 0)
                .unwrap_or(false);
            if should_close {
                if let Some(fd_entry) = self.fds[i].take() {
                    match &fd_entry.kind {
                        FdKind::PipeRead(buffer) | FdKind::FifoRead(_, buffer) => {
                            buffer.lock().close_read();
                        }
                        FdKind::PipeWrite(buffer) | FdKind::FifoWrite(_, buffer) => {
                            buffer.lock().close_write();
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    /// Set file status flags (for F_SETFL)
    /// Only modifies O_NONBLOCK and O_APPEND; other flags are ignored
    pub fn set_status_flags(&mut self, fd: i32, new_flags: u32) -> Result<(), i32> {
        let fd_entry = self.get_mut(fd).ok_or(9)?; // EBADF
        let settable = status_flags::O_NONBLOCK | status_flags::O_APPEND;
        fd_entry.status_flags = (fd_entry.status_flags & !settable) | (new_flags & settable);
        Ok(())
    }
}

/// Drop implementation for FdTable
///
/// When a process exits and its FdTable is dropped, we need to properly
/// decrement pipe reader/writer counts for any open pipe fds.
impl Drop for FdTable {
    fn drop(&mut self) {
        for i in 0..MAX_FDS {
            if let Some(fd_entry) = self.fds[i].take() {
                match fd_entry.kind {
                    FdKind::PipeRead(buffer) => {
                        buffer.lock().close_read();
                    }
                    FdKind::PipeWrite(buffer) => {
                        buffer.lock().close_write();
                    }
                    FdKind::FifoRead(_, buffer) => {
                        buffer.lock().close_read();
                    }
                    FdKind::FifoWrite(_, buffer) => {
                        buffer.lock().close_write();
                    }
                    _ => {}
                }
            }
        }
    }
}

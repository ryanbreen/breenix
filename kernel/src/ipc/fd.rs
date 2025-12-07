//! File descriptor types and table
//!
//! This module provides the file descriptor abstraction for POSIX-like I/O.
//! Each process has its own file descriptor table that maps small integers
//! to underlying file objects (pipes, stdio, etc.).

use alloc::sync::Arc;
use spin::Mutex;

/// Maximum number of file descriptors per process
pub const MAX_FDS: usize = 256;

/// Standard file descriptor numbers
pub const STDIN: i32 = 0;
pub const STDOUT: i32 = 1;
pub const STDERR: i32 = 2;

/// File descriptor flags
pub mod flags {
    /// Close-on-exec flag (used by fcntl F_SETFD)
    #[allow(dead_code)]
    pub const FD_CLOEXEC: u32 = 1;
}

/// Types of file descriptors
#[derive(Clone)]
pub enum FdKind {
    /// Standard I/O (stdin, stdout, stderr)
    StdIo(i32),
    /// Read end of a pipe
    PipeRead(Arc<Mutex<super::pipe::PipeBuffer>>),
    /// Write end of a pipe
    PipeWrite(Arc<Mutex<super::pipe::PipeBuffer>>),
}

impl core::fmt::Debug for FdKind {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            FdKind::StdIo(n) => write!(f, "StdIo({})", n),
            FdKind::PipeRead(_) => write!(f, "PipeRead"),
            FdKind::PipeWrite(_) => write!(f, "PipeWrite"),
        }
    }
}

/// A file descriptor entry in the per-process table
#[derive(Clone)]
pub struct FileDescriptor {
    /// What kind of file this descriptor refers to
    pub kind: FdKind,
    /// Flags (FD_CLOEXEC, etc.)
    pub flags: u32,
}

impl core::fmt::Debug for FileDescriptor {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("FileDescriptor")
            .field("kind", &self.kind)
            .field("flags", &self.flags)
            .finish()
    }
}

impl FileDescriptor {
    /// Create a new file descriptor
    pub fn new(kind: FdKind) -> Self {
        FileDescriptor { kind, flags: 0 }
    }

    /// Create with specific flags (used by pipe2, etc.)
    #[allow(dead_code)]
    pub fn with_flags(kind: FdKind, flags: u32) -> Self {
        FileDescriptor { kind, flags }
    }
}

/// Per-process file descriptor table
///
/// Note: Uses Box to heap-allocate the fd array to avoid stack overflow
/// during process creation (the array is ~6KB which is too large for stack).
pub struct FdTable {
    /// The file descriptors (None = unused slot)
    fds: alloc::boxed::Box<[Option<FileDescriptor>; MAX_FDS]>,
}

impl Default for FdTable {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for FdTable {
    fn clone(&self) -> Self {
        FdTable {
            fds: alloc::boxed::Box::new((*self.fds).clone()),
        }
    }
}

impl FdTable {
    /// Create a new file descriptor table with standard I/O pre-allocated
    pub fn new() -> Self {
        // Use Box::new to allocate directly on heap, avoiding stack overflow
        let mut fds = alloc::boxed::Box::new(core::array::from_fn(|_| None));

        // Pre-allocate stdin, stdout, stderr
        fds[STDIN as usize] = Some(FileDescriptor::new(FdKind::StdIo(STDIN)));
        fds[STDOUT as usize] = Some(FileDescriptor::new(FdKind::StdIo(STDOUT)));
        fds[STDERR as usize] = Some(FileDescriptor::new(FdKind::StdIo(STDERR)));

        FdTable { fds }
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

    /// Get a reference to a file descriptor
    pub fn get(&self, fd: i32) -> Option<&FileDescriptor> {
        if fd < 0 || fd as usize >= MAX_FDS {
            return None;
        }
        self.fds[fd as usize].as_ref()
    }

    /// Get a mutable reference to a file descriptor (used by fcntl)
    #[allow(dead_code)]
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
    #[allow(dead_code)]
    pub fn dup2(&mut self, old_fd: i32, new_fd: i32) -> Result<i32, i32> {
        if old_fd < 0 || old_fd as usize >= MAX_FDS {
            return Err(9); // EBADF
        }
        if new_fd < 0 || new_fd as usize >= MAX_FDS {
            return Err(9); // EBADF
        }

        let fd_entry = self.fds[old_fd as usize].clone().ok_or(9)?;

        // Close new_fd if it's open (silently ignore errors)
        let _ = self.close(new_fd);

        self.fds[new_fd as usize] = Some(fd_entry);
        Ok(new_fd)
    }

    /// Duplicate a file descriptor to the lowest available slot
    /// Used for dup() syscall
    #[allow(dead_code)]
    pub fn dup(&mut self, old_fd: i32) -> Result<i32, i32> {
        if old_fd < 0 || old_fd as usize >= MAX_FDS {
            return Err(9); // EBADF
        }

        let fd_entry = self.fds[old_fd as usize].clone().ok_or(9)?;

        // Find lowest available slot
        for i in 0..MAX_FDS {
            if self.fds[i].is_none() {
                self.fds[i] = Some(fd_entry);
                return Ok(i as i32);
            }
        }
        Err(24) // EMFILE
    }
}

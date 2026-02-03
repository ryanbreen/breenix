//! File descriptor types and table
//!
//! This module provides the unified file descriptor abstraction for POSIX-like I/O.
//! Each process has its own file descriptor table that maps small integers
//! to underlying file objects (pipes, stdio, sockets, etc.).

use alloc::sync::Arc;
use spin::Mutex;

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
#[allow(dead_code)] // Fields will be used when open/read/write are fully implemented
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
/// This unified enum supports all fd types in Breenix:
/// - Standard I/O (stdin/stdout/stderr)
/// - Pipes (read and write ends)
/// - UDP sockets (with future support for TCP, files, etc.)
/// - Regular files (filesystem files)
/// - Device files (/dev/null, /dev/zero, etc.)
///
/// Note: Sockets use Arc<Mutex<>> like pipes because they need to be shared
/// and cannot be cloned (they contain unique socket handles and rx queues).
#[derive(Clone)]
pub enum FdKind {
    /// Standard I/O (stdin, stdout, stderr)
    StdIo(i32),
    /// Read end of a pipe
    PipeRead(Arc<Mutex<super::pipe::PipeBuffer>>),
    /// Write end of a pipe
    PipeWrite(Arc<Mutex<super::pipe::PipeBuffer>>),
    /// UDP socket (wrapped in Arc<Mutex<>> for sharing and dup/fork)
    /// Available on both x86_64 and ARM64 (driver abstraction handles hardware differences)
    UdpSocket(Arc<Mutex<crate::socket::udp::UdpSocket>>),
    /// TCP socket (unbound, or bound but not connected/listening)
    /// The u16 is the bound local port (0 if unbound)
    TcpSocket(u16),
    /// TCP listener (bound and listening socket)
    /// The u16 is the listening port
    TcpListener(u16),
    /// TCP connection (established connection)
    /// Contains the connection ID for lookup in the global TCP connection table
    TcpConnection(crate::net::tcp::ConnectionId),
    /// Regular file descriptor
    #[allow(dead_code)] // Will be constructed when open() is fully implemented
    RegularFile(Arc<Mutex<RegularFile>>),
    /// Directory file descriptor (for getdents)
    Directory(Arc<Mutex<DirectoryFile>>),
    /// Device file (/dev/null, /dev/zero, /dev/console, /dev/tty)
    Device(crate::fs::devfs::DeviceType),
    /// /dev directory (virtual directory for listing devices)
    DevfsDirectory { position: u64 },
    /// /dev/pts directory (virtual directory for listing PTY slaves)
    DevptsDirectory { position: u64 },
    /// PTY master file descriptor
    /// Allow unused - constructed by posix_openpt syscall in Phase 2
    #[allow(dead_code)]
    PtyMaster(u32),
    /// PTY slave file descriptor
    /// Allow unused - constructed when opening /dev/pts/N in Phase 2
    #[allow(dead_code)]
    PtySlave(u32),
    /// Unix stream socket (AF_UNIX, SOCK_STREAM) - for socketpair IPC
    /// Fully architecture-independent - uses in-memory buffers
    UnixStream(alloc::sync::Arc<spin::Mutex<crate::socket::unix::UnixStreamSocket>>),
    /// Unix socket (AF_UNIX, SOCK_STREAM) - unbound or bound but not connected/listening
    /// Fully architecture-independent
    UnixSocket(alloc::sync::Arc<spin::Mutex<crate::socket::unix::UnixSocket>>),
    /// Unix listener socket (AF_UNIX, SOCK_STREAM) - listening for connections
    /// Fully architecture-independent
    UnixListener(alloc::sync::Arc<spin::Mutex<crate::socket::unix::UnixListener>>),
    /// FIFO (named pipe) read end - path is stored for cleanup on close
    FifoRead(alloc::string::String, Arc<Mutex<super::pipe::PipeBuffer>>),
    /// FIFO (named pipe) write end - path is stored for cleanup on close
    FifoWrite(alloc::string::String, Arc<Mutex<super::pipe::PipeBuffer>>),
}

impl core::fmt::Debug for FdKind {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            FdKind::StdIo(n) => write!(f, "StdIo({})", n),
            FdKind::PipeRead(_) => write!(f, "PipeRead"),
            FdKind::PipeWrite(_) => write!(f, "PipeWrite"),
            FdKind::UdpSocket(_) => write!(f, "UdpSocket"),
            FdKind::TcpSocket(port) => write!(f, "TcpSocket(port={})", port),
            FdKind::TcpListener(port) => write!(f, "TcpListener(port={})", port),
            FdKind::TcpConnection(id) => write!(f, "TcpConnection({:?})", id),
            FdKind::RegularFile(_) => write!(f, "RegularFile"),
            FdKind::Directory(_) => write!(f, "Directory"),
            FdKind::Device(dt) => write!(f, "Device({:?})", dt),
            FdKind::DevfsDirectory { position } => write!(f, "DevfsDirectory(pos={})", position),
            FdKind::DevptsDirectory { position } => write!(f, "DevptsDirectory(pos={})", position),
            FdKind::PtyMaster(n) => write!(f, "PtyMaster({})", n),
            FdKind::PtySlave(n) => write!(f, "PtySlave({})", n),
            FdKind::UnixStream(s) => {
                let sock = s.lock();
                write!(f, "UnixStream({:?})", sock.endpoint)
            }
            FdKind::UnixSocket(s) => {
                let sock = s.lock();
                write!(f, "UnixSocket({:?})", sock.state)
            }
            FdKind::UnixListener(l) => {
                let listener = l.lock();
                write!(f, "UnixListener(pending={})", listener.pending_count())
            }
            FdKind::FifoRead(path, _) => write!(f, "FifoRead({})", path),
            FdKind::FifoWrite(path, _) => write!(f, "FifoWrite({})", path),
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
        // CRITICAL: No logging here - this runs during fork() with potential timer interrupts
        // Logging can cause deadlock if timer fires while holding logger lock
        let cloned_fds = alloc::boxed::Box::new((*self.fds).clone());

        // Increment reference counts for all cloned fds that need it
        for fd_opt in cloned_fds.iter() {
            if let Some(fd_entry) = fd_opt {
                match &fd_entry.kind {
                    FdKind::PipeRead(buffer) => buffer.lock().add_reader(),
                    FdKind::PipeWrite(buffer) => buffer.lock().add_writer(),
                    FdKind::FifoRead(path, buffer) => {
                        // Increment both FIFO entry reader count and pipe buffer reader count
                        if let Some(entry) = super::fifo::FIFO_REGISTRY.get(path) {
                            entry.lock().readers += 1;
                        }
                        buffer.lock().add_reader();
                    }
                    FdKind::FifoWrite(path, buffer) => {
                        // Increment both FIFO entry writer count and pipe buffer writer count
                        if let Some(entry) = super::fifo::FIFO_REGISTRY.get(path) {
                            entry.lock().writers += 1;
                        }
                        buffer.lock().add_writer();
                    }
                    FdKind::PtyMaster(pty_num) => {
                        // Increment PTY master reference count for the clone
                        // No logging - this runs during fork()
                        if let Some(pair) = crate::tty::pty::get(*pty_num) {
                            pair.master_refcount.fetch_add(1, core::sync::atomic::Ordering::SeqCst);
                        }
                    }
                    FdKind::TcpConnection(conn_id) => {
                        // Increment TCP connection reference count for the clone
                        crate::net::tcp::tcp_add_ref(conn_id);
                    }
                    FdKind::TcpListener(port) => {
                        // Increment TCP listener reference count for the clone
                        crate::net::tcp::tcp_listener_ref_inc(*port);
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

        // Per POSIX: if old_fd == new_fd, just verify old_fd is valid and return it
        // This avoids a race condition where close_read/close_write followed by
        // add_reader/add_writer would temporarily set the count to zero
        if old_fd == new_fd {
            // Verify old_fd is valid
            if self.fds[old_fd as usize].is_none() {
                return Err(9); // EBADF
            }
            return Ok(new_fd);
        }

        let fd_entry = self.fds[old_fd as usize].clone().ok_or(9)?;

        // If new_fd is open, close it and decrement pipe ref counts
        if let Some(old_entry) = self.fds[new_fd as usize].take() {
            match old_entry.kind {
                FdKind::PipeRead(buffer) => buffer.lock().close_read(),
                FdKind::PipeWrite(buffer) => buffer.lock().close_write(),
                FdKind::FifoRead(ref path, ref buffer) => {
                    super::fifo::close_fifo_read(path);
                    buffer.lock().close_read();
                }
                FdKind::FifoWrite(ref path, ref buffer) => {
                    super::fifo::close_fifo_write(path);
                    buffer.lock().close_write();
                }
                _ => {}
            }
        }

        // Increment pipe/FIFO reference counts for the duplicated fd
        match &fd_entry.kind {
            FdKind::PipeRead(buffer) => buffer.lock().add_reader(),
            FdKind::PipeWrite(buffer) => buffer.lock().add_writer(),
            FdKind::FifoRead(path, buffer) => {
                if let Some(entry) = super::fifo::FIFO_REGISTRY.get(path) {
                    entry.lock().readers += 1;
                }
                buffer.lock().add_reader();
            }
            FdKind::FifoWrite(path, buffer) => {
                if let Some(entry) = super::fifo::FIFO_REGISTRY.get(path) {
                    entry.lock().writers += 1;
                }
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
    /// Note: POSIX says dup/F_DUPFD clear FD_CLOEXEC on the new fd
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

        // Increment pipe/FIFO reference counts for the duplicated fd
        match &fd_entry.kind {
            FdKind::PipeRead(buffer) => buffer.lock().add_reader(),
            FdKind::PipeWrite(buffer) => buffer.lock().add_writer(),
            FdKind::FifoRead(path, buffer) => {
                if let Some(entry) = super::fifo::FIFO_REGISTRY.get(path) {
                    entry.lock().readers += 1;
                }
                buffer.lock().add_reader();
            }
            FdKind::FifoWrite(path, buffer) => {
                if let Some(entry) = super::fifo::FIFO_REGISTRY.get(path) {
                    entry.lock().writers += 1;
                }
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
            FdKind::FifoRead(path, buffer) => {
                super::fifo::close_fifo_read(path);
                buffer.lock().close_read();
            }
            FdKind::FifoWrite(path, buffer) => {
                super::fifo::close_fifo_write(path);
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
    pub fn set_fd_flags(&mut self, fd: i32, flags: u32) -> Result<(), i32> {
        self.get_mut(fd).map(|e| e.flags = flags).ok_or(9) // EBADF
    }

    /// Get file status flags (for F_GETFL)
    pub fn get_status_flags(&self, fd: i32) -> Result<u32, i32> {
        self.get(fd).map(|e| e.status_flags).ok_or(9) // EBADF
    }

    /// Set file status flags (for F_SETFL)
    /// Only modifies O_NONBLOCK and O_APPEND; other flags are ignored
    pub fn set_status_flags(&mut self, fd: i32, flags: u32) -> Result<(), i32> {
        let fd_entry = self.get_mut(fd).ok_or(9)?; // EBADF
        // Only allow setting O_NONBLOCK and O_APPEND via F_SETFL
        let settable = status_flags::O_NONBLOCK | status_flags::O_APPEND;
        fd_entry.status_flags = (fd_entry.status_flags & !settable) | (flags & settable);
        Ok(())
    }
}

/// Drop implementation for FdTable
///
/// When a process exits and its FdTable is dropped, we need to properly
/// decrement pipe reader/writer counts for any open pipe fds. This ensures
/// that when all writers close, readers get EOF instead of EAGAIN.
///
/// Note: UdpSocket cleanup is handled by UdpSocket's own Drop impl when the
/// Arc reference count goes to zero.
impl Drop for FdTable {
    fn drop(&mut self) {
        log::debug!("FdTable::drop() - closing all fds and decrementing pipe counts");
        for i in 0..MAX_FDS {
            if let Some(fd_entry) = self.fds[i].take() {
                match fd_entry.kind {
                    FdKind::PipeRead(buffer) => {
                        buffer.lock().close_read();
                        log::debug!("FdTable::drop() - closed pipe read fd {}", i);
                    }
                    FdKind::PipeWrite(buffer) => {
                        buffer.lock().close_write();
                        log::debug!("FdTable::drop() - closed pipe write fd {}", i);
                    }
                    FdKind::UdpSocket(_) => {
                        // Socket cleanup handled by UdpSocket::Drop when Arc refcount reaches 0
                        log::debug!("FdTable::drop() - releasing UDP socket fd {}", i);
                    }
                    FdKind::TcpSocket(_) => {
                        // Unbound TCP socket doesn't need cleanup
                        log::debug!("FdTable::drop() - releasing TCP socket fd {}", i);
                    }
                    FdKind::TcpListener(port) => {
                        // Decrement ref count, remove only if it reaches 0
                        crate::net::tcp::tcp_listener_ref_dec(port);
                        log::debug!("FdTable::drop() - released TCP listener fd {} on port {}", i, port);
                    }
                    FdKind::TcpConnection(conn_id) => {
                        // Close the TCP connection
                        let _ = crate::net::tcp::tcp_close(&conn_id);
                        log::debug!("FdTable::drop() - closed TCP connection fd {}", i);
                    }
                    FdKind::StdIo(_) => {
                        // StdIo doesn't need cleanup
                    }
                    FdKind::RegularFile(_) => {
                        // Regular file cleanup handled by Arc refcount
                        log::debug!("FdTable::drop() - releasing regular file fd {}", i);
                    }
                    FdKind::Directory(_) => {
                        // Directory cleanup handled by Arc refcount
                        log::debug!("FdTable::drop() - releasing directory fd {}", i);
                    }
                    FdKind::Device(_) => {
                        // Device files don't need cleanup
                        log::debug!("FdTable::drop() - releasing device fd {}", i);
                    }
                    FdKind::DevfsDirectory { .. } => {
                        // Devfs directory doesn't need cleanup
                        log::debug!("FdTable::drop() - releasing devfs directory fd {}", i);
                    }
                    FdKind::DevptsDirectory { .. } => {
                        // Devpts directory doesn't need cleanup
                        log::debug!("FdTable::drop() - releasing devpts directory fd {}", i);
                    }
                    FdKind::PtyMaster(pty_num) => {
                        // PTY master cleanup - decrement refcount, only release when all masters closed
                        if let Some(pair) = crate::tty::pty::get(pty_num) {
                            let old_count = pair.master_refcount.fetch_sub(1, core::sync::atomic::Ordering::SeqCst);
                            log::debug!("FdTable::drop() - PTY master fd {} (pty {}) refcount {} -> {}",
                                i, pty_num, old_count, old_count - 1);
                            if old_count == 1 {
                                crate::tty::pty::release(pty_num);
                                log::debug!("FdTable::drop() - released PTY {} (last master closed)", pty_num);
                            }
                        }
                    }
                    FdKind::PtySlave(_pty_num) => {
                        // PTY slave doesn't own the pair, just decrement reference
                        log::debug!("FdTable::drop() - released PTY slave fd {}", i);
                    }
                    FdKind::UnixStream(socket) => {
                        // Close the Unix socket endpoint
                        socket.lock().close();
                        log::debug!("FdTable::drop() - closed Unix stream socket fd {}", i);
                    }
                    FdKind::UnixSocket(socket) => {
                        // Unbind from registry if bound
                        let sock = socket.lock();
                        if let Some(path) = &sock.bound_path {
                            crate::socket::UNIX_SOCKET_REGISTRY.unbind(path);
                            log::debug!("FdTable::drop() - unbound Unix socket fd {} from path", i);
                        }
                        log::debug!("FdTable::drop() - closed Unix socket fd {}", i);
                    }
                    FdKind::UnixListener(listener) => {
                        // Unbind from registry and wake any pending accept waiters
                        let l = listener.lock();
                        crate::socket::UNIX_SOCKET_REGISTRY.unbind(&l.path);
                        l.wake_waiters();
                        log::debug!("FdTable::drop() - closed Unix listener fd {}", i);
                    }
                    FdKind::FifoRead(path, buffer) => {
                        // Decrement FIFO reader count and pipe buffer reader count
                        super::fifo::close_fifo_read(&path);
                        buffer.lock().close_read();
                        log::debug!("FdTable::drop() - closed FIFO read fd {} ({})", i, path);
                    }
                    FdKind::FifoWrite(path, buffer) => {
                        // Decrement FIFO writer count and pipe buffer writer count
                        super::fifo::close_fifo_write(&path);
                        buffer.lock().close_write();
                        log::debug!("FdTable::drop() - closed FIFO write fd {} ({})", i, path);
                    }
                }
            }
        }
    }
}

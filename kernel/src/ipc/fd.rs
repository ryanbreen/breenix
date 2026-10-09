//! File descriptor types and table
//!
//! This module provides the unified file descriptor abstraction for POSIX-like I/O.
//! Each process has its own file descriptor table that maps small integers
//! to underlying file objects (pipes, stdio, sockets, etc.).

use crate::memory::slab::{SlabBox, FD_TABLE_SLAB};
use alloc::sync::Arc;
use core::sync::atomic::{AtomicU32, Ordering};
use spin::Mutex;

/// Maximum number of file descriptors per process
pub const MAX_FDS: usize = 4096;
/// Slab-backed initial capacity; larger descriptor numbers grow on allocation.
pub const INITIAL_FDS: usize = 256;

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
    /// Access mode bits of an open(2) flags word
    pub const O_ACCMODE: u32 = 0x3;
    /// Open for reading only
    pub const O_RDONLY: u32 = 0x0;
    /// Open for writing only
    pub const O_WRONLY: u32 = 0x1;
    /// Open for reading and writing
    pub const O_RDWR: u32 = 0x2;
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
    /// Report the first record lock that would block the described one
    pub const F_GETLK: i32 = 5;
    /// Set or clear a record lock, failing at once on a conflict
    pub const F_SETLK: i32 = 6;
    /// Set or clear a record lock, waiting out a conflict
    pub const F_SETLKW: i32 = 7;
    /// Duplicate fd with close-on-exec set
    pub const F_DUPFD_CLOEXEC: i32 = 1030;
}

/// Regular file descriptor
///
/// The access mode and status flags (O_APPEND) of the open file description
/// live on the `FileDescriptor`'s shared description word, not here.
#[derive(Clone, Debug)]
pub struct RegularFile {
    pub handle: crate::fs::ext2::live_inode::FileHandle,
    pub inode_num: u64,
    pub mount_id: usize,
    pub position: u64,
}

/// Directory file descriptor (for getdents)
#[derive(Clone, Debug)]
pub struct DirectoryFile {
    pub handle: crate::fs::ext2::live_inode::FileHandle,
    pub inode_num: u64,
    pub mount_id: usize,
    pub position: u64, // getdents64 cookie: byte offset of the next record
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
    FifoRead(alloc::string::String, Arc<Mutex<super::pipe::PipeBuffer>>, Arc<Mutex<super::fifo::FifoEntry>>),
    /// FIFO (named pipe) write end - path is stored for cleanup on close
    FifoWrite(alloc::string::String, Arc<Mutex<super::pipe::PipeBuffer>>, Arc<Mutex<super::fifo::FifoEntry>>),
    /// Procfs virtual file (content generated at open time)
    ProcfsFile {
        content: alloc::string::String,
        position: usize,
    },
    /// Procfs directory listing (for /proc and /proc/[pid])
    ProcfsDirectory {
        path: alloc::string::String,
        position: u64,
    },
    /// Epoll instance file descriptor
    Epoll(u64),
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
            FdKind::FifoRead(path, _, _) => write!(f, "FifoRead({})", path),
            FdKind::FifoWrite(path, _, _) => write!(f, "FifoWrite({})", path),
            FdKind::ProcfsFile { content, position } => {
                write!(f, "ProcfsFile(len={}, pos={})", content.len(), position)
            }
            FdKind::ProcfsDirectory { path, position } => {
                write!(f, "ProcfsDirectory(path={}, pos={})", path, position)
            }
            FdKind::Epoll(id) => write!(f, "Epoll({})", id),
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
    /// The open file description's flags word: the access mode (O_ACCMODE
    /// bits) and the file status flags (O_APPEND, O_NONBLOCK). Cloning the
    /// entry (dup, dup2, dup3, F_DUPFD, fork) shares the word, so F_SETFL on
    /// one descriptor is seen through every descriptor for the description.
    description: Arc<AtomicU32>,
}

impl core::fmt::Debug for FileDescriptor {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("FileDescriptor")
            .field("kind", &self.kind)
            .field("flags", &self.flags)
            .field("status_flags", &self.status_flags())
            .finish()
    }
}

/// Status flags F_SETFL may change. The access mode is fixed at open.
const SETTABLE_STATUS_FLAGS: u32 = status_flags::O_APPEND | status_flags::O_NONBLOCK;

/// Access mode of a descriptor that is not created by open(2) from a caller's
/// flags word: pipe and FIFO ends and the stdio streams are one-way (stdin is
/// read-only, stdout and stderr write-only, as read and write enforce),
/// listings are read-only, and the rest (sockets, PTYs, epoll) are read-write.
fn inherent_access_mode(kind: &FdKind) -> u32 {
    match kind {
        FdKind::StdIo(STDIN)
        | FdKind::PipeRead(_)
        | FdKind::FifoRead(_, _, _)
        | FdKind::Directory(_)
        | FdKind::DevfsDirectory { .. }
        | FdKind::DevptsDirectory { .. }
        | FdKind::ProcfsFile { .. }
        | FdKind::ProcfsDirectory { .. } => status_flags::O_RDONLY,
        FdKind::StdIo(_) | FdKind::PipeWrite(_) | FdKind::FifoWrite(_, _, _) => status_flags::O_WRONLY,
        _ => status_flags::O_RDWR,
    }
}

impl FileDescriptor {
    /// Create a new file descriptor with its kind's inherent access mode
    pub fn new(kind: FdKind) -> Self {
        Self::with_flags(kind, 0, 0)
    }

    /// Create with specific flags (used by pipe2, etc.). The access mode is
    /// the kind's inherent one; `status_flags` supplies O_APPEND/O_NONBLOCK.
    pub fn with_flags(kind: FdKind, flags: u32, status_flags: u32) -> Self {
        let word = inherent_access_mode(&kind) | (status_flags & SETTABLE_STATUS_FLAGS);
        FileDescriptor {
            kind,
            flags,
            description: Arc::new(AtomicU32::new(word)),
        }
    }

    /// Create the descriptor for a new open file description from an open(2)
    /// flags word: its access mode and status flags, and FD_CLOEXEC from
    /// O_CLOEXEC.
    pub fn opened(kind: FdKind, open_flags: u32) -> Self {
        let fd_flags = if open_flags & status_flags::O_CLOEXEC != 0 {
            flags::FD_CLOEXEC
        } else {
            0
        };
        let word = (open_flags & status_flags::O_ACCMODE) | (open_flags & SETTABLE_STATUS_FLAGS);
        FileDescriptor {
            kind,
            flags: fd_flags,
            description: Arc::new(AtomicU32::new(word)),
        }
    }

    /// The open file description's flags word (F_GETFL): access mode and
    /// status flags.
    pub fn status_flags(&self) -> u32 {
        self.description.load(Ordering::Acquire)
    }

    /// Whether the open file description was opened for reading.
    pub fn readable(&self) -> bool {
        matches!(
            self.status_flags() & status_flags::O_ACCMODE,
            status_flags::O_RDONLY | status_flags::O_RDWR
        )
    }

    /// Whether the open file description was opened for writing.
    pub fn writable(&self) -> bool {
        matches!(
            self.status_flags() & status_flags::O_ACCMODE,
            status_flags::O_WRONLY | status_flags::O_RDWR
        )
    }

    /// F_SETFL: replace the settable status flags (O_APPEND, O_NONBLOCK) of the
    /// open file description. The access mode and any other bit in `flags` are
    /// ignored.
    pub fn set_status_flags(&self, flags: u32) {
        let _ = self
            .description
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |word| {
                Some((word & !SETTABLE_STATUS_FLAGS) | (flags & SETTABLE_STATUS_FLAGS))
            });
    }
}

/// Per-process file descriptor table
///
/// Note: Uses SlabBox to allocate the fd array from a slab cache (O(1) alloc/free)
/// with fallback to the global heap. The array is ~6KB which is too large for stack.
#[derive(Clone)]
enum FdSlots {
    Slab(SlabBox<[Option<FileDescriptor>; INITIAL_FDS]>),
    Heap(alloc::vec::Vec<Option<FileDescriptor>>),
}

impl core::ops::Deref for FdSlots {
    type Target = [Option<FileDescriptor>];
    fn deref(&self) -> &Self::Target {
        match self {
            Self::Slab(slots) => &**slots,
            Self::Heap(slots) => slots,
        }
    }
}
impl core::ops::DerefMut for FdSlots {
    fn deref_mut(&mut self) -> &mut Self::Target {
        match self {
            Self::Slab(slots) => &mut **slots,
            Self::Heap(slots) => slots,
        }
    }
}
impl FdSlots {
    fn grow(&mut self, length: usize) -> Result<(), i32> {
        if length <= self.len() {
            return Ok(());
        }
        match self {
            Self::Heap(slots) => {
                slots.try_reserve(length - slots.len()).map_err(|_| 12)?;
                slots.resize_with(length, || None);
            }
            Self::Slab(slots) => {
                let mut larger = alloc::vec::Vec::new();
                larger.try_reserve_exact(length).map_err(|_| 12)?;
                for slot in slots.iter_mut() {
                    larger.push(slot.take());
                }
                larger.resize_with(length, || None);
                *self = Self::Heap(larger);
            }
        }
        Ok(())
    }
}

pub struct FdTable {
    /// The file descriptors (None = unused slot)
    fds: FdSlots,
    allocation_limit: usize,
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
        let cloned_fds = self.fds.clone();

        // Increment reference counts for all cloned fds that need it
        for fd_opt in cloned_fds.iter() {
            if let Some(fd_entry) = fd_opt {
                match &fd_entry.kind {
                    FdKind::PipeRead(buffer) => buffer.lock().add_reader(),
                    FdKind::PipeWrite(buffer) => buffer.lock().add_writer(),
                    FdKind::FifoRead(_, buffer, entry) => {
                        // Increment both FIFO entry reader count and pipe buffer reader count
                        entry.lock().readers += 1;
                        buffer.lock().add_reader();
                    }
                    FdKind::FifoWrite(_, buffer, entry) => {
                        // Increment both FIFO entry writer count and pipe buffer writer count
                        entry.lock().writers += 1;
                        buffer.lock().add_writer();
                    }
                    FdKind::PtyMaster(pty_num) => {
                        // Increment PTY master reference count for the clone
                        // No logging - this runs during fork()
                        if let Some(pair) = crate::tty::pty::get(*pty_num) {
                            pair.master_refcount
                                .fetch_add(1, core::sync::atomic::Ordering::SeqCst);
                        }
                    }
                    FdKind::PtySlave(pty_num) => {
                        // Increment PTY slave reference count for the clone
                        if let Some(pair) = crate::tty::pty::get(*pty_num) {
                            pair.slave_open();
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

        FdTable {
            fds: cloned_fds,
            allocation_limit: self.allocation_limit,
        }
    }
}

impl FdTable {
    /// Lowering a limit leaves already open descriptors usable.
    pub fn set_limit(&mut self, limit: u64) {
        self.allocation_limit = limit.min(MAX_FDS as u64) as usize;
    }

    /// Create a new file descriptor table with standard I/O pre-allocated
    pub fn new() -> Self {
        // Try slab allocation first (O(1)), fall back to global heap
        let mut fds = if let Some(raw) = FD_TABLE_SLAB.alloc() {
            // Slab returns zeroed memory; write None into each slot
            let arr = raw as *mut [Option<FileDescriptor>; INITIAL_FDS];
            for i in 0..INITIAL_FDS {
                unsafe {
                    core::ptr::write(&mut (*arr)[i], None);
                }
            }
            unsafe { SlabBox::from_slab(arr, &FD_TABLE_SLAB) }
        } else {
            SlabBox::from_box(alloc::boxed::Box::new(core::array::from_fn(|_| None)))
        };

        // Pre-allocate stdin, stdout, stderr
        fds[STDIN as usize] = Some(FileDescriptor::new(FdKind::StdIo(STDIN)));
        fds[STDOUT as usize] = Some(FileDescriptor::new(FdKind::StdIo(STDOUT)));
        fds[STDERR as usize] = Some(FileDescriptor::new(FdKind::StdIo(STDERR)));

        FdTable {
            fds: FdSlots::Slab(fds),
            allocation_limit: INITIAL_FDS,
        }
    }

    /// Take all file descriptor entries out of the table, leaving it empty.
    ///
    /// Returns a Vec of (fd_number, FileDescriptor) pairs for deferred cleanup.
    /// Used by process exit to extract FD entries while holding PM lock,
    /// then close them outside the lock to minimize lock hold time.
    pub fn take_all(&mut self) -> alloc::vec::Vec<(usize, FileDescriptor)> {
        let mut entries = alloc::vec::Vec::new();
        for fd in 0..self.fds.len() {
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
        let slot = self.free_slot(min_fd.max(0) as usize)?;
        self.fds[slot] = Some(FileDescriptor::new(kind));
        Ok(slot as i32)
    }

    /// Make sure a slot is free below the soft limit, growing the table now if
    /// that is what it takes, so a later allocation needs no memory. Returns
    /// false at the limit or when the table cannot grow.
    pub fn reserve_free_slot(&mut self) -> bool {
        self.free_slot(0).is_ok()
    }

    fn free_slot(&mut self, start: usize) -> Result<usize, i32> {
        if let Some(slot) =
            (start..self.allocation_limit.min(self.fds.len())).find(|&i| self.fds[i].is_none())
        {
            return Ok(slot);
        }
        let slot = start.max(self.fds.len());
        if slot >= self.allocation_limit {
            return Err(24);
        }
        let capacity = self
            .fds
            .len()
            .saturating_mul(2)
            .max(slot + 1)
            .min(self.allocation_limit);
        self.fds.grow(capacity)?;
        Ok(slot)
    }

    /// Allocate a pre-configured entry, as used by pipe2.
    pub fn alloc_with_entry(&mut self, entry: FileDescriptor) -> Result<i32, i32> {
        let slot = self.free_slot(0)?;
        self.fds[slot] = Some(entry);
        Ok(slot as i32)
    }

    /// Get a reference to a file descriptor
    pub fn get(&self, fd: i32) -> Option<&FileDescriptor> {
        if fd < 0 || fd as usize >= self.fds.len() {
            return None;
        }
        self.fds[fd as usize].as_ref()
    }

    /// Get a mutable reference to a file descriptor (used by fcntl)
    #[allow(dead_code)]
    pub fn get_mut(&mut self, fd: i32) -> Option<&mut FileDescriptor> {
        if fd < 0 || fd as usize >= self.fds.len() {
            return None;
        }
        self.fds[fd as usize].as_mut()
    }

    /// Close a file descriptor
    /// Returns the closed FileDescriptor on success, or an error code
    pub fn close(&mut self, fd: i32) -> Result<FileDescriptor, i32> {
        if fd < 0 || fd as usize >= self.fds.len() {
            return Err(9); // EBADF - bad file descriptor
        }
        self.fds[fd as usize].take().ok_or(9) // EBADF
    }

    /// Duplicate a file descriptor to a specific slot
    /// Used for dup2() and dup3(). The new descriptor shares the open file
    /// description; its FD_CLOEXEC is set only when `set_cloexec` (dup3's
    /// O_CLOEXEC), never copied from `old_fd`.
    pub fn dup2(
        &mut self,
        old_fd: i32,
        new_fd: i32,
        set_cloexec: bool,
    ) -> Result<(i32, Option<FileDescriptor>), i32> {
        if old_fd < 0 || old_fd as usize >= self.fds.len() {
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
            return Ok((new_fd, None));
        }

        // Equal descriptors allocate nothing, including when the limit was lowered.
        if new_fd as usize >= self.allocation_limit {
            return Err(9);
        }

        self.fds.grow(new_fd as usize + 1)?;
        let mut fd_entry = self.fds[old_fd as usize].clone().ok_or(9)?;
        fd_entry.flags = if set_cloexec { flags::FD_CLOEXEC } else { 0 };

        // Extract the overwritten descriptor under PM. Its caller closes it
        // after releasing PM, after the replacement reference is established.
        let overwritten = self.fds[new_fd as usize].take();

        // Increment ref counts for the duplicated fd. TcpListener/
        // TcpConnection mirror clone_for_fork's arms (this file, above):
        // the whole ref-counted-fd protocol is "every path that creates a
        // second FdEntry pointing at the same underlying listener/
        // connection increments; every path that removes an FdEntry
        // pointing at it decrements (sys_close, FdTable::drop,
        // close_cloexec, and this function's own close-old-new_fd arm
        // above)". #724 review finding M1: this dup2 inc side was missing,
        // so a dup'd listener could be retired by sys_close's dec while a
        // surviving fd still held FdKind::TcpListener(port).
        match &fd_entry.kind {
            FdKind::PipeRead(buffer) => buffer.lock().add_reader(),
            FdKind::PipeWrite(buffer) => buffer.lock().add_writer(),
            FdKind::FifoRead(_, buffer, entry) => {
                entry.lock().readers += 1;
                buffer.lock().add_reader();
            }
            FdKind::FifoWrite(_, buffer, entry) => {
                entry.lock().writers += 1;
                buffer.lock().add_writer();
            }
            FdKind::PtyMaster(pty_num) => {
                if let Some(pair) = crate::tty::pty::get(*pty_num) {
                    pair.master_refcount
                        .fetch_add(1, core::sync::atomic::Ordering::SeqCst);
                }
            }
            FdKind::PtySlave(pty_num) => {
                if let Some(pair) = crate::tty::pty::get(*pty_num) {
                    pair.slave_open();
                }
            }
            FdKind::TcpConnection(conn_id) => {
                crate::net::tcp::tcp_add_ref(conn_id);
            }
            FdKind::TcpListener(port) => {
                crate::net::tcp::tcp_listener_ref_inc(*port);
            }
            _ => {}
        }

        self.fds[new_fd as usize] = Some(fd_entry);
        Ok((new_fd, overwritten))
    }

    /// Duplicate a file descriptor to the lowest available slot
    /// Used for dup() syscall
    pub fn dup(&mut self, old_fd: i32) -> Result<i32, i32> {
        self.dup_at_least(old_fd, 0, false)
    }

    /// Duplicate a file descriptor to slot >= min_fd
    /// Used for fcntl F_DUPFD and F_DUPFD_CLOEXEC
    /// Note: POSIX says dup/F_DUPFD clear FD_CLOEXEC on the new fd
    pub fn dup_at_least(
        &mut self,
        old_fd: i32,
        min_fd: i32,
        set_cloexec: bool,
    ) -> Result<i32, i32> {
        if old_fd < 0 || old_fd as usize >= self.fds.len() {
            return Err(9); // EBADF
        }
        if min_fd < 0 || min_fd as usize >= self.allocation_limit {
            return Err(22); // EINVAL
        }

        let mut fd_entry = self.fds[old_fd as usize].clone().ok_or(9)?;

        // POSIX: dup and F_DUPFD clear FD_CLOEXEC, F_DUPFD_CLOEXEC sets it
        fd_entry.flags = if set_cloexec { flags::FD_CLOEXEC } else { 0 };

        // Reserve a free slot before creating any reference, so EMFILE needs
        // no rollback close or notification while the caller holds PM.
        let free_slot = self.free_slot(min_fd as usize)?;

        // Increment reference counts for the duplicated fd. Same protocol as
        // dup2()'s increment block above and clone_for_fork's arms: every
        // path that hands out a second FdEntry for the same underlying
        // listener/connection must increment, so sys_close's decrement
        // (syscall/pipe.rs) never retires it while a live fd still refers
        // to it (#724 review finding M1 -- this dup()/F_DUPFD path was
        // missing the TcpListener/TcpConnection arms other kinds already
        // had here).
        match &fd_entry.kind {
            FdKind::PipeRead(buffer) => buffer.lock().add_reader(),
            FdKind::PipeWrite(buffer) => buffer.lock().add_writer(),
            FdKind::FifoRead(_, buffer, entry) => {
                entry.lock().readers += 1;
                buffer.lock().add_reader();
            }
            FdKind::FifoWrite(_, buffer, entry) => {
                entry.lock().writers += 1;
                buffer.lock().add_writer();
            }
            FdKind::PtyMaster(pty_num) => {
                if let Some(pair) = crate::tty::pty::get(*pty_num) {
                    pair.master_refcount
                        .fetch_add(1, core::sync::atomic::Ordering::SeqCst);
                }
            }
            FdKind::PtySlave(pty_num) => {
                if let Some(pair) = crate::tty::pty::get(*pty_num) {
                    pair.slave_open();
                }
            }
            FdKind::TcpConnection(conn_id) => {
                crate::net::tcp::tcp_add_ref(conn_id);
            }
            FdKind::TcpListener(port) => {
                crate::net::tcp::tcp_listener_ref_inc(*port);
            }
            _ => {}
        }

        self.fds[free_slot] = Some(fd_entry);
        Ok(free_slot as i32)
    }

    /// Get file descriptor flags (for F_GETFD)
    pub fn get_fd_flags(&self, fd: i32) -> Result<u32, i32> {
        self.get(fd).map(|e| e.flags).ok_or(9) // EBADF
    }

    /// Set file descriptor flags (for F_SETFD)
    pub fn set_fd_flags(&mut self, fd: i32, flags: u32) -> Result<(), i32> {
        self.get_mut(fd).map(|e| e.flags = flags).ok_or(9) // EBADF
    }

    /// Get the open file description's access mode and status flags (for F_GETFL)
    pub fn get_status_flags(&self, fd: i32) -> Result<u32, i32> {
        self.get(fd).map(|e| e.status_flags()).ok_or(9) // EBADF
    }

    /// Count the number of open file descriptors
    pub fn open_fd_count(&self) -> usize {
        self.fds.iter().filter(|slot| slot.is_some()).count()
    }

    /// Close all file descriptors marked with FD_CLOEXEC.
    /// Called during exec() per POSIX semantics.
    /// Properly decrements pipe/fifo reference counts.
    pub fn close_cloexec(&mut self, closes: &mut DeferredFdCloses) {
        for i in 0..self.fds.len() {
            let should_close = self.fds[i]
                .as_ref()
                .map(|fd| (fd.flags & flags::FD_CLOEXEC) != 0)
                .unwrap_or(false);
            if should_close {
                if let Some(entry) = self.fds[i].take() {
                    closes.entries.push((i, entry));
                }
            }
        }
    }

    /// Set file status flags (for F_SETFL) on the open file description, so
    /// every descriptor sharing it sees them. Only modifies O_NONBLOCK and
    /// O_APPEND; the access mode and other flags are ignored.
    pub fn set_status_flags(&mut self, fd: i32, flags: u32) -> Result<(), i32> {
        self.get(fd).ok_or(9)?.set_status_flags(flags); // EBADF
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
        for i in 0..self.fds.len() {
            if let Some(fd_entry) = self.fds[i].take() {
                match fd_entry.kind {
                    FdKind::PipeRead(buffer) => {
                        let notifications = buffer.lock().close_read();
                        notifications.deliver();
                        log::debug!("FdTable::drop() - closed pipe read fd {}", i);
                    }
                    FdKind::PipeWrite(buffer) => {
                        let notifications = buffer.lock().close_write();
                        notifications.deliver();
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
                        log::debug!(
                            "FdTable::drop() - released TCP listener fd {} on port {}",
                            i,
                            port
                        );
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
                            let old_count = pair
                                .master_refcount
                                .fetch_sub(1, core::sync::atomic::Ordering::SeqCst);
                            log::debug!(
                                "FdTable::drop() - PTY master fd {} (pty {}) refcount {} -> {}",
                                i,
                                pty_num,
                                old_count,
                                old_count - 1
                            );
                            if old_count == 1 {
                                crate::tty::pty::release(pty_num);
                                log::debug!(
                                    "FdTable::drop() - released PTY {} (last master closed)",
                                    pty_num
                                );
                            }
                        }
                    }
                    FdKind::PtySlave(pty_num) => {
                        // Decrement slave refcount — master sees POLLHUP when last slave closes
                        if let Some(pair) = crate::tty::pty::get(pty_num) {
                            pair.slave_close();
                        }
                        log::debug!("FdTable::drop() - released PTY slave fd {}", i);
                    }
                    FdKind::UnixStream(socket) => {
                        // Close the Unix socket endpoint
                        let notifications = socket.lock().close();
                        notifications.deliver_deferred();
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
                    FdKind::FifoRead(path, buffer, entry) => {
                        // Decrement FIFO reader count and pipe buffer reader count
                        super::fifo::close_fifo_read(&entry);
                        let notifications = buffer.lock().close_read();
                        notifications.deliver();
                        log::debug!("FdTable::drop() - closed FIFO read fd {} ({})", i, path);
                    }
                    FdKind::FifoWrite(path, buffer, entry) => {
                        // Decrement FIFO writer count and pipe buffer writer count
                        super::fifo::close_fifo_write(&entry);
                        let notifications = buffer.lock().close_write();
                        notifications.deliver();
                        log::debug!("FdTable::drop() - closed FIFO write fd {} ({})", i, path);
                    }
                    FdKind::ProcfsFile { .. } => {
                        // Procfs files are purely in-memory, nothing to clean up
                    }
                    FdKind::ProcfsDirectory { .. } => {
                        // Procfs directory doesn't need cleanup
                    }
                    FdKind::Epoll(id) => {
                        // Clean up the epoll instance
                        crate::syscall::epoll::remove_instance(id);
                    }
                }
            }
        }
    }
}

/// Caller-owned exec cleanup, declared before any process-manager guard.
/// Reverse local destruction order releases PM before this performs cleanup,
/// including error returns after descriptors have been extracted.
#[must_use]
#[derive(Default)]
pub struct DeferredFdCloses {
    entries: alloc::vec::Vec<(usize, FileDescriptor)>,
}

impl DeferredFdCloses {
    /// Drop the record locks `owner` holds on the files these descriptors
    /// referred to: closing any descriptor for a file releases them.
    pub fn release_record_locks(&self, owner: u64) {
        for (_, entry) in &self.entries {
            crate::fs::locks::release_closed(owner, &entry.kind);
        }
    }
}

impl Drop for DeferredFdCloses {
    fn drop(&mut self) {
        crate::task::process_task::close_extracted_fds(core::mem::take(&mut self.entries));
    }
}

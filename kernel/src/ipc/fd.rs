//! File descriptor types and table
//!
//! This module provides the unified file descriptor abstraction for POSIX-like I/O.
//! Each process has its own file descriptor table that maps small integers
//! to underlying file objects (pipes, stdio, sockets, etc.).

use crate::memory::slab::{SlabBox, FD_TABLE_SLAB};
use alloc::sync::{Arc, Weak};
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
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
    /// An IPC object with shared open-description status flags.
    Ipc(Arc<dyn super::object::IpcFile>),
    /// Standard I/O (stdin, stdout, stderr)
    StdIo(i32),
    /// Read end of a pipe
    PipeRead(Arc<Mutex<super::pipe::PipeBuffer>>),
    /// Write end of a pipe
    PipeWrite(Arc<Mutex<super::pipe::PipeBuffer>>),
    /// A socket of any family and type. dup and fork clone the Arc; the
    /// protocol releases the socket when the last reference goes.
    Socket(Arc<crate::socket::object::Socket>),
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
    /// PTY master file descriptor. The descriptor names the pair itself, not
    /// its number, so it can never reach a pair that later took the number.
    PtyMaster(Arc<crate::tty::pty::PtyPair>),
    /// PTY slave file descriptor (see `PtyMaster`).
    PtySlave(Arc<crate::tty::pty::PtyPair>),
    /// FIFO (named pipe) read end - path is stored for cleanup on close
    FifoRead(alloc::string::String, Arc<Mutex<super::pipe::PipeBuffer>>, Arc<Mutex<super::fifo::FifoEntry>>),
    /// FIFO (named pipe) write end - path is stored for cleanup on close
    FifoWrite(alloc::string::String, Arc<Mutex<super::pipe::PipeBuffer>>, Arc<Mutex<super::fifo::FifoEntry>>),
    /// Linux O_RDWR FIFO endpoint: one reader and one writer reference.
    FifoReadWrite(alloc::string::String, Arc<Mutex<super::pipe::PipeBuffer>>, Arc<Mutex<super::fifo::FifoEntry>>),
    /// Procfs virtual file (content generated at open time, shared by reads)
    ProcfsFile {
        mode: u32,
        content: Arc<alloc::vec::Vec<u8>>,
        position: usize,
        /// The entry's user and group (`procfs::owner`).
        owner: (u32, u32),
    },
    /// Procfs directory listing (for /proc and /proc/[pid])
    ProcfsDirectory {
        path: alloc::string::String,
        position: u64,
        /// The directory's user and group (`procfs::owner`).
        owner: (u32, u32),
    },
    /// An epoll instance. dup and fork clone the Arc; the instance goes when
    /// the last descriptor for it closes.
    Epoll(Arc<crate::syscall::epoll::EpollInstance>),
}

impl core::fmt::Debug for FdKind {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            FdKind::Ipc(object) => write!(f, "Ipc({:?})", object),
            FdKind::StdIo(n) => write!(f, "StdIo({})", n),
            FdKind::PipeRead(_) => write!(f, "PipeRead"),
            FdKind::PipeWrite(_) => write!(f, "PipeWrite"),
            FdKind::Socket(socket) => write!(
                f,
                "Socket(family={}, type={})",
                socket.family, socket.kind
            ),
            FdKind::RegularFile(_) => write!(f, "RegularFile"),
            FdKind::Directory(_) => write!(f, "Directory"),
            FdKind::Device(dt) => write!(f, "Device({:?})", dt),
            FdKind::DevfsDirectory { position } => write!(f, "DevfsDirectory(pos={})", position),
            FdKind::DevptsDirectory { position } => write!(f, "DevptsDirectory(pos={})", position),
            FdKind::PtyMaster(pair) => write!(f, "PtyMaster({})", pair.pty_num),
            FdKind::PtySlave(pair) => write!(f, "PtySlave({})", pair.pty_num),
            FdKind::FifoRead(path, _, _) => write!(f, "FifoRead({})", path),
            FdKind::FifoWrite(path, _, _) => write!(f, "FifoWrite({})", path),
            FdKind::FifoReadWrite(path, _, _) => write!(f, "FifoReadWrite({})", path),
            FdKind::ProcfsFile { content, position, .. } => {
                write!(f, "ProcfsFile(len={}, pos={})", content.len(), position)
            }
            FdKind::ProcfsDirectory { path, position, .. } => {
                write!(f, "ProcfsDirectory(path={}, pos={})", path, position)
            }
            FdKind::Epoll(_) => write!(f, "Epoll"),
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
    /// Mounted inode metadata retained when the stream is served by a driver.
    pub(crate) native_inode: Option<crate::fs::ext2::live_inode::FileHandle>,
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
            native_inode: None,
        }
    }

    /// `with_flags`, giving ENOMEM rather than a panic when the open file
    /// description cannot be allocated.
    pub fn try_with_flags(kind: FdKind, flags: u32, status_flags: u32) -> Result<Self, i32> {
        let word = inherent_access_mode(&kind) | (status_flags & SETTABLE_STATUS_FLAGS);
        Ok(FileDescriptor {
            kind,
            flags,
            description: Arc::try_new(AtomicU32::new(word)).map_err(|_| 12)?,
            native_inode: None,
        })
    }

    /// The open file description this descriptor refers to, as a reference
    /// that does not keep it open: epoll watches descriptions this way.
    pub(crate) fn description_ref(&self) -> Weak<AtomicU32> {
        Arc::downgrade(&self.description)
    }

    /// Whether this descriptor refers to the open file description `of`.
    pub(crate) fn refers_to(&self, of: &Weak<AtomicU32>) -> bool {
        core::ptr::eq(Arc::as_ptr(&self.description), of.as_ptr())
    }

    /// Create the descriptor for a new open file description from an open(2)
    /// flags word: its access mode and status flags, and FD_CLOEXEC from
    /// O_CLOEXEC. ENOMEM, rather than a panic, when the description cannot be
    /// allocated.
    pub fn try_opened(kind: FdKind, open_flags: u32) -> Result<Self, u64> {
        let flags = if open_flags & status_flags::O_CLOEXEC != 0 { flags::FD_CLOEXEC } else { 0 };
        let word = (open_flags & status_flags::O_ACCMODE) | (open_flags & SETTABLE_STATUS_FLAGS);
        Ok(Self { kind, flags, description: Arc::try_new(AtomicU32::new(word)).map_err(|_| 12u64)?, native_inode: None })
    }

    /// Attach the native namespace inode without changing driver operations.
    pub fn with_inode(mut self, inode: Option<crate::fs::ext2::live_inode::FileHandle>) -> Self {
        self.native_inode = inode;
        self
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
    /// Atomically replace mutable status flags and return the previous word.
    pub fn replace_status_flags(&self, flags: u32) -> u32 {
        super::fd_flags::replace(&self.description, flags, SETTABLE_STATUS_FLAGS)
    }

    /// Atomically replace only the settable status flags in `mask` and
    /// return the previous word.
    pub fn update_status_flags(&self, mask: u32, flags: u32) -> u32 {
        super::fd_flags::replace(&self.description, flags, mask & SETTABLE_STATUS_FLAGS)
    }

    pub fn set_status_flags(&self, flags: u32) {
        let _ = self
            .description
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |word| {
                Some((word & !SETTABLE_STATUS_FLAGS) | (flags & SETTABLE_STATUS_FLAGS))
            });
    }
}

/// The slots of a file descriptor table.
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

/// Per-process file descriptor table.
///
/// Threads created with CLONE_FILES share one table (#1307): `FdTable` is a
/// handle to the table, and every row of the process holds one. Cloning a
/// handle (fork) copies the table; `share` (CLONE_FILES) hands out another
/// handle to the same one.
pub struct FdTable {
    shared: Arc<SharedFdTable>,
}

/// The table behind every `FdTable` handle that shares it.
///
/// A handle is reached through its `Process` row, and rows are only ever
/// reached under PROCESS_MANAGER, which therefore serializes every access to a
/// table through a row, however many rows' handles share it. A `&`/`&mut`
/// borrow taken through one handle lives no longer than the call that took
/// it, and no caller holds borrows through two rows of one thread group at
/// once.
///
/// A thread also reads its own table without that lock (`try_read_lockless`,
/// #1650), and `access` keeps such a reader and a change apart: a change made
/// under PROCESS_MANAGER marks the table with that hold (`process_manager_hold`)
/// and closes it to these readers until the hold ends, and a change waits
/// out a reader, which holds the table only while it copies what it needs.
struct SharedFdTable {
    inner: core::cell::UnsafeCell<FdTableInner>,
    /// 0 when no reader holds the table and no hold in progress has changed
    /// it; `LOCKLESS_READER` while a reader holds it; otherwise the hold that
    /// changed it, shifted left by one.
    access: AtomicU64,
}

/// `SharedFdTable::access`: a reader without PROCESS_MANAGER holds the table.
const LOCKLESS_READER: u64 = 1;

impl SharedFdTable {
    /// Close the table to readers without PROCESS_MANAGER for the rest of the
    /// hold in progress, waiting out one that holds it now.
    fn claim_for_change(&self) {
        let mine = crate::process::process_manager_hold() << 1;
        loop {
            let now = self.access.load(Ordering::Acquire);
            if now == mine {
                return;
            }
            if now == LOCKLESS_READER {
                core::hint::spin_loop();
                continue;
            }
            if self.access.compare_exchange(now, mine, Ordering::AcqRel, Ordering::Acquire).is_ok() {
                return;
            }
        }
    }
}

// SAFETY: see `SharedFdTable`: PROCESS_MANAGER serializes access through rows,
// and `access` keeps lock-free readers and changes apart.
unsafe impl Sync for SharedFdTable {}
// SAFETY: the table owns its descriptors; it may move to another CPU's context.
unsafe impl Send for SharedFdTable {}

impl core::ops::Deref for FdTable {
    type Target = FdTableInner;
    fn deref(&self) -> &FdTableInner {
        // SAFETY: see `SharedFdTable`.
        unsafe { &*self.shared.inner.get() }
    }
}

impl core::ops::DerefMut for FdTable {
    fn deref_mut(&mut self) -> &mut FdTableInner {
        self.shared.claim_for_change();
        // SAFETY: see `SharedFdTable`.
        unsafe { &mut *self.shared.inner.get() }
    }
}

/// A thread's reference to its process's descriptor table, which does not
/// keep the table alive (`ThreadSignals::fd_table`).
#[derive(Clone)]
pub struct FdTableRef(alloc::sync::Weak<SharedFdTable>);

impl FdTableRef {
    /// The table, while its process still holds it.
    pub fn upgrade(&self) -> Option<FdTable> {
        self.0.upgrade().map(|shared| FdTable { shared })
    }
}

impl Default for FdTable {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for FdTable {
    /// A copy of the table, as fork gives a child: its descriptors refer to
    /// the same open file descriptions.
    fn clone(&self) -> Self {
        Self::from_inner((**self).clone())
    }
}

impl FdTable {
    fn from_inner(inner: FdTableInner) -> Self {
        Self {
            shared: Arc::new(SharedFdTable {
                inner: core::cell::UnsafeCell::new(inner),
                access: AtomicU64::new(0),
            }),
        }
    }

    /// A reference to this table that does not keep it alive.
    pub fn downgrade(&self) -> FdTableRef {
        FdTableRef(Arc::downgrade(&self.shared))
    }

    /// Run `read` on the table without PROCESS_MANAGER (#1650). None when a
    /// hold of that lock in progress has changed the table, or another such
    /// reader keeps it past a few tries: the caller takes the lock.
    pub fn try_read_lockless<R>(&self, read: impl FnOnce(&FdTableInner) -> R) -> Option<R> {
        const TRIES: usize = 64;
        let shared = &self.shared;
        for _ in 0..TRIES {
            let now = shared.access.load(Ordering::Acquire);
            if now == LOCKLESS_READER {
                core::hint::spin_loop();
                continue;
            }
            if now != 0 && now >> 1 == crate::process::process_manager_hold() {
                return None;
            }
            if shared
                .access
                .compare_exchange(now, LOCKLESS_READER, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                // SAFETY: no change is in progress and none starts until the
                // reader releases the table (`claim_for_change`).
                let result = read(unsafe { &*shared.inner.get() });
                shared.access.store(0, Ordering::Release);
                return Some(result);
            }
        }
        None
    }

    /// Create a new file descriptor table with standard I/O pre-allocated
    pub fn new() -> Self {
        Self::from_inner(FdTableInner::new())
    }

    /// A table with no descriptors at all.
    pub fn empty() -> Self {
        Self::from_inner(FdTableInner::empty())
    }

    /// Another handle to this same table (CLONE_FILES).
    pub fn share(&self) -> Self {
        Self {
            shared: self.shared.clone(),
        }
    }

    /// Whether `other` is a handle to this same table.
    pub fn is_same_table(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.shared, &other.shared)
    }

    /// Whether another row holds a handle to this table.
    pub fn is_shared(&self) -> bool {
        Arc::strong_count(&self.shared) > 1
    }

    /// Give this handle a table of its own: when another row shares the
    /// table (CLONE_FILES), the handle is replaced by a copy of it, whose
    /// descriptors refer to the same open file descriptions. Exec does this
    /// before it closes FD_CLOEXEC descriptors, so the closes, and every
    /// later change, leave the other rows' table alone.
    pub fn unshare(&mut self) {
        if self.is_shared() {
            *self = self.clone();
        }
    }
}

/// Count one more descriptor entry for the object `kind` names, as dup, dup2,
/// fork and SCM_RIGHTS do when they create a second entry for it. Pipes,
/// FIFOs and PTYs keep their own counts; everything else (sockets included)
/// is counted by its `Arc`.
pub(crate) fn add_reference(kind: &FdKind) {
    match kind {
        FdKind::PipeRead(buffer) => buffer.lock().add_reader(),
        FdKind::PipeWrite(buffer) => buffer.lock().add_writer(),
        FdKind::FifoRead(_, buffer, entry) => {
            // Increment both FIFO entry reader count and pipe buffer reader count
            entry.lock().readers += 1;
            buffer.lock().add_reader();
        }
        FdKind::FifoReadWrite(_, buffer, entry) => {
            {
                let mut e = entry.lock();
                e.readers += 1;
                e.writers += 1;
            }
            {
                let mut p = buffer.lock();
                p.add_reader();
                p.add_writer();
            }
        }
        FdKind::FifoWrite(_, buffer, entry) => {
            // Increment both FIFO entry writer count and pipe buffer writer count
            entry.lock().writers += 1;
            buffer.lock().add_writer();
        }
        FdKind::PtyMaster(pair) => {
            // No logging - this runs during fork()
            pair.master_refcount
                .fetch_add(1, core::sync::atomic::Ordering::SeqCst);
        }
        FdKind::PtySlave(pair) => {
            pair.slave_open();
        }
        _ => {}
    }
}

impl FileDescriptor {
    /// A second entry for the same open file description, counted as dup
    /// counts it. SCM_RIGHTS carries descriptors this way; one that is never
    /// installed is closed through `process_task::close_extracted_fds`.
    pub fn counted_clone(&self) -> FileDescriptor {
        add_reference(&self.kind);
        self.clone()
    }
}

/// The descriptors of an `FdTable`.
pub struct FdTableInner {
    /// The file descriptors (None = unused slot)
    fds: FdSlots,
    allocation_limit: usize,
    /// Free slots an open holds for the descriptor it installs once it has
    /// changed the disk (`reserve_slot`). No other allocation takes them.
    reserved: alloc::vec::Vec<usize>,
}

impl Clone for FdTableInner {
    fn clone(&self) -> Self {
        // CRITICAL: No logging here - this runs during fork() with potential timer interrupts
        // Logging can cause deadlock if timer fires while holding logger lock
        let cloned_fds = self.fds.clone();

        // Increment reference counts for all cloned fds that need it
        for fd_opt in cloned_fds.iter() {
            if let Some(fd_entry) = fd_opt {
                add_reference(&fd_entry.kind);
            }
        }

        FdTableInner {
            fds: cloned_fds,
            allocation_limit: self.allocation_limit,
            // A reservation belongs to the open in progress in this table.
            reserved: alloc::vec::Vec::new(),
        }
    }
}

impl FdTableInner {
    /// Lowering a limit leaves already open descriptors usable.
    pub fn set_limit(&mut self, limit: u64) {
        self.allocation_limit = limit.min(MAX_FDS as u64) as usize;
    }

    /// A table with standard I/O pre-allocated.
    fn new() -> Self {
        let mut table = Self::empty();
        // Pre-allocate stdin, stdout, stderr
        table.fds[STDIN as usize] = Some(FileDescriptor::new(FdKind::StdIo(STDIN)));
        table.fds[STDOUT as usize] = Some(FileDescriptor::new(FdKind::StdIo(STDOUT)));
        table.fds[STDERR as usize] = Some(FileDescriptor::new(FdKind::StdIo(STDERR)));
        table
    }

    /// A table with no descriptors.
    fn empty() -> Self {
        // Try slab allocation first (O(1)), fall back to global heap
        let fds = if let Some(raw) = FD_TABLE_SLAB.alloc() {
            // Slab returns zeroed memory; write None into each slot
            let arr = raw as *mut [Option<FileDescriptor>; INITIAL_FDS];
            for i in 0..INITIAL_FDS {
                unsafe {
                    core::ptr::write(&mut (*arr)[i], None);
                }
            }
            unsafe { SlabBox::from_slab(arr, &FD_TABLE_SLAB) }
        } else {
            SlabBox::heap_from_fn(|_| None)
        };

        FdTableInner {
            fds: FdSlots::Slab(fds),
            allocation_limit: INITIAL_FDS,
            reserved: alloc::vec::Vec::new(),
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

    /// Hold the lowest free slot below the soft limit, growing the table now
    /// if that is what it takes, for a descriptor installed later with
    /// `install_reserved`. No other allocation takes the slot meanwhile, even
    /// one by a thread sharing the table (CLONE_FILES), so the install needs
    /// no memory and cannot fail. None at the limit or when the table cannot
    /// grow.
    pub fn reserve_slot(&mut self) -> Option<i32> {
        let slot = self.free_slot(0).ok()?;
        self.reserved.try_reserve(1).ok()?;
        self.reserved.push(slot);
        Some(slot as i32)
    }

    /// `reserve_slot`, giving the reason when no slot can be held: EMFILE at
    /// the limit, ENOMEM when the table cannot grow.
    pub fn try_reserve_slot(&mut self) -> Result<i32, i32> {
        let slot = self.free_slot(0)?;
        self.reserved.try_reserve(1).map_err(|_| 12)?;
        self.reserved.push(slot);
        Ok(slot as i32)
    }

    /// Install `entry` in slot `fd`, held by `reserve_slot`. Hands the entry
    /// back when the slot is not held, so nothing it counts is lost.
    pub fn install_held(&mut self, fd: i32, entry: FileDescriptor) -> Result<i32, FileDescriptor> {
        let Some(index) = self.reserved.iter().position(|&slot| slot as i32 == fd) else {
            return Err(entry);
        };
        let slot = self.reserved.swap_remove(index);
        self.fds[slot] = Some(entry);
        Ok(fd)
    }

    /// The lowest descriptor number that refers to the open file description
    /// `of`.
    pub(crate) fn find_description(&self, of: &Weak<AtomicU32>) -> Option<(i32, &FileDescriptor)> {
        self.fds
            .iter()
            .enumerate()
            .find_map(|(fd, entry)| match entry {
                Some(entry) if entry.refers_to(of) => Some((fd as i32, entry)),
                _ => None,
            })
    }

    /// Install `entry` in slot `fd`, held by `reserve_slot`. A slot that is
    /// not held is allocated as `alloc_with_entry` would.
    pub fn install_reserved(&mut self, fd: i32, entry: FileDescriptor) -> Result<i32, i32> {
        let Some(index) = self.reserved.iter().position(|&slot| slot as i32 == fd) else {
            return self.alloc_with_entry(entry);
        };
        let slot = self.reserved.swap_remove(index);
        self.fds[slot] = Some(entry);
        Ok(fd)
    }

    /// Give back slot `fd`, held by `reserve_slot` for an open that failed.
    pub fn release_reserved(&mut self, fd: i32) {
        self.reserved.retain(|&slot| slot as i32 != fd);
    }

    fn free_slot(&mut self, start: usize) -> Result<usize, i32> {
        if let Some(slot) = (start..self.allocation_limit.min(self.fds.len()))
            .find(|&i| self.fds[i].is_none() && !self.reserved.contains(&i))
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
        // A slot an open in progress holds is busy (Linux: EBUSY).
        if self.reserved.contains(&(new_fd as usize)) {
            return Err(16);
        }

        self.fds.grow(new_fd as usize + 1)?;
        let mut fd_entry = self.fds[old_fd as usize].clone().ok_or(9)?;
        fd_entry.flags = if set_cloexec { flags::FD_CLOEXEC } else { 0 };

        // Extract the overwritten descriptor under PM. Its caller closes it
        // after releasing PM, after the replacement reference is established.
        let overwritten = self.fds[new_fd as usize].take();

        // Every path that creates a second entry for the same pipe, FIFO or
        // PTY increments its count here; every path that removes one
        // decrements (sys_close, FdTable::drop, close_cloexec, and the
        // caller's close of the overwritten entry). #724 review finding M1
        // was this dup2 side missing. Sockets are counted by their Arc.
        add_reference(&fd_entry.kind);

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

        // Same protocol as dup2() above and fork's table clone: the second
        // entry is counted before it exists, so sys_close's decrement never
        // retires the object while a live descriptor still refers to it.
        add_reference(&fd_entry.kind);

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

    /// The kinds of the open descriptors.
    pub fn kinds(&self) -> impl Iterator<Item = &FdKind> {
        self.fds.iter().flatten().map(|entry| &entry.kind)
    }

    /// Close all file descriptors marked with FD_CLOEXEC.
    /// Called during exec() per POSIX semantics.
    /// Properly decrements pipe/fifo reference counts.
    pub fn close_cloexec(&mut self, pid: crate::process::ProcessId, closes: &mut DeferredFdCloses) {
        for i in 0..self.fds.len() {
            let should_close = self.fds[i]
                .as_ref()
                .map(|fd| (fd.flags & flags::FD_CLOEXEC) != 0)
                .unwrap_or(false);
            if should_close {
                if let Some(entry) = self.fds[i].take() {
                    if let FdKind::Ipc(object) = &entry.kind {
                        object.on_close(pid);
                    }
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
/// Sockets are released by their own Drop when the last Arc reference goes.
impl Drop for FdTableInner {
    fn drop(&mut self) {
        log::debug!("FdTable::drop() - closing all fds and decrementing pipe counts");
        for i in 0..self.fds.len() {
            if let Some(fd_entry) = self.fds[i].take() {
                match fd_entry.kind {
                    FdKind::Ipc(_) => {}
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
                    FdKind::Socket(_) => {
                        // The last reference releases the socket; under PM
                        // the release waits for the reclamation boundary.
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
                    FdKind::PtyMaster(pair) => crate::tty::pty::close_master_ref(&pair),
                    FdKind::PtySlave(pair) => crate::tty::pty::close_slave_ref(&pair),
                    FdKind::FifoRead(path, buffer, entry) => {
                        // Decrement FIFO reader count and pipe buffer reader count
                        super::fifo::close_fifo_read(&entry);
                        let notifications = buffer.lock().close_read();
                        notifications.deliver();
                        log::debug!("FdTable::drop() - closed FIFO read fd {} ({})", i, path);
                    }
                    FdKind::FifoReadWrite(_, buffer, entry) => {
                        super::fifo::close_fifo_read(&entry);
                        super::fifo::close_fifo_write(&entry);
                        let read = buffer.lock().close_read();
                        read.deliver();
                        let write = buffer.lock().close_write();
                        write.deliver();
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
                    FdKind::Epoll(_) => {
                        // The instance goes with its last descriptor.
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
    pub fn reserve(&mut self, count: usize) -> Result<(), &'static str> {
        self.entries.try_reserve(count).map_err(|_| "Out of memory for exec descriptor closes")
    }

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

//! FIFO (named pipe) implementation
//!
//! FIFOs are special files that provide pipe-like communication through a
//! filesystem path. They enable unrelated processes to communicate by
//! opening the same path.
//!
//! Key semantics:
//! - Opening for read blocks until a writer opens (unless O_NONBLOCK)
//! - Opening for write blocks until a reader opens (unless O_NONBLOCK)
//! - Once both ends are open, I/O works exactly like anonymous pipes
//! - FIFOs persist in the filesystem namespace until unlinked

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;

use super::pipe::PipeBuffer;
use crate::arch_impl::traits::CpuOps;

// Architecture-specific CPU type for interrupt control
#[cfg(target_arch = "x86_64")]
type Cpu = crate::arch_impl::x86_64::X86Cpu;
#[cfg(target_arch = "aarch64")]
type Cpu = crate::arch_impl::aarch64::Aarch64Cpu;

/// Global FIFO registry
pub static FIFO_REGISTRY: FifoRegistry = FifoRegistry::new();

/// A FIFO entry in the registry
pub struct FifoEntry {
    /// The underlying pipe buffer (created on first open)
    pub buffer: Option<Arc<Mutex<PipeBuffer>>>,
    /// Number of processes that have opened for reading
    pub readers: usize,
    /// Number of processes that have opened for writing
    pub writers: usize,
    /// Opens for reading since creation. A blocked writer waits for this to
    /// move rather than for `readers` to be non-zero, so a reader that opens
    /// and closes again before the writer runs still completes its open
    /// (Linux's `pipe->r_counter`).
    pub reader_opens: u64,
    /// Opens for writing since creation; see `reader_opens`.
    pub writer_opens: u64,
    /// Threads waiting to open for reading (waiting for writer)
    pub read_waiters: Vec<u64>,
    /// Threads waiting to open for writing (waiting for reader)
    pub write_waiters: Vec<u64>,
    /// File mode (permissions)
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
}

impl FifoEntry {
    /// Create a new FIFO entry
    pub fn new(mode: u32, uid: u32, gid: u32) -> Self {
        FifoEntry {
            buffer: None,
            readers: 0,
            writers: 0,
            reader_opens: 0,
            writer_opens: 0,
            read_waiters: Vec::new(),
            write_waiters: Vec::new(),
            mode,
            uid,
            gid,        }
    }

    /// In-memory metadata uses the same permission and ownership rules as ext2.
    pub(crate) fn inode(&self) -> crate::fs::ext2::Ext2Inode {
        let mut inode = crate::fs::ext2::Ext2Inode::new_regular_file(0, 0, 0);
        inode.i_mode = crate::fs::ext2::inode::EXT2_S_IFIFO | self.mode as u16;
        inode.set_owner(self.uid, self.gid);
        inode
    }

    /// Get or create the pipe buffer
    pub fn get_or_create_buffer(&mut self) -> Arc<Mutex<PipeBuffer>> {
        if let Some(ref buffer) = self.buffer {
            buffer.clone()
        } else {
            // POSIX read(): an empty FIFO with no writer returns EOF (0),
            // including an O_NONBLOCK reader opened before the first writer.
            // Do not invent a writer reference to turn that EOF into EAGAIN.
            let buffer = Arc::new(Mutex::new(PipeBuffer::new_zero_refs()));
            self.buffer = Some(buffer.clone());
            buffer
        }
    }

    /// Check if the FIFO has both readers and writers (ready for I/O)
    #[allow(dead_code)] // Part of FifoEntry public API
    pub fn is_ready(&self) -> bool {
        self.readers > 0 && self.writers > 0
    }

    /// Add a reader and wake any waiting writers
    pub fn add_reader(&mut self) {
        self.readers += 1;
        self.reader_opens = self.reader_opens.wrapping_add(1);
        // Wake writers waiting for a reader
        let waiters: Vec<u64> = self.write_waiters.drain(..).collect();
        for tid in waiters {
            crate::task::scheduler::with_scheduler(|sched| {
                sched.unblock(tid);
            });
        }
    }

    /// Add a writer and wake any waiting readers
    pub fn add_writer(&mut self) {
        self.writers += 1;
        self.writer_opens = self.writer_opens.wrapping_add(1);
        // Wake readers waiting for a writer
        let waiters: Vec<u64> = self.read_waiters.drain(..).collect();
        for tid in waiters {
            crate::task::scheduler::with_scheduler(|sched| {
                sched.unblock(tid);
            });
        }
    }

    /// Remove a reader
    pub fn remove_reader(&mut self) {
        if self.readers > 0 {
            self.readers -= 1;
        }
    }

    /// Remove a writer
    pub fn remove_writer(&mut self) {
        if self.writers > 0 {
            self.writers -= 1;
        }
    }

    /// Add current thread as waiting for read
    pub fn add_read_waiter(&mut self, tid: u64) {
        if !self.read_waiters.contains(&tid) {
            self.read_waiters.push(tid);
        }
    }

    /// Add current thread as waiting for write
    pub fn add_write_waiter(&mut self, tid: u64) {
        if !self.write_waiters.contains(&tid) {
            self.write_waiters.push(tid);
        }
    }

    /// Remove thread from read waiters
    pub fn remove_read_waiter(&mut self, tid: u64) {
        self.read_waiters.retain(|&t| t != tid);
    }

    /// Remove thread from write waiters
    pub fn remove_write_waiter(&mut self, tid: u64) {
        self.write_waiters.retain(|&t| t != tid);
    }
}

/// Registry of all FIFOs in the system
pub struct FifoRegistry {
    /// Map from path to FIFO entry
    fifos: Mutex<BTreeMap<String, Arc<Mutex<FifoEntry>>>>,
}

impl FifoRegistry {
    /// Create a new empty registry
    pub const fn new() -> Self {
        FifoRegistry {
            fifos: Mutex::new(BTreeMap::new()),
        }
    }

    /// Create a new FIFO at the given path
    ///
    /// Returns Ok(()) on success, Err(errno) on failure:
    /// - EEXIST (17) if path already exists
    pub fn create(&self, path: &str, mode: u32, uid: u32, gid: u32) -> Result<(), i32> {
        let mut fifos = self.fifos.lock();

        if fifos.contains_key(path) {
            return Err(17); // EEXIST
        }

        let entry = Arc::new(Mutex::new(FifoEntry::new(mode, uid, gid)));
        fifos.insert(String::from(path), entry);

        log::debug!("FIFO created: {} with mode {:#o}", path, mode);
        Ok(())
    }

    /// Check if a path is a FIFO
    pub fn exists(&self, path: &str) -> bool {
        self.fifos.lock().contains_key(path)
    }

    /// Get a FIFO entry by path
    pub fn get(&self, path: &str) -> Option<Arc<Mutex<FifoEntry>>> {
        self.fifos.lock().get(path).cloned()
    }

    /// Remove a FIFO from the registry
    ///
    /// Returns Ok(()) on success, Err(ENOENT) if not found
    pub fn unlink(&self, path: &str) -> Result<(), i32> {
        let mut fifos = self.fifos.lock();

        if fifos.remove(path).is_some() {
            log::debug!("FIFO unlinked: {}", path);
            Ok(())
        } else {
            Err(2) // ENOENT
        }
    }

    /// List all FIFOs (for debugging)
    #[allow(dead_code)]
    pub fn list(&self) -> Vec<String> {
        self.fifos.lock().keys().cloned().collect()
    }
}

/// Result of opening a FIFO
pub enum FifoOpenResult {
    /// FIFO opened successfully, here's the buffer
    Ready(Arc<Mutex<PipeBuffer>>),
    /// Need to block waiting for the other end. Carries the other end's
    /// open count when the caller blocked; the open completes once it moves.
    Block(u64),
    /// Error occurred
    Error(i32),
}

/// Open the held FIFO for reading; pathname replacement cannot change it.
///
/// If no writer is present and O_NONBLOCK is not set, this will block.
/// O_NONBLOCK read-only open succeeds even when no writer is present.
pub fn open_fifo_read(entry_arc: &Arc<Mutex<FifoEntry>>, nonblock: bool) -> FifoOpenResult {
    // CRITICAL: Disable interrupts during lock acquisition to prevent
    // preemption while holding the lock.
    Cpu::without_interrupts(|| {
        let mut entry = entry_arc.lock();

        // Get or create the buffer
        let buffer = entry.get_or_create_buffer();

        // Add ourselves as a reader
        entry.add_reader();

        // Increment the pipe buffer's reader count
        buffer.lock().add_reader();

        // Check if a writer exists
        if entry.writers > 0 {
            // Writer exists, ready to go
            FifoOpenResult::Ready(buffer)
        } else if nonblock {
            // No writer and non-blocking - still open but may get EAGAIN on read
            // POSIX says O_NONBLOCK read-only open succeeds immediately
            FifoOpenResult::Ready(buffer)
        } else {
            // Need to block waiting for writer
            if let Some(tid) = crate::task::scheduler::current_thread_id() {
                entry.add_read_waiter(tid);
            }
            FifoOpenResult::Block(entry.writer_opens)
        }
    })
}

/// Open the held FIFO for writing; pathname replacement cannot change it.
///
/// If no reader is present and O_NONBLOCK is not set, this will block.
/// If O_NONBLOCK is set and no reader is present, returns ENXIO.
pub fn open_fifo_write(entry_arc: &Arc<Mutex<FifoEntry>>, nonblock: bool) -> FifoOpenResult {
    // CRITICAL: Disable interrupts during lock acquisition to prevent
    // preemption while holding the lock.
    Cpu::without_interrupts(|| {
        let mut entry = entry_arc.lock();

        // Check if a reader exists first (for O_NONBLOCK case)
        if entry.readers == 0 && nonblock {
            // No reader and non-blocking - POSIX says return ENXIO
            return FifoOpenResult::Error(6); // ENXIO
        }

        // Get or create the buffer
        let buffer = entry.get_or_create_buffer();

        // Add ourselves as a writer
        entry.add_writer();

        // Increment the pipe buffer's writer count
        buffer.lock().add_writer();

        // Check if a reader exists
        if entry.readers > 0 {
            // Reader exists, ready to go
            FifoOpenResult::Ready(buffer)
        } else {
            // Need to block waiting for reader
            if let Some(tid) = crate::task::scheduler::current_thread_id() {
                entry.add_write_waiter(tid);
            }
            FifoOpenResult::Block(entry.reader_opens)
        }
    })
}

/// Re-check a blocked FIFO open and, if no partner has opened the other end
/// since the caller blocked, register the caller as waiting for it again.
///
/// The open completes once the other end's open count has moved past
/// `partner_opens_seen` (from `FifoOpenResult::Block`), as Linux's
/// `wait_for_partner` does: the partner may already have closed again by
/// the time the opener runs, and the opener then reads EOF or gets EPIPE
/// rather than waiting for another partner. A blocked open can also be
/// woken by something other than a partner -- a signal, or a wake left over
/// from an earlier wait of the same thread -- and then waits again, keeping
/// the reference it took in `open_fifo_read`/`open_fifo_write`. The check
/// and the registration share the entry lock, so an arrival cannot fall
/// between them.
pub fn recheck_fifo_open(
    entry_arc: &Arc<Mutex<FifoEntry>>,
    for_write: bool,
    partner_opens_seen: u64,
) -> FifoOpenResult {
    Cpu::without_interrupts(|| {
        let mut entry = entry_arc.lock();
        let partner_opened = if for_write {
            entry.readers > 0 || entry.reader_opens != partner_opens_seen
        } else {
            entry.writers > 0 || entry.writer_opens != partner_opens_seen
        };
        if partner_opened {
            if let Some(ref buffer) = entry.buffer {
                return FifoOpenResult::Ready(buffer.clone());
            }
        }
        if let Some(tid) = crate::task::scheduler::current_thread_id() {
            if for_write {
                entry.add_write_waiter(tid);
            } else {
                entry.add_read_waiter(tid);
            }
        }
        FifoOpenResult::Block(partner_opens_seen)
    })
}

/// A FIFO open that holds the reader or writer reference it took in
/// `open_fifo_read`/`open_fifo_write` but has no descriptor yet: it is
/// waiting for the other end, or about to install its descriptor.
///
/// A blocked opener is recorded on its process row
/// (`Process::pending_fifo_opens`) for as long as it waits, because a
/// SIGKILL terminates a parked thread without returning through the open;
/// `process::exit_process_and_retire` then gives the reference back. Exactly
/// one side releases it: whoever takes the record off the row under
/// PROCESS_MANAGER, the opener before it installs its descriptor or abandons
/// the open, or the exit. A missing record means the exit already did.
pub struct PendingFifoOpen {
    /// The opening thread, registered as a waiter on the entry.
    pub tid: u64,
    pub entry: Arc<Mutex<FifoEntry>>,
    pub for_write: bool,
}

/// Give back the reference an open took when it ends without a descriptor:
/// interrupted by a signal, killed while it waited, or refused a descriptor.
/// Without this the abandoned opener stays counted: a reader that later opens
/// sees a writer that will never write and blocks in read() instead of
/// reading EOF.
///
/// Must not be called with PROCESS_MANAGER held: the close notifications are
/// delivered inline.
pub fn abandon_fifo_open(open: &PendingFifoOpen) {
    let buffer = Cpu::without_interrupts(|| {
        let mut entry = open.entry.lock();
        if open.for_write {
            entry.remove_write_waiter(open.tid);
            entry.remove_writer();
        } else {
            entry.remove_read_waiter(open.tid);
            entry.remove_reader();
        }
        entry.buffer.clone()
    });
    if let Some(buffer) = buffer {
        if open.for_write {
            let notifications = buffer.lock().close_write();
            notifications.deliver();
        } else {
            let notifications = buffer.lock().close_read();
            notifications.deliver();
        }
    }
}

/// Close a FIFO read end
pub fn close_fifo_read(entry: &Arc<Mutex<FifoEntry>>) {
    entry.lock().remove_reader();
}

/// Close the write end of the original FIFO, even after unlink/recreation.
pub fn close_fifo_write(entry: &Arc<Mutex<FifoEntry>>) {
    entry.lock().remove_writer();
}

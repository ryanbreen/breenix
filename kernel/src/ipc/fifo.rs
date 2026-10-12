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

use alloc::sync::{Arc, Weak};
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
    /// Held filesystem identity for a disk-backed FIFO, including after unlink.
    pub handle: Option<crate::fs::ext2::live_inode::FileHandle>,
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

}

impl FifoEntry {
    /// Create a new FIFO entry
    pub fn new() -> Self {
        FifoEntry {
            handle: None,
            buffer: None,
            readers: 0,
            writers: 0,
            reader_opens: 0,
            writer_opens: 0,
            read_waiters: Vec::new(),
            write_waiters: Vec::new(),
        }
    }

    /// Get or create the pipe buffer
    pub fn get_or_create_buffer(&mut self) -> Result<Arc<Mutex<PipeBuffer>>, i32> {
        if let Some(ref buffer) = self.buffer {
            Ok(buffer.clone())
        } else {
            // POSIX read(): an empty FIFO with no writer returns EOF (0),
            // including an O_NONBLOCK reader opened before the first writer.
            // Do not invent a writer reference to turn that EOF into EAGAIN.
            let buffer = Arc::try_new(Mutex::new(PipeBuffer::try_new_zero_refs()?)).map_err(|_| 12)?;
            self.buffer = Some(buffer.clone());
            Ok(buffer)
        }
    }

    /// Add a reader and wake any waiting writers
    pub fn add_reader(&mut self) {
        self.readers += 1;
        self.reader_opens = self.reader_opens.wrapping_add(1);
        // Wake writers waiting for a reader
        let waiters = core::mem::take(&mut self.write_waiters);
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
        let waiters = core::mem::take(&mut self.read_waiters);
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
        self.discard_unused_buffer();
    }

    /// Remove a writer
    pub fn remove_writer(&mut self) {
        if self.writers > 0 {
            self.writers -= 1;
        }
        self.discard_unused_buffer();
    }

    /// Last-close cleanup is a leaf under the entry lock, including under PM.
    /// Pending opens count as endpoints until abandoned or installed.
    fn discard_unused_buffer(&mut self) {
        if self.readers == 0 && self.writers == 0 {
            self.buffer = None;
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

/// Runtime pipe state, keyed by the held filesystem inode's incarnation.
/// Weak entries keep the table from pinning an unlinked inode after last close.
pub struct FifoRegistry {
    fifos: Mutex<Vec<(crate::fs::ext2::live_inode::FileKey, Weak<Mutex<FifoEntry>>)>>,
}
impl FifoRegistry {
    pub const fn new() -> Self { Self { fifos: Mutex::new(Vec::new()) } }
    pub fn get_or_create(
        &self, handle: crate::fs::ext2::live_inode::FileHandle,
    ) -> Result<Arc<Mutex<FifoEntry>>, i32> {
        let key = handle.object.key;
        let mut fifos = self.fifos.lock();
        fifos.retain(|(_, entry)| entry.strong_count() != 0);
        if let Some(entry) = fifos.iter().find_map(|(held, entry)| (*held == key).then(|| entry.upgrade()).flatten()) {
            return Ok(entry);
        }
        fifos.try_reserve(1).map_err(|_| 12)?;
        let mut entry = FifoEntry::new();
        entry.handle = Some(handle);
        let entry = Arc::try_new(Mutex::new(entry)).map_err(|_| 12)?;
        fifos.push((key, Arc::downgrade(&entry)));
        Ok(entry)
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
        if !nonblock && entry.read_waiters.try_reserve(1).is_err() { return FifoOpenResult::Error(12); }
        let buffer = match entry.get_or_create_buffer() { Ok(b) => b, Err(e) => return FifoOpenResult::Error(e) };

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
        if !nonblock && entry.write_waiters.try_reserve(1).is_err() { return FifoOpenResult::Error(12); }
        let buffer = match entry.get_or_create_buffer() { Ok(b) => b, Err(e) => return FifoOpenResult::Error(e) };

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

/// Linux opens both ends atomically for O_RDWR, with no partner wait.
pub fn open_fifo_readwrite(entry_arc: &Arc<Mutex<FifoEntry>>) -> FifoOpenResult {
    Cpu::without_interrupts(|| {
        let mut entry = entry_arc.lock();
        let buffer = match entry.get_or_create_buffer() { Ok(b) => b, Err(e) => return FifoOpenResult::Error(e) };
        entry.add_reader();
        entry.add_writer();
        { let mut pipe = buffer.lock(); pipe.add_reader(); pipe.add_writer(); }
        FifoOpenResult::Ready(buffer)
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
                if entry.write_waiters.try_reserve(1).is_err() { return FifoOpenResult::Error(12); }
                entry.add_write_waiter(tid);
            } else {
                if entry.read_waiters.try_reserve(1).is_err() { return FifoOpenResult::Error(12); }
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
        // Retain the old generation before last-close discards the entry's Arc,
        // so the abandoned open still releases its pipe count and notifications.
        let buffer = entry.buffer.clone();
        if open.for_write {
            entry.remove_write_waiter(open.tid);
            entry.remove_writer();
        } else {
            entry.remove_read_waiter(open.tid);
            entry.remove_reader();
        }
        buffer
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

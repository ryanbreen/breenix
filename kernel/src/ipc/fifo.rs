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
    /// Threads waiting to open for reading (waiting for writer)
    pub read_waiters: Vec<u64>,
    /// Threads waiting to open for writing (waiting for reader)
    pub write_waiters: Vec<u64>,
    /// File mode (permissions)
    pub mode: u32,
}

impl FifoEntry {
    /// Create a new FIFO entry
    pub fn new(mode: u32) -> Self {
        FifoEntry {
            buffer: None,
            readers: 0,
            writers: 0,
            read_waiters: Vec::new(),
            write_waiters: Vec::new(),
            mode,
        }
    }

    /// Get or create the pipe buffer
    pub fn get_or_create_buffer(&mut self) -> Arc<Mutex<PipeBuffer>> {
        if let Some(ref buffer) = self.buffer {
            buffer.clone()
        } else {
            let buffer = Arc::new(Mutex::new(PipeBuffer::new()));
            self.buffer = Some(buffer.clone());
            buffer
        }
    }

    /// Check if the FIFO has both readers and writers (ready for I/O)
    pub fn is_ready(&self) -> bool {
        self.readers > 0 && self.writers > 0
    }

    /// Add a reader and wake any waiting writers
    pub fn add_reader(&mut self) {
        self.readers += 1;
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
            // Also update the pipe buffer's reader count
            if let Some(ref buffer) = self.buffer {
                buffer.lock().close_read();
            }
        }
    }

    /// Remove a writer
    pub fn remove_writer(&mut self) {
        if self.writers > 0 {
            self.writers -= 1;
            // Also update the pipe buffer's writer count
            if let Some(ref buffer) = self.buffer {
                buffer.lock().close_write();
            }
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
    pub fn create(&self, path: &str, mode: u32) -> Result<(), i32> {
        let mut fifos = self.fifos.lock();

        if fifos.contains_key(path) {
            return Err(17); // EEXIST
        }

        let entry = Arc::new(Mutex::new(FifoEntry::new(mode)));
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
    /// Need to block waiting for the other end
    Block,
    /// Error occurred
    Error(i32),
}

/// Open a FIFO for reading
///
/// If no writer is present and O_NONBLOCK is not set, this will block.
/// If O_NONBLOCK is set and no writer is present, returns ENXIO.
pub fn open_fifo_read(path: &str, nonblock: bool) -> FifoOpenResult {
    let entry_arc = match FIFO_REGISTRY.get(path) {
        Some(e) => e,
        None => return FifoOpenResult::Error(2), // ENOENT
    };

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
        FifoOpenResult::Block
    }
}

/// Open a FIFO for writing
///
/// If no reader is present and O_NONBLOCK is not set, this will block.
/// If O_NONBLOCK is set and no reader is present, returns ENXIO.
pub fn open_fifo_write(path: &str, nonblock: bool) -> FifoOpenResult {
    let entry_arc = match FIFO_REGISTRY.get(path) {
        Some(e) => e,
        None => return FifoOpenResult::Error(2), // ENOENT
    };

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
        FifoOpenResult::Block
    }
}

/// Complete a blocked FIFO open after being woken
///
/// Returns the buffer if now ready, or Block if still waiting
pub fn complete_fifo_open(path: &str, for_write: bool) -> FifoOpenResult {
    let entry_arc = match FIFO_REGISTRY.get(path) {
        Some(e) => e,
        None => return FifoOpenResult::Error(2), // ENOENT
    };

    let entry = entry_arc.lock();

    // Check if the other end is now present
    if for_write {
        if entry.readers > 0 {
            if let Some(ref buffer) = entry.buffer {
                return FifoOpenResult::Ready(buffer.clone());
            }
        }
    } else {
        if entry.writers > 0 {
            if let Some(ref buffer) = entry.buffer {
                return FifoOpenResult::Ready(buffer.clone());
            }
        }
    }

    // Still waiting
    FifoOpenResult::Block
}

/// Close a FIFO read end
pub fn close_fifo_read(path: &str) {
    if let Some(entry_arc) = FIFO_REGISTRY.get(path) {
        entry_arc.lock().remove_reader();
    }
}

/// Close a FIFO write end
pub fn close_fifo_write(path: &str) {
    if let Some(entry_arc) = FIFO_REGISTRY.get(path) {
        entry_arc.lock().remove_writer();
    }
}

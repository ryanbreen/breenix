//! Process structure and lifecycle

use crate::memory::process_memory::ProcessPageTable;
use crate::memory::stack::GuardedStack;
use crate::signal::SignalState;
use crate::ipc::FdTable;
use crate::task::thread::Thread;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
#[cfg(target_arch = "x86_64")]
use x86_64::VirtAddr;
#[cfg(not(target_arch = "x86_64"))]
use crate::memory::arch_stub::VirtAddr;

/// Info about a framebuffer mmap'd into a process's address space.
/// The user buffer is a compact pane buffer (no cross-pane padding).
#[derive(Debug, Clone, Copy)]
pub struct FbMmapInfo {
    /// Userspace virtual address of the mapping
    pub user_addr: u64,
    /// Width in pixels (pane only)
    pub width: usize,
    /// Height in pixels
    pub height: usize,
    /// User buffer stride in bytes (width * bpp, compact)
    pub user_stride: usize,
    /// Bytes per pixel
    pub bpp: usize,
    /// Total mapping size in bytes (page-aligned)
    pub mapping_size: u64,
    /// Pixel X offset in the physical framebuffer (0 for left pane, width/2+4 for right pane)
    pub x_offset: usize,
}

/// Process ID type
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ProcessId(u64);

impl ProcessId {
    pub fn new(id: u64) -> Self {
        ProcessId(id)
    }

    pub fn as_u64(self) -> u64 {
        self.0
    }
}

/// Process state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessState {
    /// Process is being created
    Creating,
    /// Process is ready to run
    Ready,
    /// Process is currently running
    Running,
    /// Process is blocked waiting for something
    Blocked,
    /// Process has terminated
    Terminated(i32), // exit code
}

/// A process represents a running program with its own address space
pub struct Process {
    /// Unique process identifier
    #[allow(dead_code)]
    pub id: ProcessId,

    /// Process group ID (for job control)
    /// By default, a process's pgid equals its pid when created
    pub pgid: ProcessId,

    /// Session ID (for session management)
    /// A session is a collection of process groups, typically associated with
    /// a controlling terminal. Initially set to pid on process creation.
    pub sid: ProcessId,

    /// Current working directory (absolute path)
    pub cwd: String,

    /// Process name (for debugging)
    pub name: String,

    /// Current state
    pub state: ProcessState,

    /// Entry point address
    pub entry_point: VirtAddr,

    /// Main thread of the process
    pub main_thread: Option<Thread>,

    /// Additional threads (for future multi-threading support)
    #[allow(dead_code)]
    pub threads: Vec<u64>, // Thread IDs

    /// Parent process ID (if any)
    pub parent: Option<ProcessId>,

    /// Child processes
    pub children: Vec<ProcessId>,

    /// Exit code (if terminated)
    pub exit_code: Option<i32>,

    /// Memory usage statistics
    pub memory_usage: MemoryUsage,

    /// Stack allocated for this process
    pub stack: Option<Box<GuardedStack>>,

    /// Per-process page table
    pub page_table: Option<Box<ProcessPageTable>>,

    /// Heap start address (page-aligned, set from ELF segments_end)
    pub heap_start: u64,

    /// Current heap end (program break)
    pub heap_end: u64,

    /// Virtual memory areas for this process (mmap regions)
    #[allow(dead_code)]
    pub vmas: alloc::vec::Vec<crate::memory::vma::Vma>,

    /// Next hint address for mmap allocation (grows downward)
    #[allow(dead_code)]
    pub mmap_hint: u64,

    /// Signal handling state (pending, blocked, handlers)
    pub signals: SignalState,

    /// File descriptor table for this process
    pub fd_table: FdTable,

    /// Alarm deadline (tick count when SIGALRM should be delivered)
    pub alarm_deadline: Option<u64>,

    /// Interval timers for setitimer/getitimer (ITIMER_REAL, ITIMER_VIRTUAL, ITIMER_PROF)
    pub itimers: crate::signal::IntervalTimers,

    /// Thread group ID for futex keying. Threads created with CLONE_VM share
    /// the same thread_group_id so futexes at the same virtual address map to
    /// the same wait queue. None means use self.id.as_u64().
    pub thread_group_id: Option<u64>,

    /// Inherited CR3 value for CLONE_VM threads that share a parent's address space.
    /// When set, context_switch uses this CR3 instead of looking up page_table.
    pub inherited_cr3: Option<u64>,

    /// Address to write 0 to and futex-wake when this thread exits (CLONE_CHILD_CLEARTID).
    pub clear_child_tid: Option<u64>,

    /// Bottom of the user stack (lowest mapped address, grows downward via demand paging)
    pub user_stack_bottom: u64,

    /// Top of the user stack (highest address, fixed at allocation time)
    pub user_stack_top: u64,

    /// Old page tables from previous exec() calls, pending deferred cleanup.
    /// These cannot be freed immediately during exec because CR3 may still point
    /// to the old table when a timer interrupt fires. They are drained at the
    /// start of the next exec (by which point CR3 has definitely switched) or
    /// when the process exits.
    pub pending_old_page_tables: Vec<Box<ProcessPageTable>>,

    /// Framebuffer mmap info (if this process has an mmap'd framebuffer)
    pub fb_mmap: Option<FbMmapInfo>,

    /// Whether this process has taken over the display (called take_over_display syscall)
    pub has_display_ownership: bool,

    /// Accumulated CPU ticks for this process (for btop display)
    pub cpu_ticks: u64,
}

/// Memory usage tracking
#[derive(Debug, Default)]
pub struct MemoryUsage {
    /// Size of loaded program segments in bytes
    pub code_size: usize,
    /// Size of allocated heap in bytes
    #[allow(dead_code)]
    pub heap_size: usize,
    /// Size of allocated stack in bytes
    pub stack_size: usize,
}

impl Process {
    /// Create a new process
    pub fn new(id: ProcessId, name: String, entry_point: VirtAddr) -> Self {
        Process {
            id,
            // By default, a process's pgid equals its pid (process is its own group leader)
            pgid: id,
            // By default, a process's sid equals its pid (process is its own session leader)
            sid: id,
            // Default working directory is root
            cwd: String::from("/"),
            name,
            state: ProcessState::Creating,
            entry_point,
            main_thread: None,
            threads: Vec::new(),
            parent: None,
            children: Vec::new(),
            exit_code: None,
            memory_usage: MemoryUsage::default(),
            stack: None,
            page_table: None,
            heap_start: 0,
            heap_end: 0,
            vmas: alloc::vec::Vec::new(),
            mmap_hint: crate::memory::vma::MMAP_REGION_END,
            signals: SignalState::default(),
            fd_table: FdTable::new(),
            alarm_deadline: None,
            itimers: crate::signal::IntervalTimers::default(),
            thread_group_id: None,
            inherited_cr3: None,
            clear_child_tid: None,
            user_stack_bottom: 0,
            user_stack_top: 0,
            pending_old_page_tables: Vec::new(),
            fb_mmap: None,
            has_display_ownership: false,
            cpu_ticks: 0,
        }
    }

    /// Set the main thread for this process
    pub fn set_main_thread(&mut self, thread: Thread) {
        self.main_thread = Some(thread);
        self.state = ProcessState::Ready;
    }

    /// Mark process as running
    pub fn set_running(&mut self) {
        self.state = ProcessState::Running;
    }

    /// Mark process as blocked
    #[allow(dead_code)]
    pub fn set_blocked(&mut self) {
        self.state = ProcessState::Blocked;
    }

    /// Mark process as ready
    pub fn set_ready(&mut self) {
        self.state = ProcessState::Ready;
    }

    /// Terminate the process
    ///
    /// This sets the process state to Terminated and closes all file descriptors
    /// to properly release resources (e.g., decrement pipe reader/writer counts).
    /// Also cleans up Copy-on-Write frame references to avoid memory leaks.
    /// CRITICAL: Also marks the main thread as Terminated so the scheduler
    /// doesn't keep scheduling this thread after process termination.
    pub fn terminate(&mut self, exit_code: i32) {
        // Guard against double-terminate: if the process is already terminated,
        // skip all cleanup to prevent double-decrementing COW page refcounts
        // (which would free pages still mapped by other processes).
        if matches!(self.state, ProcessState::Terminated(_)) {
            return;
        }

        // Close all file descriptors before setting state to Terminated
        // This ensures pipe counts are properly decremented so readers get EOF
        self.close_all_fds();

        // Clean up Copy-on-Write frame references
        // This decrements refcounts for all pages and deallocates frames that are no longer shared
        self.cleanup_cow_frames();

        self.state = ProcessState::Terminated(exit_code);
        self.exit_code = Some(exit_code);

        // CRITICAL FIX: Mark the main thread as terminated so the scheduler
        // doesn't keep putting it back in the ready queue. The scheduler checks
        // thread state (not process state) when deciding whether to re-queue a thread.
        // Without this, a process terminated by signal would have its thread keep
        // getting scheduled forever in an infinite loop.
        if let Some(ref mut thread) = self.main_thread {
            thread.set_terminated();
            log::info!(
                "Process {} terminated (exit_code={}), marked thread {} as Terminated",
                self.id.as_u64(),
                exit_code,
                thread.id()
            );
        }
    }

    /// Close all file descriptors in this process
    ///
    /// This properly decrements pipe reader/writer counts, ensuring that
    /// when all writers close, readers get EOF instead of EAGAIN.
    #[cfg(target_arch = "x86_64")]
    fn close_all_fds(&mut self) {
        use crate::ipc::FdKind;

        log::debug!("Process::close_all_fds() for process '{}'", self.name);

        // Close each fd, which will decrement pipe counts
        for fd in 0..crate::ipc::MAX_FDS {
            if let Ok(fd_entry) = self.fd_table.close(fd as i32) {
                match fd_entry.kind {
                    FdKind::PipeRead(buffer) => {
                        buffer.lock().close_read();
                        log::debug!("Process::close_all_fds() - closed pipe read fd {}", fd);
                    }
                    FdKind::PipeWrite(buffer) => {
                        buffer.lock().close_write();
                        log::debug!("Process::close_all_fds() - closed pipe write fd {}", fd);
                    }
                    FdKind::UdpSocket(_) => {
                        // Socket cleanup handled by UdpSocket::Drop when Arc refcount reaches 0
                        log::debug!("Process::close_all_fds() - released UDP socket fd {}", fd);
                    }
                    FdKind::TcpSocket(_) => {
                        // Unbound TCP socket doesn't need cleanup
                        log::debug!("Process::close_all_fds() - released TCP socket fd {}", fd);
                    }
                    FdKind::TcpListener(port) => {
                        // Decrement ref count, remove only if it reaches 0
                        crate::net::tcp::tcp_listener_ref_dec(port);
                        log::debug!("Process::close_all_fds() - released TCP listener fd {} on port {}", fd, port);
                    }
                    FdKind::TcpConnection(conn_id) => {
                        // Close the TCP connection
                        let _ = crate::net::tcp::tcp_close(&conn_id);
                        log::debug!("Process::close_all_fds() - closed TCP connection fd {}", fd);
                    }
                    FdKind::StdIo(_) => {
                        // StdIo doesn't need cleanup
                    }
                    FdKind::RegularFile(_) => {
                        // Regular file cleanup handled by Arc refcount
                        log::debug!("Process::close_all_fds() - released regular file fd {}", fd);
                    }
                    FdKind::Directory(_) => {
                        // Directory cleanup handled by Arc refcount
                        log::debug!("Process::close_all_fds() - released directory fd {}", fd);
                    }
                    FdKind::Device(_) => {
                        // Device files don't need cleanup
                        log::debug!("Process::close_all_fds() - released device fd {}", fd);
                    }
                    FdKind::DevfsDirectory { .. } => {
                        // Devfs directory doesn't need cleanup
                        log::debug!("Process::close_all_fds() - released devfs directory fd {}", fd);
                    }
                    FdKind::DevptsDirectory { .. } => {
                        // Devpts directory doesn't need cleanup
                        log::debug!("Process::close_all_fds() - released devpts directory fd {}", fd);
                    }
                    FdKind::PtyMaster(pty_num) => {
                        // PTY master cleanup - decrement refcount, only release when all masters closed
                        if let Some(pair) = crate::tty::pty::get(pty_num) {
                            let old_count = pair.master_refcount.fetch_sub(1, core::sync::atomic::Ordering::SeqCst);
                            log::debug!("Process::close_all_fds() - PTY master fd {} (pty {}) refcount {} -> {}",
                                fd, pty_num, old_count, old_count - 1);
                            if old_count == 1 {
                                crate::tty::pty::release(pty_num);
                                log::debug!("Process::close_all_fds() - released PTY {} (last master closed)", pty_num);
                            }
                        }
                    }
                    FdKind::PtySlave(pty_num) => {
                        // Decrement slave refcount — master sees POLLHUP when last slave closes
                        if let Some(pair) = crate::tty::pty::get(pty_num) {
                            pair.slave_close();
                        }
                        log::debug!("Process::close_all_fds() - released PTY slave fd {}", fd);
                    }
                    FdKind::UnixStream(socket) => {
                        // Close Unix socket endpoint
                        socket.lock().close();
                        log::debug!("Process::close_all_fds() - closed Unix stream socket fd {}", fd);
                    }
                    FdKind::UnixSocket(_) => {
                        // Unbound/bound Unix socket doesn't need cleanup
                        log::debug!("Process::close_all_fds() - released Unix socket fd {}", fd);
                    }
                    FdKind::UnixListener(_) => {
                        // Unix listener socket cleanup handled by Arc refcount
                        log::debug!("Process::close_all_fds() - released Unix listener fd {}", fd);
                    }
                    FdKind::FifoRead(path, buffer) => {
                        // Close FIFO read end
                        crate::ipc::fifo::close_fifo_read(&path);
                        buffer.lock().close_read();
                        log::debug!("Process::close_all_fds() - closed FIFO read fd {} ({})", fd, path);
                    }
                    FdKind::FifoWrite(path, buffer) => {
                        // Close FIFO write end
                        crate::ipc::fifo::close_fifo_write(&path);
                        buffer.lock().close_write();
                        log::debug!("Process::close_all_fds() - closed FIFO write fd {} ({})", fd, path);
                    }
                    FdKind::ProcfsFile { .. } => {
                        // Procfs files are purely in-memory, nothing to clean up
                    }
                    FdKind::ProcfsDirectory { .. } => {
                        // Procfs directory doesn't need cleanup
                    }
                }
            }
        }
    }

    /// Close all file descriptors in this process (ARM64)
    #[cfg(not(target_arch = "x86_64"))]
    fn close_all_fds(&mut self) {
        use crate::ipc::FdKind;

        log::debug!("Process::close_all_fds() for process '{}'", self.name);

        // Close each fd, which will decrement pipe counts
        for fd in 0..crate::ipc::MAX_FDS {
            if let Ok(fd_entry) = self.fd_table.close(fd as i32) {
                match fd_entry.kind {
                    FdKind::PipeRead(buffer) => {
                        buffer.lock().close_read();
                        log::debug!("Process::close_all_fds() - closed pipe read fd {}", fd);
                    }
                    FdKind::PipeWrite(buffer) => {
                        buffer.lock().close_write();
                        log::debug!("Process::close_all_fds() - closed pipe write fd {}", fd);
                    }
                    FdKind::StdIo(_) => {
                        // StdIo doesn't need cleanup
                    }
                    FdKind::UdpSocket(_) => {
                        // UDP socket cleanup handled by Drop
                        log::debug!("Process::close_all_fds() - closed UDP socket fd {}", fd);
                    }
                    FdKind::UnixStream(_) => {
                        // Unix stream cleanup handled by Drop
                        log::debug!("Process::close_all_fds() - closed Unix stream fd {}", fd);
                    }
                    FdKind::UnixSocket(_) => {
                        // Unix socket cleanup handled by Drop
                        log::debug!("Process::close_all_fds() - closed Unix socket fd {}", fd);
                    }
                    FdKind::UnixListener(_) => {
                        // Unix listener cleanup handled by Drop
                        log::debug!("Process::close_all_fds() - closed Unix listener fd {}", fd);
                    }
                    FdKind::PtyMaster(pty_num) => {
                        // PTY master cleanup - decrement refcount, only release when all masters closed
                        if let Some(pair) = crate::tty::pty::get(pty_num) {
                            let old_count = pair.master_refcount.fetch_sub(1, core::sync::atomic::Ordering::SeqCst);
                            if old_count == 1 {
                                crate::tty::pty::release(pty_num);
                            }
                        }
                        log::debug!("Process::close_all_fds() - closed PTY master fd {}", fd);
                    }
                    FdKind::PtySlave(pty_num) => {
                        // Decrement slave refcount — master sees POLLHUP when last slave closes
                        if let Some(pair) = crate::tty::pty::get(pty_num) {
                            pair.slave_close();
                        }
                        log::debug!("Process::close_all_fds() - closed PTY slave fd {}", fd);
                    }
                    FdKind::RegularFile(_) => {
                        // Regular file cleanup handled by Arc refcount
                        log::debug!("Process::close_all_fds() - released regular file fd {}", fd);
                    }
                    FdKind::Directory(_) => {
                        // Directory cleanup handled by Arc refcount
                        log::debug!("Process::close_all_fds() - released directory fd {}", fd);
                    }
                    FdKind::Device(_) => {
                        // Device files don't need cleanup
                        log::debug!("Process::close_all_fds() - released device fd {}", fd);
                    }
                    FdKind::DevfsDirectory { .. } => {
                        // Devfs directory doesn't need cleanup
                        log::debug!("Process::close_all_fds() - released devfs directory fd {}", fd);
                    }
                    FdKind::DevptsDirectory { .. } => {
                        // Devpts directory doesn't need cleanup
                        log::debug!("Process::close_all_fds() - released devpts directory fd {}", fd);
                    }
                    FdKind::FifoRead(path, buffer) => {
                        // Close FIFO read end
                        crate::ipc::fifo::close_fifo_read(&path);
                        buffer.lock().close_read();
                        log::debug!("Process::close_all_fds() - closed FIFO read fd {} ({})", fd, path);
                    }
                    FdKind::FifoWrite(path, buffer) => {
                        // Close FIFO write end
                        crate::ipc::fifo::close_fifo_write(&path);
                        buffer.lock().close_write();
                        log::debug!("Process::close_all_fds() - closed FIFO write fd {} ({})", fd, path);
                    }
                    FdKind::TcpSocket(_) => {
                        // Unbound TCP socket doesn't need cleanup
                        log::debug!("Process::close_all_fds() - closed TCP socket fd {}", fd);
                    }
                    FdKind::TcpListener(port) => {
                        // Decrement ref count, remove only if it reaches 0
                        crate::net::tcp::tcp_listener_ref_dec(port);
                        log::debug!("Process::close_all_fds() - released TCP listener fd {} port {}", fd, port);
                    }
                    FdKind::TcpConnection(conn_id) => {
                        // Close TCP connection
                        let _ = crate::net::tcp::tcp_close(&conn_id);
                        log::debug!("Process::close_all_fds() - closed TCP connection fd {}", fd);
                    }
                    FdKind::ProcfsFile { .. } => {
                        // Procfs files are purely in-memory, nothing to clean up
                    }
                    FdKind::ProcfsDirectory { .. } => {
                        // Procfs directory doesn't need cleanup
                    }
                }
            }
        }
    }

    /// Clean up Copy-on-Write frame references when process exits
    ///
    /// Walks all user pages in the process's page table and decrements their
    /// reference counts. Frames that are no longer shared (refcount reaches 0)
    /// are returned to the frame allocator for reuse.
    #[cfg(target_arch = "x86_64")]
    fn cleanup_cow_frames(&mut self) {
        use crate::memory::frame_allocator::deallocate_frame;
        use crate::memory::frame_metadata::frame_decref;
        use x86_64::structures::paging::{PageTableFlags, PhysFrame};

        // Get the page table for this process
        let page_table = match self.page_table.as_ref() {
            Some(pt) => pt,
            None => {
                log::debug!(
                    "Process {}: No page table to clean up",
                    self.id.as_u64()
                );
                return;
            }
        };

        let mut freed_count = 0;
        let mut shared_count = 0;

        // Walk all user pages and decrement refcounts
        let _ = page_table.walk_mapped_pages(|_virt_addr, phys_addr, flags| {
            // Only process user-accessible pages
            if !flags.contains(PageTableFlags::USER_ACCESSIBLE) {
                return;
            }

            let frame = PhysFrame::containing_address(phys_addr);

            // Decrement reference count.
            // Returns true if the frame should be freed:
            // - Tracked frame whose refcount reached 0 (was shared, now sole owner exiting)
            // - Untracked frame (private to this process, never shared via CoW)
            // Returns false if still shared (refcount > 0 after decrement).
            if frame_decref(frame) {
                deallocate_frame(frame);
                freed_count += 1;
            } else {
                shared_count += 1;
            }
        });

        if freed_count > 0 || shared_count > 0 {
            log::debug!(
                "Process {}: CoW cleanup - freed {} frames, {} still shared",
                self.id.as_u64(),
                freed_count,
                shared_count
            );
        }
    }

    /// Clean up Copy-on-Write frame references when process exits (ARM64)
    ///
    /// Walks all user pages in the process's page table and decrements their
    /// reference counts. Frames that are no longer shared (refcount reaches 0)
    /// are returned to the frame allocator for reuse.
    #[cfg(not(target_arch = "x86_64"))]
    fn cleanup_cow_frames(&mut self) {
        use crate::memory::frame_allocator::deallocate_frame;
        use crate::memory::frame_metadata::frame_decref;
        use crate::memory::arch_stub::{PageTableFlags, PhysFrame};

        // Get the page table for this process
        let page_table = match self.page_table.as_ref() {
            Some(pt) => pt,
            None => {
                log::debug!(
                    "Process {}: No page table to clean up",
                    self.id.as_u64()
                );
                return;
            }
        };

        let mut freed_count = 0;
        let mut shared_count = 0;

        // Walk all user pages and decrement refcounts
        let _ = page_table.walk_mapped_pages(|_virt_addr, phys_addr, flags| {
            // Only process user-accessible pages
            if !flags.contains(PageTableFlags::USER_ACCESSIBLE) {
                return;
            }

            let frame = PhysFrame::containing_address(phys_addr);

            // Decrement reference count.
            // Returns true if the frame should be freed:
            // - Tracked frame whose refcount reached 0 (was shared, now sole owner exiting)
            // - Untracked frame (private to this process, never shared via CoW)
            // Returns false if still shared (refcount > 0 after decrement).
            if frame_decref(frame) {
                deallocate_frame(frame);
                freed_count += 1;
            } else {
                shared_count += 1;
            }
        });

        if freed_count > 0 || shared_count > 0 {
            log::debug!(
                "Process {}: CoW cleanup - freed {} frames, {} still shared",
                self.id.as_u64(),
                freed_count,
                shared_count
            );
        }
    }

    /// Drain and clean up any pending old page tables from previous exec() calls.
    ///
    /// This is safe to call once CR3 has definitely switched away from the old
    /// page table (e.g., at the start of the next exec, or during process exit).
    /// Each old page table has its user-space frames freed via `cleanup_for_exec()`.
    pub fn drain_old_page_tables(&mut self) {
        if !self.pending_old_page_tables.is_empty() {
            let count = self.pending_old_page_tables.len();
            for old_pt in self.pending_old_page_tables.drain(..) {
                old_pt.cleanup_for_exec();
            }
            log::debug!(
                "Process {}: drained {} pending old page table(s)",
                self.id.as_u64(),
                count
            );
        }
    }

    /// Check if process is terminated
    pub fn is_terminated(&self) -> bool {
        matches!(self.state, ProcessState::Terminated(_))
    }

    /// Add a child process
    #[allow(dead_code)]
    pub fn add_child(&mut self, child_id: ProcessId) {
        self.children.push(child_id);
    }

    /// Remove a child process
    #[allow(dead_code)]
    pub fn remove_child(&mut self, child_id: ProcessId) {
        self.children.retain(|&id| id != child_id);
    }

    /// Get the process ID
    #[allow(dead_code)]
    pub fn pid(&self) -> ProcessId {
        self.id
    }

    /// Get a reference to the page table
    #[allow(dead_code)]
    pub fn page_table(&self) -> Option<&ProcessPageTable> {
        self.page_table.as_ref().map(|b| b.as_ref())
    }

    /// Get the CR3 value for this process.
    /// Returns the page table's physical frame address, falling back to
    /// inherited_cr3 for CLONE_VM threads that share a parent's address space.
    #[cfg(target_arch = "x86_64")]
    pub fn cr3_value(&self) -> Option<u64> {
        if let Some(ref pt) = self.page_table {
            Some(pt.level_4_frame().start_address().as_u64())
        } else {
            self.inherited_cr3
        }
    }

    /// Get the CR3 value for this process (ARM64).
    #[cfg(not(target_arch = "x86_64"))]
    pub fn cr3_value(&self) -> Option<u64> {
        if let Some(ref pt) = self.page_table {
            Some(pt.level_4_frame().start_address().as_u64())
        } else {
            self.inherited_cr3
        }
    }

    /// Get mutable access to VMA list
    #[allow(dead_code)]
    pub fn vma_list_mut(&mut self) -> &mut alloc::vec::Vec<crate::memory::vma::Vma> {
        &mut self.vmas
    }
}

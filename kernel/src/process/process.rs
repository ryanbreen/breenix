//! Process structure and lifecycle

use crate::ipc::FdTable;
#[cfg(not(target_arch = "x86_64"))]
use crate::memory::arch_stub::VirtAddr;
use crate::memory::process_memory::ProcessPageTable;
use crate::memory::stack::GuardedStack;
use crate::signal::SignalState;
use crate::task::thread::Thread;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
#[cfg(target_arch = "x86_64")]
use x86_64::VirtAddr;

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

/// Where the row sits in the reap/tombstone lifetime.
///
/// P6a deviation **D-1**: `RowState` is a *derived accessor* over the facts the
/// row already carries — `state`, `reaped` — and never a stored field. A second
/// stored copy of "has this row terminated" would be a second authority, and
/// `Process::is_terminated()` (which `ProcessManager::any_live_root_matches`
/// relies on to keep the two-event join from deadlocking against RootProof) is
/// itself derived from this accessor, so the two cannot disagree. It also keeps
/// the join off `OPAQUE_THREAD_STATE_STORES`: there is no `row.state = computed`
/// store for a raw write to launder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowState {
    /// The row has not terminated.
    Live,
    /// Terminated, not yet reaped: `waitpid` must still be able to collect it.
    Zombie,
    /// Terminated and reaped: invisible to every live-process query.
    Tombstone,
}

/// Phase-2 exit-obligation state. The PM lock is the sole serializer for
/// every transition; later teardown phases extend this exact shape rather
/// than upgrading a boolean in place.
#[derive(Clone, Copy)]
pub(crate) enum ExitObligationState {
    Absent,
    Pending,
    Claimed { claimer: u64, fence: ExitClaimFence },
    Completed,
}

#[derive(Clone, Copy)]
pub(crate) struct ExitClaimFence {
    #[cfg(target_arch = "aarch64")]
    retirement: crate::task::scheduler::RetirementFence,
}

impl ExitClaimFence {
    fn capture() -> Self {
        Self {
            #[cfg(target_arch = "aarch64")]
            retirement: crate::task::scheduler::retirement_grace_target(),
        }
    }
}

/// P2's durable notification seed: SIGCHLD is a class-A obligation completed
/// with its PM-owned effect, while Report uses T1/T2/T3 around the unchanged
/// `btrt::on_process_exit` effect outside PM. T4 intentionally does not exist.
pub(crate) struct ExitNotificationObligations {
    pub(crate) sigchld: ExitObligationState,
    report: ExitObligationState,
}

impl ExitNotificationObligations {
    const fn new() -> Self {
        Self {
            sigchld: ExitObligationState::Absent,
            report: ExitObligationState::Absent,
        }
    }

    /// T1: create each obligation exactly once, on the first exit commit.
    pub(crate) fn seed(&mut self) {
        if matches!(self.sigchld, ExitObligationState::Absent) {
            self.sigchld = ExitObligationState::Pending;
        }
        if matches!(self.report, ExitObligationState::Absent) {
            self.report = ExitObligationState::Pending;
        }
    }

    /// Class-A T2.3: the caller performs the SIGCHLD effect in the same PM
    /// acquisition before marking the obligation complete.
    pub(crate) fn complete_sigchld(&mut self) {
        if matches!(self.sigchld, ExitObligationState::Pending) {
            self.sigchld = ExitObligationState::Completed;
        }
    }

    /// T2: claim the report effect under PM. Exactly one competing exit path
    /// can observe Pending and become the sole redeemer.
    pub(crate) fn claim_report(&mut self, claimer: u64) -> bool {
        if !matches!(self.report, ExitObligationState::Pending) {
            return false;
        }
        self.report = ExitObligationState::Claimed {
            claimer,
            fence: ExitClaimFence::capture(),
        };
        true
    }

    /// T3: only the path that claimed the report may complete it, under a
    /// fresh PM acquisition after the effect ran outside PM.
    pub(crate) fn complete_report(&mut self, claimer: u64) {
        match self.report {
            ExitObligationState::Claimed {
                claimer: owner,
                fence,
            } if owner == claimer => {
                #[cfg(target_arch = "aarch64")]
                let _claim_fence = fence.retirement;
                #[cfg(not(target_arch = "aarch64"))]
                let _claim_fence = fence;
                self.report = ExitObligationState::Completed;
            }
            ExitObligationState::Claimed { .. } => {
                crate::trace_count!(crate::tracing::providers::teardown::LEDGER_CLAIM_MISMATCH);
            }
            _ => {}
        }
    }
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

    /// Real user ID
    pub uid: u32,
    /// Real group ID
    pub gid: u32,
    /// Effective user ID
    pub euid: u32,
    /// Effective group ID
    pub egid: u32,
    /// File creation mask (umask)
    pub umask: u32,

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

    /// The reap half of the two-event join: `(reaper, status)`, written exactly
    /// once by `claim_reap` under the process-manager lock. Deliberately private
    /// — the only writer is the join's own claim, so no raw field store can
    /// launder a row into a tombstone.
    reaped: Option<(ProcessId, i32)>,

    /// The retirement half of the two-event join, latched exactly once by
    /// `mark_retired` when this row's deferred resources have been reclaimed.
    /// Private for the same reason `reaped` is.
    retired: bool,

    /// Durable P2 notification state, serialized exclusively by the PM lock.
    pub(crate) exit_notifications: ExitNotificationObligations,

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
            // Single-user OS: everything runs as root (uid=0, gid=0)
            uid: 0,
            gid: 0,
            euid: 0,
            egid: 0,
            // Standard default umask: owner rwx, group/other rx
            umask: 0o022,
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
            reaped: None,
            retired: false,
            exit_notifications: ExitNotificationObligations::new(),
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

    /// Attach the main thread while the row is still `Creating`. The row is only
    /// marked `Ready` once it has been published into the manager, so no runnable
    /// thread can ever refer to a row that does not yet exist.
    pub fn attach_main_thread_unpublished(&mut self, thread: Thread) {
        self.main_thread = Some(thread);
    }

    /// A row may acquire a new CLONE_VM group member only while it is live. A
    /// `Creating` row has not finished publication (both exits from `Creating` -
    /// `set_main_thread` and `set_ready` - write `Ready`), and a `Terminated` row is
    /// already leaving; neither may gain a member behind the publisher's back.
    pub fn admits_clone(&self) -> bool {
        match self.state {
            ProcessState::Creating => false,
            ProcessState::Ready | ProcessState::Running | ProcessState::Blocked => true,
            ProcessState::Terminated(_) => false,
        }
    }

    /// A row whose publication has not completed must never have its address space
    /// armed. Cheap field read: no lock, no allocation, no formatting, safe to call
    /// from the dispatch path.
    pub fn is_unpublished(&self) -> bool {
        match self.state {
            ProcessState::Creating => true,
            ProcessState::Ready => false,
            ProcessState::Running => false,
            ProcessState::Blocked => false,
            ProcessState::Terminated(_) => false,
        }
    }

    /// Mark process as running
    pub fn set_running(&mut self) {
        self.state = ProcessState::Running;
    }

    /// Mark process as blocked
    pub fn set_blocked(&mut self) {
        self.state = ProcessState::Blocked;
    }

    /// Mark process as ready
    pub fn set_ready(&mut self) {
        self.state = ProcessState::Ready;
    }

    #[cfg(feature = "boot_tests")]
    pub fn force_unpublished_for_test(&mut self) {
        self.state = ProcessState::Creating;
    }

    /// Terminate the process
    ///
    /// This sets the process state to Terminated and closes all file descriptors
    /// to properly release resources (e.g., decrement pipe reader/writer counts).
    /// Also cleans up Copy-on-Write frame references to avoid memory leaks.
    /// CRITICAL: Also marks the main thread as Terminated so the scheduler
    /// doesn't keep scheduling this thread after process termination.
    ///
    /// NOTE: This method does FD cleanup and CoW cleanup inline, which means
    /// it acquires pipe locks, scheduler locks, and frame metadata locks.
    /// For `handle_thread_exit`, use `terminate_minimal()` + deferred cleanup
    /// to reduce PM lock hold time on ARM64 SMP.
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
        // Record at the terminated-state transition so fault and signal deaths
        // count too. The guard above makes this exactly once per process. This
        // is safe under PROCESS_MANAGER: record_exit allocates/logs nothing and
        // takes only its leaf spin mutex; that mutex never nests PROCESS_MANAGER.
        crate::task::exit_tally::record_exit(&self.name, exit_code);

        // CRITICAL FIX: Mark the main thread as terminated so the scheduler
        // doesn't keep putting it back in the ready queue. The scheduler checks
        // thread state (not process state) when deciding whether to re-queue a thread.
        // Without this, a process terminated by signal would have its thread keep
        // getting scheduled forever in an infinite loop.
        if let Some(ref mut thread) = self.main_thread {
            thread.set_terminated();
        }
    }

    /// Minimal terminate: mark process and thread as terminated without cleanup.
    ///
    /// Used by `handle_thread_exit` to mark the process as terminated under PM lock,
    /// then perform FD closure and CoW cleanup OUTSIDE the PM lock. This prevents
    /// a system-wide hang on ARM64 SMP where logging, pipe wakeups, and scheduler
    /// calls inside close_all_fds create lock ordering violations with the serial
    /// output lock and framebuffer lock while all CPUs have interrupts disabled.
    pub fn terminate_minimal(&mut self, exit_code: i32) {
        if matches!(self.state, ProcessState::Terminated(_)) {
            return;
        }
        self.state = ProcessState::Terminated(exit_code);
        self.exit_code = Some(exit_code);
        // Record at the terminated-state transition so fault and signal deaths
        // count too. The guard above makes this exactly once per process. This
        // is safe under PROCESS_MANAGER: record_exit allocates/logs nothing and
        // takes only its leaf spin mutex; that mutex never nests PROCESS_MANAGER.
        crate::task::exit_tally::record_exit(&self.name, exit_code);
        if let Some(ref mut thread) = self.main_thread {
            thread.set_terminated();
        }
    }

    /// Extract all file descriptor entries for deferred cleanup outside PM lock.
    ///
    /// Returns the FD entries without closing them — the caller is responsible
    /// for pipe close_read/close_write, PTY refcounting, etc.
    pub fn take_fd_entries(&mut self) -> alloc::vec::Vec<(usize, crate::ipc::fd::FileDescriptor)> {
        let entries = self.fd_table.take_all();
        if crate::process::process_manager_held_on_current_cpu() {
            crate::tracing::providers::teardown::FD_CLOSES_UNDER_PM.add(entries.len() as u64);
        }
        entries
    }

    /// Close all file descriptors in this process
    ///
    /// This properly decrements pipe reader/writer counts, ensuring that
    /// when all writers close, readers get EOF instead of EAGAIN.
    ///
    /// CRITICAL: No logging in this function — it runs under PM lock where
    /// log calls create lock ordering violations (PM → SERIAL → framebuffer).
    #[cfg(target_arch = "x86_64")]
    fn close_all_fds(&mut self) {
        use crate::ipc::FdKind;

        for fd in 0..crate::ipc::MAX_FDS {
            if let Ok(fd_entry) = self.fd_table.close(fd as i32) {
                if crate::process::process_manager_held_on_current_cpu() {
                    crate::trace_count!(crate::tracing::providers::teardown::FD_CLOSES_UNDER_PM);
                }
                match fd_entry.kind {
                    FdKind::PipeRead(buffer) => {
                        buffer.lock().close_read();
                    }
                    FdKind::PipeWrite(buffer) => {
                        buffer.lock().close_write();
                    }
                    FdKind::TcpListener(port) => {
                        crate::net::tcp::tcp_listener_ref_dec(port);
                    }
                    FdKind::TcpConnection(conn_id) => {
                        let _ = crate::net::tcp::tcp_close(&conn_id);
                    }
                    FdKind::PtyMaster(pty_num) => {
                        if let Some(pair) = crate::tty::pty::get(pty_num) {
                            let old_count = pair
                                .master_refcount
                                .fetch_sub(1, core::sync::atomic::Ordering::SeqCst);
                            if old_count == 1 {
                                crate::tty::pty::release(pty_num);
                            }
                        }
                    }
                    FdKind::PtySlave(pty_num) => {
                        if let Some(pair) = crate::tty::pty::get(pty_num) {
                            pair.slave_close();
                        }
                    }
                    FdKind::UnixStream(socket) => {
                        socket.lock().close();
                    }
                    FdKind::FifoRead(path, buffer) => {
                        crate::ipc::fifo::close_fifo_read(&path);
                        buffer.lock().close_read();
                    }
                    FdKind::FifoWrite(path, buffer) => {
                        crate::ipc::fifo::close_fifo_write(&path);
                        buffer.lock().close_write();
                    }
                    _ => {} // StdIo, RegularFile, Directory, Device, etc. — no action needed
                }
            }
        }
    }

    /// Close all file descriptors in this process (ARM64)
    ///
    /// CRITICAL: No logging in this function — it runs under PM lock where
    /// log calls create lock ordering violations (PM → SERIAL → framebuffer).
    #[cfg(not(target_arch = "x86_64"))]
    fn close_all_fds(&mut self) {
        use crate::ipc::FdKind;

        for fd in 0..crate::ipc::MAX_FDS {
            if let Ok(fd_entry) = self.fd_table.close(fd as i32) {
                if crate::process::process_manager_held_on_current_cpu() {
                    crate::trace_count!(crate::tracing::providers::teardown::FD_CLOSES_UNDER_PM);
                }
                match fd_entry.kind {
                    FdKind::PipeRead(buffer) => {
                        buffer.lock().close_read();
                    }
                    FdKind::PipeWrite(buffer) => {
                        buffer.lock().close_write();
                    }
                    FdKind::TcpListener(port) => {
                        crate::net::tcp::tcp_listener_ref_dec(port);
                    }
                    FdKind::TcpConnection(conn_id) => {
                        let _ = crate::net::tcp::tcp_close(&conn_id);
                    }
                    FdKind::PtyMaster(pty_num) => {
                        if let Some(pair) = crate::tty::pty::get(pty_num) {
                            let old_count = pair
                                .master_refcount
                                .fetch_sub(1, core::sync::atomic::Ordering::SeqCst);
                            if old_count == 1 {
                                crate::tty::pty::release(pty_num);
                            }
                        }
                    }
                    FdKind::PtySlave(pty_num) => {
                        if let Some(pair) = crate::tty::pty::get(pty_num) {
                            pair.slave_close();
                        }
                    }
                    FdKind::UnixStream(socket) => {
                        socket.lock().close();
                    }
                    FdKind::FifoRead(path, buffer) => {
                        crate::ipc::fifo::close_fifo_read(&path);
                        buffer.lock().close_read();
                    }
                    FdKind::FifoWrite(path, buffer) => {
                        crate::ipc::fifo::close_fifo_write(&path);
                        buffer.lock().close_write();
                    }
                    _ => {} // StdIo, RegularFile, Directory, Device, etc. — no action needed
                }
            }
        }
    }

    /// Clean up Copy-on-Write frame references when process exits
    ///
    /// Walks all user pages in the process's page table and decrements their
    /// reference counts. Frames that are no longer shared (refcount reaches 0)
    /// are returned to the frame allocator for reuse.
    pub(crate) fn cleanup_cow_frames(&mut self) {
        if let Some(page_table) = self.page_table.as_mut() {
            page_table.release_mapped_leaves();
        }
    }

    /// Retire pending superseded address spaces within a shared per-frame budget.
    /// Incomplete tables stay pending so a later pass can resume their custody
    /// release once the old hardware root is no longer live.
    pub(crate) fn drain_old_page_tables_bounded(&mut self, budget: &mut u32) -> bool {
        while *budget > 0 {
            let Some(old_page_table) = self.pending_old_page_tables.last_mut() else {
                return true;
            };
            if old_page_table.cleanup_for_exec(self.id.as_u64(), budget)
                != crate::memory::process_memory::RetireProgress::Complete
            {
                return false;
            }
            self.pending_old_page_tables.pop();
        }
        self.pending_old_page_tables.is_empty()
    }

    pub fn drain_old_page_tables(&mut self) {
        let mut budget = crate::memory::process_memory::RETIRE_FRAME_BUDGET;
        let _ = self.drain_old_page_tables_bounded(&mut budget);
    }

    /// The row's lifetime state. **The single authority** for P6a: every other
    /// predicate on this file derives from it rather than re-reading `state`.
    pub fn row_state(&self) -> RowState {
        match self.state {
            ProcessState::Creating
            | ProcessState::Ready
            | ProcessState::Running
            | ProcessState::Blocked => RowState::Live,
            ProcessState::Terminated(_) => match self.reaped {
                None => RowState::Zombie,
                Some(_) => RowState::Tombstone,
            },
        }
    }

    /// Check if process is terminated.
    ///
    /// Derived from `row_state()`, and **a tombstone is terminated**: the
    /// `!is_terminated()` filter in `any_live_root_matches` is what keeps a
    /// reaped-but-unretired row from blocking its own retirement. Inverting this
    /// reintroduces the retire-waits-for-row / row-waits-for-retire cycle.
    pub fn is_terminated(&self) -> bool {
        matches!(self.row_state(), RowState::Zombie | RowState::Tombstone)
    }

    /// A reaped row. Live-process queries must not see it.
    pub fn is_tombstone(&self) -> bool {
        matches!(self.row_state(), RowState::Tombstone)
    }

    /// Reap arm of the two-event join: record `(reaper, status)` exactly once.
    ///
    /// Write-once by construction — a second reaper of the same row reads the
    /// existing claim and changes nothing — and refused on a row that has not
    /// terminated, so no live row can be tombstoned by an out-of-order claim.
    pub(crate) fn claim_reap(&mut self, reaper: ProcessId, status: i32) {
        if self.reaped.is_some() || !self.is_terminated() {
            return;
        }
        self.reaped = Some((reaper, status));
    }

    /// Retirement arm of the two-event join: latch that this row's deferred
    /// resources have been reclaimed. Write-once, and refused on a row that has
    /// not terminated so a receipt naming a still-live row cannot latch it.
    pub(crate) fn mark_retired(&mut self) {
        if self.retired || !self.is_terminated() {
            return;
        }
        self.retired = true;
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

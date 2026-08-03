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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitOutcome {
    Committed,
    AlreadyCommitted,
    Missing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitStage {
    Live,
    ExitCommitted,
    Reclaimed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExitWorkBits(u32);

impl ExitWorkBits {
    pub const fn empty() -> Self {
        Self(0)
    }

    pub const REPARENT_CHILDREN: Self = Self(1 << 0);
    pub const NOTIFY_PARENT: Self = Self(1 << 1);
    pub const CLEANUP_GRAPHICS: Self = Self(1 << 2);
    pub const CLOSE_FDS: Self = Self(1 << 3);

    pub const fn contains(self, work: Self) -> bool {
        self.0 & work.0 == work.0
    }

    pub fn insert(&mut self, work: Self) {
        self.0 |= work.0;
    }

    pub fn remove(&mut self, work: Self) {
        self.0 &= !work.0;
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
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

    /// Child processes. AArch64 derives this relation from child.parent.
    #[cfg(target_arch = "x86_64")]
    pub children: Vec<ProcessId>,

    /// Exit code (if terminated)
    pub exit_code: Option<i32>,

    #[cfg(target_arch = "aarch64")]
    pub exit_stage: ExitStage,
    #[cfg(target_arch = "aarch64")]
    pub exit_work_bits: ExitWorkBits,
    #[cfg(target_arch = "aarch64")]
    pub reaped: bool,
    #[cfg(target_arch = "aarch64")]
    pub retired_root: Option<u64>,

    /// Memory usage statistics
    pub memory_usage: MemoryUsage,

    /// Stack allocated for this process
    pub stack: Option<Box<GuardedStack>>,

    /// Per-process page table
    pub page_table: Option<Box<ProcessPageTable>>,

    /// Preallocated receiver for heavy resources at the exit commit point.
    #[cfg(target_arch = "aarch64")]
    pub grave: Option<Box<crate::task::reclaim::ProcessGrave>>,

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
    /// to the old table when a timer interrupt fires. They are drained outside
    /// the PM lock at the start of the next exec, or move into the process grave
    /// if the process exits first.
    pub pending_old_page_tables: Vec<Box<ProcessPageTable>>,

    /// Framebuffer mmap info (if this process has an mmap'd framebuffer)
    pub fb_mmap: Option<FbMmapInfo>,

    /// Whether this process has taken over the display (called take_over_display syscall)
    pub has_display_ownership: bool,

    /// Whether this process may own entries in the AArch64 window registry.
    #[cfg(target_arch = "aarch64")]
    pub has_window_buffers: bool,

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
    pub fn new(
        id: ProcessId,
        name: String,
        entry_point: VirtAddr,
        #[cfg(target_arch = "aarch64")]
        grave: Box<crate::task::reclaim::ProcessGrave>,
    ) -> Self {
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
            #[cfg(target_arch = "x86_64")]
            children: Vec::new(),
            exit_code: None,
            #[cfg(target_arch = "aarch64")]
            exit_stage: ExitStage::Live,
            #[cfg(target_arch = "aarch64")]
            exit_work_bits: ExitWorkBits::empty(),
            #[cfg(target_arch = "aarch64")]
            reaped: false,
            #[cfg(target_arch = "aarch64")]
            retired_root: None,
            memory_usage: MemoryUsage::default(),
            stack: None,
            page_table: None,
            #[cfg(target_arch = "aarch64")]
            grave: Some(grave),
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
            #[cfg(target_arch = "aarch64")]
            has_window_buffers: false,
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

    #[cfg(target_arch = "x86_64")]
    pub fn terminate(&mut self, exit_code: i32) {
        if matches!(self.state, ProcessState::Terminated(_)) {
            return;
        }

        self.close_all_fds();
        self.cleanup_cow_frames();
        self.state = ProcessState::Terminated(exit_code);
        self.exit_code = Some(exit_code);

        if let Some(ref mut thread) = self.main_thread {
            thread.set_terminated();
        }
    }

    #[cfg(target_arch = "x86_64")]
    pub fn terminate_minimal(&mut self, exit_code: i32) {
        if matches!(self.state, ProcessState::Terminated(_)) {
            return;
        }
        self.state = ProcessState::Terminated(exit_code);
        self.exit_code = Some(exit_code);
        if let Some(ref mut thread) = self.main_thread {
            thread.set_terminated();
        }
    }

    #[cfg(target_arch = "x86_64")]
    pub fn take_fd_entries(&mut self) -> alloc::vec::Vec<(usize, crate::ipc::fd::FileDescriptor)> {
        self.fd_table.take_all()
    }

    #[cfg(target_arch = "x86_64")]
    fn close_all_fds(&mut self) {
        use crate::ipc::FdKind;

        for fd in 0..crate::ipc::MAX_FDS {
            if let Ok(fd_entry) = self.fd_table.close(fd as i32) {
                match fd_entry.kind {
                    FdKind::PipeRead(buffer) => buffer.lock().close_read(),
                    FdKind::PipeWrite(buffer) => buffer.lock().close_write(),
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
                    FdKind::UnixStream(socket) => socket.lock().close(),
                    FdKind::FifoRead(path, buffer) => {
                        crate::ipc::fifo::close_fifo_read(&path);
                        buffer.lock().close_read();
                    }
                    FdKind::FifoWrite(path, buffer) => {
                        crate::ipc::fifo::close_fifo_write(&path);
                        buffer.lock().close_write();
                    }
                    _ => {}
                }
            }
        }
    }

    #[cfg(target_arch = "x86_64")]
    pub(crate) fn cleanup_cow_frames(&mut self) {
        use crate::memory::frame_allocator::deallocate_frame;
        use crate::memory::frame_metadata::frame_decref;
        use x86_64::structures::paging::{PageTableFlags, PhysFrame};

        let page_table = match self.page_table.as_ref() {
            Some(page_table) => page_table,
            None => return,
        };

        let _ = page_table.walk_mapped_pages(|_virt_addr, phys_addr, flags| {
            if !flags.contains(PageTableFlags::USER_ACCESSIBLE) {
                return;
            }

            let frame = PhysFrame::containing_address(phys_addr);
            if frame_decref(frame) {
                deallocate_frame(frame);
            }
        });
    }

    #[cfg(target_arch = "x86_64")]
    pub fn drain_old_page_tables(&mut self) {
        for old_page_table in self.pending_old_page_tables.drain(..) {
            old_page_table.cleanup_for_exec();
        }
    }

    #[cfg(target_arch = "aarch64")]
    pub fn mark_exit_committed(
        &mut self,
        exit_code: i32,
        work_bits: ExitWorkBits,
    ) -> ExitOutcome {
        if self.exit_stage != ExitStage::Live {
            return ExitOutcome::AlreadyCommitted;
        }
        self.exit_stage = ExitStage::ExitCommitted;
        self.exit_work_bits = work_bits;
        self.state = ProcessState::Terminated(exit_code);
        self.exit_code = Some(exit_code);
        ExitOutcome::Committed
    }

    /// Move every heavy resource into the grave allocated at process birth.
    #[cfg(target_arch = "aarch64")]
    pub fn commit_grave(
        &mut self,
        exit_code: i32,
        work_bits: ExitWorkBits,
    ) -> Option<Box<crate::task::reclaim::ProcessGrave>> {
        let retired_root = self.cr3_value();
        let mut grave = self.grave.take()?;
        grave.exit_code = exit_code;
        grave.page_table = self.page_table.take();
        core::mem::swap(
            &mut grave.old_page_tables,
            &mut self.pending_old_page_tables,
        );
        grave.stack = self.stack.take();
        #[cfg(target_arch = "aarch64")]
        {
            grave.fence = crate::task::scheduler::RetirementFence::capture();
        }
        let (secs, nanos) = crate::time::get_monotonic_time_ns();
        grave.queued_at_ns = secs.saturating_mul(1_000_000_000) + nanos;
        self.retired_root = retired_root;
        let outcome = self.mark_exit_committed(exit_code, work_bits);
        debug_assert_eq!(outcome, ExitOutcome::Committed);
        Some(grave)
    }

    #[cfg(target_arch = "aarch64")]
    pub fn is_exit_committed(&self) -> bool {
        self.exit_stage != ExitStage::Live
    }

    #[cfg(target_arch = "aarch64")]
    pub fn mark_reclaimed(&mut self) {
        self.exit_stage = ExitStage::Reclaimed;
    }

    #[cfg(target_arch = "aarch64")]
    pub fn mark_reaped(&mut self) {
        self.reaped = true;
    }

    #[cfg(target_arch = "aarch64")]
    pub fn can_remove_row(&self) -> bool {
        self.reaped && self.exit_stage == ExitStage::Reclaimed && self.exit_work_bits.is_empty()
    }

    /// Check if process is terminated
    pub fn is_terminated(&self) -> bool {
        matches!(self.state, ProcessState::Terminated(_))
    }

    #[cfg(target_arch = "x86_64")]
    #[allow(dead_code)]
    pub fn add_child(&mut self, child_id: ProcessId) {
        self.children.push(child_id);
    }

    #[cfg(target_arch = "x86_64")]
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

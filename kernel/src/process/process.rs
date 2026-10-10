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
    /// Width in pixels: the whole screen for the display owner, else the left half.
    /// The pane starts at x 0.
    pub width: usize,
    /// Height in pixels
    pub height: usize,
    /// User buffer stride in bytes (width * bpp, compact)
    pub user_stride: usize,
    /// Bytes per pixel
    pub bpp: usize,
    /// Total mapping size in bytes (page-aligned)
    pub mapping_size: u64,
    /// Mapped as the display owner (the whole screen). Draws through it are
    /// refused once another process has taken the display.
    pub whole_screen: bool,
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

/// A stop or continue a parent has not yet collected with `waitpid`
/// (`WUNTRACED`, `WCONTINUED`) or `waitid` (`WSTOPPED`, `WCONTINUED`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobReport {
    /// Stopped by the default action of this signal.
    Stopped(u32),
    /// Continued by SIGCONT.
    Continued,
}

/// Job-control state of one row. `stopped` is kept on every row of a thread
/// group, since each row's thread is held on its own; `report` and
/// `report_owed` live on the group's leader, the row its parent waits for.
/// Serialized by the process-manager lock.
#[derive(Debug, Clone, Copy, Default)]
pub struct JobControl {
    /// The signal that stopped the process, while it is stopped.
    pub stopped: Option<u32>,
    /// The latest stop or continue, until a wait reports it.
    pub report: Option<JobReport>,
    /// The stop has been taken but a thread of the group may still be running
    /// in user mode on another CPU; the parent is told once none is.
    pub report_owed: bool,
    /// This row's thread was blocked by the stop on its way to user mode, and
    /// SIGCONT makes it ready again. A thread stopped while it waits in a
    /// syscall is not parked: that wait goes on, and the thread is held at the
    /// syscall's return.
    pub parked: bool,
    /// The row's exit has looked for process groups it left orphaned.
    pub orphan_check_done: bool,
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
    pub limits: alloc::sync::Arc<super::limits::Limits>,
    pub memory_locks: crate::memory::locked::MemoryLocks,
    pub image_size: u64,
    pub image_data_size: u64,
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

    /// User and group IDs and supplementary groups.
    pub cred: super::credentials::ProcessCredentials,
    /// Nice value, -20 (most favoured) to 19. Reported and inherited; the
    /// scheduler does not yet weigh it.
    pub nice: i8,
    /// CPU time charged by this process's threads and by the children it has
    /// waited for, shared by a thread group.
    pub cpu: alloc::sync::Arc<crate::task::thread::CpuAccount>,
    /// File creation mask (umask)
    pub umask: u32,

    /// Current working directory, shared by the threads of a process. Its
    /// pathname is derived from the directory.
    pub cwd: crate::fs::namei::SharedWorkingDir,

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

    /// The retirement half of the two-event join, latched exactly once when this
    /// row's last outstanding retirement obligation ends. Private for the same
    /// reason `reaped` is.
    retired: bool,

    /// Retirement receipts created for this row and not yet settled.
    ///
    /// P6a condition **C4**: `retired` is a per-row latch but retirement is a
    /// per-*receipt* event, and the 1:1 mapping holds only incidentally today.
    /// Counting the outstanding receipts keys the latch on this row's own
    /// obligations: a second receipt naming the same row leaves the count at one
    /// after the first `record_reclaim`, so the join refuses to remove the row
    /// while an obligation is still outstanding. Fail-closed — a counted
    /// tombstone beats freeing a row out from under a live receipt.
    retirement_receipts: u32,

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

    /// Blocking FIFO opens of this row's thread that hold a reader or writer
    /// reference but no descriptor yet. `exit_process_and_retire` gives them
    /// back, since a SIGKILL does not return the parked opener through its
    /// open. See `PendingFifoOpen`.
    pub pending_fifo_opens: Vec<crate::ipc::fifo::PendingFifoOpen>,

    /// The status this row reports when a SIGKILL `kill_process_now` left
    /// pending ends it: that of the thread-group death that sent it (a member
    /// dying of SIGTERM takes its peers with -SIGTERM), or -SIGKILL for a
    /// plain kill. The first kill sets it.
    pub group_exit_code: Option<i32>,

    /// Interval timers for setitimer/getitimer (ITIMER_REAL, ITIMER_VIRTUAL, ITIMER_PROF)
    pub itimers: alloc::sync::Arc<crate::signal::IntervalTimers>,

    /// Thread group ID for futex keying. Threads created with CLONE_VM share
    /// the same thread_group_id so futexes at the same virtual address map to
    /// the same wait queue. None means use self.id.as_u64().
    pub thread_group_id: Option<u64>,

    /// The POSIX record-lock owner this row belongs to. Every row of a thread
    /// group shares one; a new process gets its own, whose id is its PID.
    pub lock_owner: alloc::sync::Arc<crate::fs::locks::LockOwner>,

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

    /// Job-control stop state and the stop or continue not yet waited for.
    pub job: JobControl,

    /// The process has run a successful exec since its fork, so its parent
    /// may no longer change its process group (setpgid's EACCES).
    pub has_exec: bool,

    /// `terminate` ended this row while another live row of its thread group
    /// still ran on the address space it owns, so it released nothing. The
    /// row's exit hook hands the address space to a live row of the group, or
    /// releases it once none is left (#1321).
    pub address_space_release_deferred: bool,

    /// The exit status of a fatal signal's default action whose deferred exit
    /// (`defer_fault_exit`) could not be queued: the per-CPU ring was full and
    /// the overflow list could not grow. The deferred-exit drain finds the
    /// row by this and ends its thread group as a queued exit would.
    pub unqueued_fatal_exit: Option<i32>,

    /// This row leads a thread group whose last row has ended, and its parent
    /// has been told (`ProcessManager::group_exited`): it is told once.
    pub group_exit_reported: bool,
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
            limits: super::limits::Limits::new(),
            memory_locks: crate::memory::locked::MemoryLocks::default(),
            image_size: 0,
            image_data_size: 0,
            id,
            // A process the kernel creates starts in init's process group and
            // session, which init (PID 1) leads, rather than leading its own:
            // like a child of init, it may then create its own group or session.
            // fork, spawn and clone give a child its parent's instead.
            pgid: ProcessId(super::RESERVED_INIT_PID),
            sid: ProcessId(super::RESERVED_INIT_PID),
            cred: super::credentials::ProcessCredentials::root(),
            nice: 0,
            cpu: alloc::sync::Arc::new(crate::task::thread::CpuAccount::new(id.as_u64())),
            // Standard default umask: owner rwx, group/other rx
            umask: 0o022,
            // Default working directory is root
            cwd: crate::fs::namei::SharedWorkingDir::default(),
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
            retirement_receipts: 0,
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
            pending_fifo_opens: Vec::new(),
            group_exit_code: None,
            itimers: alloc::sync::Arc::new(crate::signal::IntervalTimers::default()),
            thread_group_id: None,
            lock_owner: crate::fs::locks::LockOwner::new(id.as_u64()),
            inherited_cr3: None,
            clear_child_tid: None,
            user_stack_bottom: 0,
            user_stack_top: 0,
            pending_old_page_tables: Vec::new(),
            fb_mmap: None,
            has_display_ownership: false,
            cpu_ticks: 0,
            job: JobControl::default(),
            has_exec: false,
            address_space_release_deferred: false,
            unqueued_fatal_exit: None,
            group_exit_reported: false,
        }
    }

    /// Set the main thread for this process
    pub fn set_main_thread(&mut self, mut thread: Thread) {
        thread.resource_limits = Some(self.limits.clone());
        thread.cpu_account = Some(self.cpu.clone());
        thread.signals = self.signals.thread.clone();
        thread.signal_timers = Some(self.itimers.clone());
        self.main_thread = Some(thread);
        self.state = ProcessState::Ready;
    }

    /// Attach the main thread while the row is still `Creating`. The row is only
    /// marked `Ready` once it has been published into the manager, so no runnable
    /// thread can ever refer to a row that does not yet exist.
    pub fn attach_main_thread_unpublished(&mut self, mut thread: Thread) {
        thread.resource_limits = Some(self.limits.clone());
        thread.cpu_account = Some(self.cpu.clone());
        thread.signals = self.signals.thread.clone();
        thread.signal_timers = Some(self.itimers.clone());
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

        // Another live row of the thread group runs on the address space this
        // row owns: its frames stay until the row's exit hook hands them on.
        let address_space_shared = self.page_table.is_some() && self.lock_owner.live_rows() > 1;

        // Close all file descriptors before setting state to Terminated
        // This ensures pipe counts are properly decremented so readers get EOF
        self.close_all_fds();
        self.leave_record_locks();

        // Clean up Copy-on-Write frame references
        // This decrements refcounts for all pages and deallocates frames that are no longer shared
        if address_space_shared {
            self.address_space_release_deferred = true;
        } else {
            self.cleanup_cow_frames();
        }

        self.state = ProcessState::Terminated(exit_code);
        self.exit_code = Some(exit_code);
        // An exited row's queued realtime signals are never delivered.
        self.signals.release_queued();
        // A POSIX timer signal due for this thread and not yet queued goes
        // to another thread of the process.
        self.itimers.posix.release_due(&self.signals.thread);
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
        self.leave_record_locks();
        self.state = ProcessState::Terminated(exit_code);
        self.exit_code = Some(exit_code);
        // An exited row's queued realtime signals are never delivered.
        self.signals.release_queued();
        // A POSIX timer signal due for this thread and not yet queued goes
        // to another thread of the process.
        self.itimers.posix.release_due(&self.signals.thread);
        // Record at the terminated-state transition so fault and signal deaths
        // count too. The guard above makes this exactly once per process. This
        // is safe under PROCESS_MANAGER: record_exit allocates/logs nothing and
        // takes only its leaf spin mutex; that mutex never nests PROCESS_MANAGER.
        crate::task::exit_tally::record_exit(&self.name, exit_code);
        if let Some(ref mut thread) = self.main_thread {
            thread.set_terminated();
        }
    }

    /// Take the record of `tid`'s blocked open of `entry` out of this row.
    /// `None` means the row's exit already gave its reference back.
    pub fn take_pending_fifo_open(
        &mut self,
        tid: u64,
        entry: &alloc::sync::Arc<spin::Mutex<crate::ipc::fifo::FifoEntry>>,
    ) -> Option<crate::ipc::fifo::PendingFifoOpen> {
        let index = self
            .pending_fifo_opens
            .iter()
            .position(|open| open.tid == tid && alloc::sync::Arc::ptr_eq(&open.entry, entry))?;
        Some(self.pending_fifo_opens.swap_remove(index))
    }

    /// This row is terminating (POSIX fcntl record locks). A thread killed
    /// while waiting for a lock gives up its wait, and the group's last row
    /// releases the group's locks. Called once, from the terminated-state
    /// transition; under PROCESS_MANAGER the lock table wakes waiters through
    /// the deferred path.
    fn leave_record_locks(&self) {
        if let Some(ref thread) = self.main_thread {
            crate::fs::locks::release_thread(thread.id);
        }
        self.lock_owner.leave();
    }

    /// An exec detached this row from its thread group, making it a process
    /// of its own with its own lock owner. Record locks survive exec, so if
    /// this was the group's last live row the group's locks move to the new
    /// owner; otherwise they stay with the group's other rows.
    pub fn detach_lock_owner(&mut self) {
        let id = self.id.as_u64();
        if self.lock_owner.id() == id {
            return;
        }
        let own = crate::fs::locks::LockOwner::new(id);
        core::mem::replace(&mut self.lock_owner, own).hand_over(id);
    }

    /// An exec is detaching this row from its thread group. A row that was
    /// not the group's leader stops sharing the group's CPU account: the time
    /// it ran as a thread stays the group's, and from here on its time is its
    /// own, so the group and the new process are never both charged for it.
    /// Call before `thread_group_id` is cleared.
    pub fn detach_cpu_account(&mut self) {
        let id = self.id.as_u64();
        if self.thread_group_id.map_or(true, |group| group == id) {
            return;
        }
        use core::sync::atomic::Ordering;
        let wall = crate::signal::monotonic_micros();
        let user = self.cpu.user_ns.load(Ordering::Relaxed) / 1000;
        let total = user.saturating_add(self.cpu.system_ns.load(Ordering::Relaxed) / 1000);
        let timers = alloc::sync::Arc::new(crate::signal::IntervalTimers::default());
        timers.real.set_value(&self.itimers.real.get_value(wall), wall);
        timers.virtual_timer.set_value(&self.itimers.virtual_timer.get_value(user), 0);
        timers.prof.set_value(&self.itimers.prof.get_value(total), 0);
        self.itimers = timers;
        self.cpu = alloc::sync::Arc::new(crate::task::thread::CpuAccount::new(id));
        if let Some(thread) = self.main_thread.as_mut() {
            thread.cpu_account = Some(self.cpu.clone());
            thread.signals = self.signals.thread.clone();
            thread.signal_timers = Some(self.itimers.clone());
        }
    }

    /// Extract all file descriptor entries for deferred cleanup outside PM lock.
    ///
    /// Returns the FD entries without closing them — the caller is responsible
    /// for pipe close_read/close_write, PTY refcounting, etc.
    pub fn take_fd_entries(&mut self) -> alloc::vec::Vec<(usize, crate::ipc::fd::FileDescriptor)> {
        // Another thread still holds this descriptor table (CLONE_FILES): this
        // row lets go of it and closes nothing.
        if self.fd_table.is_shared() {
            self.fd_table = crate::ipc::fd::FdTable::empty();
            return alloc::vec::Vec::new();
        }
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
        // Another thread still holds this descriptor table (CLONE_FILES): this
        // row lets go of it and closes nothing.
        if self.fd_table.is_shared() {
            self.fd_table = crate::ipc::fd::FdTable::empty();
            return;
        }
        use crate::ipc::FdKind;

        for fd in 0..crate::ipc::MAX_FDS {
            if let Ok(fd_entry) = self.fd_table.close(fd as i32) {
                if crate::process::process_manager_held_on_current_cpu() {
                    crate::trace_count!(crate::tracing::providers::teardown::FD_CLOSES_UNDER_PM);
                }
                match fd_entry.kind {
                    FdKind::PipeRead(buffer) => {
                        // #919/P-2: deliver the reader-closed notification now,
                        // via the PM-safe deferred path. Process::terminate()
                        // has 4 identified callers; reading each call site
                        // shows 4 of 4 hold PM at this point --
                        // context_switch.rs:1400, manager.rs's
                        // exit_process_locked, and both
                        // signal/delivery.rs default-action arms -- so a
                        // direct wake_up() would risk acquiring SCHEDULER
                        // (Level 1) while PM (Level 2) is held, which this
                        // file's "Lock Ordering Discipline" note
                        // (scheduler.rs) forbids. deliver_deferred() instead
                        // buffers the wake lock-free; the buffer's own mutex
                        // is released before delivery (temporary drops at the
                        // `let` statement's semicolon).
                        let notifications = buffer.lock().close_read();
                        notifications.deliver_deferred();
                    }
                    FdKind::PipeWrite(buffer) => {
                        // #919/P-2: close_write() itself has no new-writer
                        // notification to deliver (writers, not readers,
                        // closed); its legacy read-waiter EOF wake is
                        // unaffected and stays inline.
                        let _should_notify = buffer.lock().close_write();
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
                        let notifications = socket.lock().close();
                        notifications.deliver_deferred();
                    }
                    FdKind::FifoRead(_, buffer, entry) => {
                        crate::ipc::fifo::close_fifo_read(&entry);
                        // #919/P-2: deliver via the PM-safe deferred path
                        // (see the PipeRead arm above).
                        let notifications = buffer.lock().close_read();
                        notifications.deliver_deferred();
                    }
                    FdKind::FifoWrite(_, buffer, entry) => {
                        crate::ipc::fifo::close_fifo_write(&entry);
                        // #919/P-2: no new-writer notification here either;
                        // see the PipeWrite arm above.
                        let _should_notify = buffer.lock().close_write();
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
        // Another thread still holds this descriptor table (CLONE_FILES): this
        // row lets go of it and closes nothing.
        if self.fd_table.is_shared() {
            self.fd_table = crate::ipc::fd::FdTable::empty();
            return;
        }
        use crate::ipc::FdKind;

        for fd in 0..crate::ipc::MAX_FDS {
            if let Ok(fd_entry) = self.fd_table.close(fd as i32) {
                if crate::process::process_manager_held_on_current_cpu() {
                    crate::trace_count!(crate::tracing::providers::teardown::FD_CLOSES_UNDER_PM);
                }
                match fd_entry.kind {
                    FdKind::PipeRead(buffer) => {
                        // #919/P-2: deliver the reader-closed notification now,
                        // via the PM-safe deferred path. Process::terminate()
                        // has 4 identified callers; reading each call site
                        // shows 4 of 4 hold PM at this point --
                        // context_switch.rs:1400, manager.rs's
                        // exit_process_locked, and both
                        // signal/delivery.rs default-action arms -- so a
                        // direct wake_up() would risk acquiring SCHEDULER
                        // (Level 1) while PM (Level 2) is held, which this
                        // file's "Lock Ordering Discipline" note
                        // (scheduler.rs) forbids. deliver_deferred() instead
                        // buffers the wake lock-free; the buffer's own mutex
                        // is released before delivery (temporary drops at the
                        // `let` statement's semicolon).
                        let notifications = buffer.lock().close_read();
                        notifications.deliver_deferred();
                    }
                    FdKind::PipeWrite(buffer) => {
                        // #919/P-2: close_write() itself has no new-writer
                        // notification to deliver (writers, not readers,
                        // closed); its legacy read-waiter EOF wake is
                        // unaffected and stays inline.
                        let _should_notify = buffer.lock().close_write();
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
                        let notifications = socket.lock().close();
                        notifications.deliver_deferred();
                    }
                    FdKind::FifoRead(_, buffer, entry) => {
                        crate::ipc::fifo::close_fifo_read(&entry);
                        // #919/P-2: deliver via the PM-safe deferred path
                        // (see the PipeRead arm above).
                        let notifications = buffer.lock().close_read();
                        notifications.deliver_deferred();
                    }
                    FdKind::FifoWrite(_, buffer, entry) => {
                        crate::ipc::fifo::close_fifo_write(&entry);
                        // #919/P-2: no new-writer notification here either;
                        // see the PipeWrite arm above.
                        let _should_notify = buffer.lock().close_write();
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
    /// Returns `true` only for the caller that installed the claim. That return
    /// is the arbiter for P6a condition **C3**: two concurrent waiters both pass
    /// the scan, and the loser must return `ECHILD` and copy no status rather
    /// than reporting a reap it did not perform.
    ///
    /// Refused on a row that has not terminated, so no live row can be
    /// tombstoned by an out-of-order claim.
    pub(crate) fn claim_reap(&mut self, reaper: ProcessId, status: i32) -> bool {
        if self.reaped.is_some() || !self.is_terminated() {
            return false;
        }
        self.reaped = Some((reaper, status));
        true
    }

    /// Record that a retirement receipt now names this row. Called by the exit
    /// path that defers the row's resources, which is the sole production
    /// producer of a receipt.
    pub(crate) fn note_receipt_created(&mut self) {
        self.retirement_receipts = self.retirement_receipts.saturating_add(1);
    }

    /// Settle one outstanding retirement receipt for this row, latching
    /// `retired` when the last one is gone.
    ///
    /// Returns `true` only for the call that latched. Refused on a row that has
    /// not terminated, so a receipt naming a still-live row cannot latch it.
    ///
    /// **The arm is keyed on a COUNT, not on a receipt identity** — review
    /// finding F4, stated here rather than left for a reader to discover. A
    /// settle that names a row with no counted receipt saturates at zero and
    /// then latches, so the fail-closed property holds only while every producer
    /// of a receipt counts one. In production it does: `defer_process_resources`
    /// is the sole producer, it holds `&mut Process` and calls
    /// `note_receipt_created`, and the aarch64 `defer_live_process_resources`
    /// route goes through it. `boot_test_reclaim` builds reclaims directly and
    /// is deliberately uncounted, which is safe only because every row it names
    /// is removed by the unconditional destructor rather than by the join. What
    /// guards the gap is structural, exactly as condition C4 asks: the
    /// `RECLAIM_ENQUEUE_CALLS` census makes any new receipt producer a `+` row
    /// rather than a silent early removal.
    pub(crate) fn settle_retirement_receipt(&mut self) -> bool {
        self.retirement_receipts = self.retirement_receipts.saturating_sub(1);
        self.latch_retired_if_settled()
    }

    /// Record that this row's resources ended without ever producing a receipt
    /// (P6a condition **C2**: the aarch64 synchronous-release arm and the
    /// `already_terminated` exit arms). The latch is still refused while a
    /// receipt from an earlier exit of the same row is outstanding.
    pub(crate) fn note_resources_absent(&mut self) {
        let _ = self.latch_retired_if_settled();
    }

    fn latch_retired_if_settled(&mut self) -> bool {
        if self.retired || self.retirement_receipts != 0 || !self.is_terminated() {
            return false;
        }
        self.retired = true;
        true
    }

    /// The reap half of the join, for the join's own removal predicate.
    pub(crate) fn is_reaped(&self) -> bool {
        self.reaped.is_some()
    }

    /// The retirement half of the join.
    pub(crate) fn is_retired(&self) -> bool {
        self.retired
    }

    /// The ledger half of the join's removal condition: every obligation in this
    /// row's exit ledger is `Completed` or `Absent`.
    ///
    /// **Vacuously true in P6a, deliberately and by construction.** The ledger
    /// carries only `Sigchld` and `Report`, and DESIGN's own P2 exemption is the
    /// reason: both are discharged no later than the zombie transition, and a
    /// parent cannot reap a row that has not reached zombie, so neither can
    /// outlive the reap. `Resources` — the obligation that can outlive a reap,
    /// and the reason P6a exists — does not become row-resident until P6b, which
    /// is the phase that makes this term live. Evaluating today's two
    /// obligations here instead would let a row that never reached
    /// `handle_thread_exit` stall its own removal forever, which is a stranding
    /// this phase must not introduce.
    pub(crate) fn ledger_settled(&self) -> bool {
        true
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

    /// The status a row that has already terminated reports, changed to
    /// `exit_code`: a thread-group leader that ended with its own exit while
    /// the rest of its group ran on reports the status the process later died
    /// with (exit_group or a fatal signal). Does nothing to a live row, which
    /// terminates only through `terminate` and `terminate_minimal`.
    pub fn restate_exit_status(&mut self, exit_code: i32) {
        if let ProcessState::Terminated(status) = &mut self.state {
            *status = exit_code;
            self.exit_code = Some(exit_code);
        }
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

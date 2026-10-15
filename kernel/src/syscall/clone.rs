//! Clone syscall for thread creation
//!
//! Implements clone with CLONE_VM for creating threads that share the parent's
//! address space. Used by pthread_create via libbreenix-libc.
//!
//! Design: Each "thread" is a separate Process in the kernel that shares the
//! parent's page table via inherited_cr3. This minimizes changes to the
//! existing single-threaded process architecture while supporting std::thread.

use super::SyscallResult;
use crate::process::process::Process;
use crate::task::thread::{CpuContext, Thread, ThreadPrivilege};
use alloc::boxed::Box;

/// Clone flags (Linux-compatible)
const CLONE_VM: u64 = 0x00000100;
const CLONE_THREAD: u64 = 0x00010000;
const CLONE_SIGHAND: u64 = 0x00000800;
const CLONE_FS: u64 = 0x00000200;
const CLONE_FILES: u64 = 0x00000400;
const CLONE_SYSVSEM: u64 = 0x00040000;
const CLONE_SETTLS: u64 = 0x00080000;
const CLONE_CHILD_CLEARTID: u64 = 0x00200000;
const CLONE_CHILD_SETTID: u64 = 0x01000000;
const CLONE_VFORK: u64 = 0x00004000;
const CLONE_PARENT_SETTID: u64 = 0x00100000;
/// The low byte of the flags: the signal the parent gets when the child ends.
const CSIGNAL: u64 = 0xff;
const SIGCHLD: u64 = 17;

/// Whether a clone asks for a new process rather than a thread, and so runs as
/// fork: without CLONE_VM (how Linux libcs fork), or with CLONE_VFORK and no
/// child stack, the form vfork takes as a system call. That child would run on
/// the parent's own stack, where it may only exec or _exit; POSIX and the
/// Linux vfork page let vfork be fork, so the child gets a copy of the address
/// space instead and the parent is not suspended.
pub fn is_fork_style(flags: u64, child_stack: u64) -> bool {
    flags & CLONE_VM == 0 || (flags & CLONE_VFORK != 0 && child_stack == 0)
}

/// What a fork-style clone asks of its child beyond a plain fork's copy
/// (`is_fork_style`). Its arguments follow Linux's clone order: flags, stack,
/// parent_tid, then child_tid and tls on x86-64, tls and child_tid on ARM64.
#[derive(Clone, Copy, Default)]
pub struct ForkClone {
    /// CLONE_FILES: the child shares the parent's descriptor table.
    pub share_files: bool,
    /// CLONE_FS: the child shares the parent's working directories.
    pub share_fs: bool,
    /// CLONE_SYSVSEM: the child shares the parent's SEM_UNDO adjustments.
    pub share_undo: bool,
    /// The child's user stack pointer; the parent's when absent.
    stack: Option<u64>,
    /// CLONE_SETTLS: the child's thread pointer.
    tls: Option<u64>,
    /// CLONE_PARENT_SETTID: where the parent gets the child's thread ID.
    parent_tid: Option<u64>,
    /// CLONE_CHILD_SETTID: where the child's memory gets its thread ID.
    pub child_set_tid: Option<u64>,
    /// CLONE_CHILD_CLEARTID: the word cleared and woken when the child exits.
    pub child_clear_tid: Option<u64>,
}

impl ForkClone {
    /// A plain fork: a copy of everything, sharing nothing.
    pub const FORK: Self = Self {
        share_files: false,
        share_fs: false,
        share_undo: false,
        stack: None,
        tls: None,
        parent_tid: None,
        child_set_tid: None,
        child_clear_tid: None,
    };

    /// Check a fork-style clone's flags and take its arguments. Flags that
    /// need a shared address space (CLONE_SIGHAND, CLONE_THREAD), an exit
    /// signal other than SIGCHLD, and flags Breenix does not implement are
    /// refused with EINVAL rather than ignored.
    pub fn from_args(flags: u64, stack: u64, parent_tid: u64, arg4: u64, arg5: u64) -> Result<Self, u64> {
        const SUPPORTED: u64 = CSIGNAL
            | CLONE_VM
            | CLONE_VFORK
            | CLONE_FS
            | CLONE_FILES
            | CLONE_SYSVSEM
            | CLONE_SETTLS
            | CLONE_PARENT_SETTID
            | CLONE_CHILD_SETTID
            | CLONE_CHILD_CLEARTID;
        let einval = Err(super::errno::EINVAL as u64);
        if flags & !SUPPORTED != 0 || flags & CSIGNAL != SIGCHLD {
            return einval;
        }
        // CLONE_VM reaches here only as the stackless vfork (`is_fork_style`).
        if flags & CLONE_VM != 0 && (flags & CLONE_VFORK == 0 || stack != 0) {
            return einval;
        }
        #[cfg(target_arch = "x86_64")]
        let (child_tid, tls) = (arg4, arg5);
        #[cfg(not(target_arch = "x86_64"))]
        let (tls, child_tid) = (arg4, arg5);
        // An FS base must be a user address, as ARCH_SET_FS requires.
        #[cfg(target_arch = "x86_64")]
        if flags & CLONE_SETTLS != 0 && tls >= crate::memory::layout::USER_STACK_REGION_END {
            return Err(super::errno::EPERM as u64);
        }
        let given = |flag: u64, addr: u64| (flags & flag != 0 && addr != 0).then_some(addr);
        Ok(Self {
            share_files: flags & CLONE_FILES != 0,
            share_fs: flags & CLONE_FS != 0,
            share_undo: flags & CLONE_SYSVSEM != 0,
            stack: (stack != 0).then_some(stack),
            tls: (flags & CLONE_SETTLS != 0).then_some(tls),
            parent_tid: given(CLONE_PARENT_SETTID, parent_tid),
            child_set_tid: given(CLONE_CHILD_SETTID, child_tid),
            child_clear_tid: given(CLONE_CHILD_CLEARTID, child_tid),
        })
    }

    /// Give the child's starting registers, copied from the parent's, the
    /// requested stack and thread pointer.
    pub fn apply_to_context(&self, context: &mut crate::task::thread::CpuContext) {
        #[cfg(target_arch = "x86_64")]
        {
            if let Some(stack) = self.stack {
                context.rsp = stack;
            }
            if let Some(tls) = self.tls {
                context.user_fs_base = tls;
                context.user_fs_base_set = true;
            }
        }
        #[cfg(target_arch = "aarch64")]
        {
            if let Some(stack) = self.stack {
                context.sp_el0 = stack;
            }
            if let Some(tls) = self.tls {
                context.tpidr_el0 = tls;
            }
        }
    }

    /// CLONE_CHILD_SETTID, under PROCESS_MANAGER before the child first runs:
    /// store its thread ID in its own copy of the address space. Linux stores
    /// it from the child as it starts and ignores a failure; an unaligned or
    /// unmapped word is left alone here the same way.
    pub fn store_child_tid(&self, manager: &mut crate::process::ProcessManager, child: crate::process::ProcessId, tid: u64) {
        let Some(addr) = self.child_set_tid else {
            return;
        };
        if let Some(table) = manager
            .get_process_mut(child)
            .and_then(|process| process.page_table.as_mut())
        {
            let _ = table.store_user_u32(addr, tid as u32, child.as_u64());
        }
    }

    /// CLONE_PARENT_SETTID, once PROCESS_MANAGER is released: store the
    /// child's thread ID in the parent's memory.
    pub fn store_parent_tid(&self, tid: u64) {
        if let Some(addr) = self.parent_tid {
            let _ = super::userptr::copy_to_user(addr as *mut u32, &(tid as u32));
        }
    }
}

/// P5b: the designated init cannot acquire `CLONE_VM` siblings.
///
/// The sole consultation of the init authority on the clone path. `derived_tg_id` is the
/// parent's *effective* thread-group id (`thread_group_id.unwrap_or(pid)`), and init's is
/// derived the same way, so the comparison is group-to-group and never pid-to-pid. With no
/// designated init this refuses nothing, which is the defined behaviour of a build with no
/// real init.
pub fn refuses_init_group_clone(
    manager: &crate::process::ProcessManager,
    derived_tg_id: u64,
) -> bool {
    let Some(init) = manager.designated_init() else {
        return false;
    };
    let init_tg_id = manager
        .get_process(init)
        .and_then(|process| process.thread_group_id)
        .unwrap_or(init.as_u64());
    derived_tg_id == init_tg_id
}

/// sys_clone_thread - create a new thread sharing the parent's address space
///
/// Breenix extension: instead of the standard Linux clone semantics where both
/// parent and child return from the syscall, we support a fn_ptr + fn_arg style:
///   - child_stack: top of the child's user stack
///   - fn_ptr: entry point for the child thread (set as RIP)
///   - fn_arg: argument for the child (set as RDI)
///   - child_tidptr: address to write child TID and clear on exit
///   - tls: with CLONE_SETTLS, the child's thread pointer (TPIDR_EL0 on
///     ARM64, the FS base on x86-64); without it the child starts with its
///     creator's
///
/// Syscall args: clone(flags, child_stack, fn_ptr, fn_arg, child_tidptr, tls)
///   - arg1 (RDI): flags
///   - arg2 (RSI): child_stack (top of stack, grows down)
///   - arg3 (RDX): fn_ptr (entry point function)
///   - arg4 (R10): fn_arg (argument to pass in RDI)
///   - arg5 (R8):  child_tidptr (for CLONE_CHILD_CLEARTID / CLONE_CHILD_SETTID)
///   - arg6 (R9):  tls (for CLONE_SETTLS)
pub fn sys_clone_thread(
    flags: u64,
    child_stack: u64,
    fn_ptr: u64,
    fn_arg: u64,
    child_tidptr: u64,
    tls: u64,
) -> SyscallResult {
    // Validate required flags
    if flags & CLONE_VM == 0 {
        // Without CLONE_VM, use fork instead
        log::warn!("clone: called without CLONE_VM, use fork instead");
        return SyscallResult::Err(super::errno::EINVAL as u64);
    }

    if flags & CLONE_THREAD != 0 && flags & CLONE_SIGHAND == 0 {
        return SyscallResult::Err(super::errno::EINVAL as u64);
    }

    if child_stack == 0 || fn_ptr == 0 {
        return SyscallResult::Err(super::errno::EINVAL as u64);
    }
    // An FS base must be a user address, as ARCH_SET_FS requires.
    #[cfg(target_arch = "x86_64")]
    if flags & CLONE_SETTLS != 0 && tls >= crate::memory::layout::USER_STACK_REGION_END {
        return SyscallResult::Err(super::errno::EPERM as u64);
    }

    // Get current thread/process info
    let thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => return SyscallResult::Err(super::errno::ESRCH as u64),
    };
    let mut manager_guard = crate::process::manager();
    let manager = match manager_guard.as_mut() {
        Some(m) => m,
        None => return SyscallResult::Err(super::errno::ESRCH as u64),
    };

    // Find the parent process and admit the clone before copying any parent
    // state out of the process-manager transaction.
    let parent_pid = match manager.find_process_by_thread(thread_id) {
        Some((pid, _)) => pid,
        None => return SyscallResult::Err(super::errno::ESRCH as u64),
    };
    // A thread whose group is dying (a group exit or a fatal signal set
    // `group_exit_code` on every row) creates no thread the kills would miss.
    let group_dying = manager
        .get_process(parent_pid)
        .is_some_and(|process| process.group_exit_code.is_some());
    if group_dying
        || !manager.admit_clone_into(parent_pid)
        || !crate::process::limits::fork_allowed(manager, parent_pid)
    {
        return SyscallResult::Err(super::errno::EAGAIN as u64);
    }
    let (parent_cr3, parent_tg_id, parent_cwd, parent_lock_owner, parent_signals) = {
        let process = manager
            .get_process(parent_pid)
            .expect("admitted clone parent remains present under process-manager guard");

        // Get CR3 from page table or inherited_cr3
        let cr3 = if let Some(ref pt) = process.page_table {
            pt.level_4_frame().start_address().as_u64()
        } else if let Some(cr3) = process.inherited_cr3 {
            cr3
        } else {
            log::error!(
                "clone: parent process {} has no page table",
                parent_pid.as_u64()
            );
            return SyscallResult::Err(super::errno::ENOMEM as u64);
        };

        // Thread group ID: inherit from parent or use parent's pid
        let tg_id = process.thread_group_id.unwrap_or(parent_pid.as_u64());

        (
            cr3,
            tg_id,
            if flags & CLONE_FS != 0 {
                process.cwd.share()
            } else {
                process.cwd.copy()
            },
            process.lock_owner.clone(),
            if flags & CLONE_SIGHAND != 0 {
                process.signals.new_thread()
            } else {
                process.signals.fork()
            },
        )
    };

    // P5b: refuse a CLONE_VM join into the designated init's thread group. This returns
    // above `allocate_pid`, so a refused clone allocates nothing and creates no row.
    if refuses_init_group_clone(manager, parent_tg_id) {
        crate::tracing::providers::teardown::record_init_group_refusal();
        #[cfg(feature = "boot_tests")]
        let walk = crate::tracing::providers::teardown::init_group_walk_census(manager);
        // The walk emits on serial; drop the process-manager guard before any output.
        drop(manager_guard);
        #[cfg(feature = "boot_tests")]
        crate::tracing::providers::teardown::emit_init_group_walk(walk);
        return SyscallResult::Err(super::errno::EINVAL as u64);
    }

    // Allocate child process ID
    let child_pid = manager.allocate_pid();

    // CLONE_SYSVSEM: the child shares its creator's SEM_UNDO adjustments,
    // from before the row is published until it exits. Without it the child
    // starts with none, as after fork.
    let shares_undo = flags & CLONE_SYSVSEM != 0;
    if shares_undo && crate::ipc::sysv::share_undo(parent_pid, child_pid).is_err() {
        return SyscallResult::Err(super::errno::ENOMEM as u64);
    }

    // Allocate a kernel stack for the child thread
    let kernel_stack = match crate::memory::alloc_kernel_stack(16 * 1024) {
        Some(stack) => stack,
        None => {
            log::error!("clone: failed to allocate kernel stack");
            if shares_undo {
                crate::ipc::sysv::unshare_undo(child_pid);
            }
            return SyscallResult::Err(super::errno::ENOMEM as u64);
        }
    };
    let kernel_stack_top = kernel_stack.top();

    // Create child thread
    let child_thread_id = crate::task::thread::allocate_thread_id();

    // Set up CPU context for the child
    // Child starts at fn_ptr with fn_arg in RDI, using child_stack
    #[cfg(target_arch = "x86_64")]
    let child_context = {
        let mut ctx = CpuContext::new(
            x86_64::VirtAddr::new(fn_ptr),
            x86_64::VirtAddr::new(child_stack),
            ThreadPrivilege::User,
        );
        ctx.rdi = fn_arg; // First argument per SysV ABI
        // Without CLONE_SETTLS the thread starts with its creator's FS base,
        // which is still loaded during the creator's syscall.
        ctx.user_fs_base = if flags & CLONE_SETTLS != 0 {
            tls
        } else {
            x86_64::registers::model_specific::FsBase::read().as_u64()
        };
        ctx.user_fs_base_set = true;
        ctx
    };

    #[cfg(target_arch = "aarch64")]
    let child_context = {
        let mut ctx = CpuContext::new_user_thread(fn_ptr, child_stack, 0);
        ctx.x0 = fn_arg; // First argument per AAPCS64
        ctx.tpidr_el0 = if flags & CLONE_SETTLS != 0 {
            tls
        } else {
            let creator: u64;
            // SAFETY: reading the calling thread's own TPIDR_EL0.
            unsafe {
                core::arch::asm!("mrs {}, tpidr_el0", out(reg) creator, options(nomem, nostack));
            }
            creator
        };
        ctx
    };

    // Set up TLS for child
    #[cfg(target_arch = "x86_64")]
    let tls_block = x86_64::VirtAddr::new(0x10000 + child_thread_id * 0x1000);
    #[cfg(not(target_arch = "x86_64"))]
    let tls_block = crate::memory::arch_stub::VirtAddr::new(0x10000 + child_thread_id * 0x1000);

    #[cfg(target_arch = "x86_64")]
    if let Err(e) = crate::tls::register_thread_tls(child_thread_id, tls_block) {
        log::warn!(
            "clone: failed to register TLS for thread {}: {}",
            child_thread_id,
            e
        );
    }

    #[cfg(target_arch = "x86_64")]
    let stack_top_addr = x86_64::VirtAddr::new(child_stack);
    #[cfg(not(target_arch = "x86_64"))]
    let stack_top_addr = crate::memory::arch_stub::VirtAddr::new(child_stack);

    // Calculate stack bottom (assume 2MB stack, doesn't need to be exact)
    let stack_bottom_addr = {
        #[cfg(target_arch = "x86_64")]
        {
            x86_64::VirtAddr::new(child_stack.saturating_sub(2 * 1024 * 1024))
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            crate::memory::arch_stub::VirtAddr::new(child_stack.saturating_sub(2 * 1024 * 1024))
        }
    };

    // #650 census C rows this as the tree's only production `Blocked` thread
    // construction, and it is legal rather than a park. The publication interlock
    // for a new row is `ProcessState::Creating`, not the thread's state:
    // `Process::new` opens the row `Creating`, `admits_clone` refuses a `Creating`
    // row and `is_unpublished` reports it, and the row leaves `Creating` only at
    // the `set_ready()` below. That readying and the matching `Ready` store on this
    // thread happen under the same process-manager guard that `insert_process`
    // runs under, with no early return between them, so no observer can ever see
    // this thread blocked: it is an admission-time initial state, not a wait with
    // an owed wake. It is therefore deliberately outside the derived
    // blocked-publication family rule, which reads a blocked publication as a park
    // and demands a blocking-primitive name that an initial state has no business
    // carrying. Whether constructions belong under that rule at all is the P9
    // admission-interlock question.
    let mut child_thread = Thread {
        id: child_thread_id,
        name: alloc::format!("clone-child-{}", child_thread_id),
        state: crate::task::thread::ThreadState::Blocked,
        context: child_context,
        stack_top: stack_top_addr,
        stack_bottom: stack_bottom_addr,
        kernel_stack_top: Some(kernel_stack_top),
        kernel_stack_allocation: Some(kernel_stack),
        #[cfg(target_arch = "x86_64")]
        fpu: crate::arch_impl::x86_64::fpu::FpuState::capture(),
        #[cfg(target_arch = "aarch64")]
        fpsimd: crate::arch_impl::aarch64::fpsimd::capture(),
        tls_block,
        sched: crate::task::thread::SchedPolicy::DEFAULT,
        time_slice: 10,
        entry_point: None,
        privilege: ThreadPrivilege::User,
        has_started: false, // Will be set up via first_userspace_entry
        blocked_in_syscall: false,
        saved_by_inline_schedule: false,
        inline_schedule_spsr: 0,
        inline_schedule_prev_elr: 0,
        inline_schedule_caller_lr: 0,
        inline_schedule_saved_sp: 0,
        saved_userspace_context: None,
        wake_time_ns: None,
        realtime_sleep: false,
        timer_pop: None,
        run_start_ticks: 0,
        cpu_ticks_total: 0,
        resource_limits: None,
        cpu_account: None,
        signals: alloc::sync::Arc::new(crate::signal::ThreadSignals::with_mask(0)),
        signal_timers: None,
        owner_pid: Some(child_pid.as_u64()),
        cached_ttbr0: 0,
        wait_loop_iters: core::sync::atomic::AtomicU64::new(0),
        kill_custody: core::sync::atomic::AtomicU64::new(0),
        cpu_affinity: None,
    };

    // Set has_started to true so we go through the restore path (not first_entry)
    // Actually, for clone children we want has_started=true because the context
    // is fully set up (like fork children)
    child_thread.has_started = true;

    // Create child process
    #[cfg(target_arch = "x86_64")]
    let entry_point = x86_64::VirtAddr::new(fn_ptr);
    #[cfg(not(target_arch = "x86_64"))]
    let entry_point = crate::memory::arch_stub::VirtAddr::new(fn_ptr);

    let parent = manager.get_process(parent_pid)
        .expect("parent remains present under PM during clone");
    let child_name = if flags & CLONE_THREAD != 0 {
        alloc::format!("thread-{}", child_pid.as_u64())
    } else {
        // A process sharing the address space runs the parent's program.
        match crate::syscall::exec::copy_string(&parent.name) {
            Ok(name) => name,
            Err(errno) => {
                if shares_undo {
                    crate::ipc::sysv::unshare_undo(child_pid);
                }
                return SyscallResult::Err(errno);
            }
        }
    };
    let mut child_process = Process::new(child_pid, child_name, entry_point);
    child_process.forked_without_exec = flags & CLONE_THREAD == 0;
    child_process.limits = parent.limits.clone();
    child_process.cred = parent.cred.clone();
    child_process.nice = parent.nice;
    // The creator's scheduling policy, which sched_setscheduler keeps in step
    // with the scheduler's copy. Under SCHED_RESET_ON_FORK a new thread does
    // not inherit a negative nice either.
    let creator_sched = parent.main_thread.as_ref().map(|thread| thread.sched);
    if creator_sched.is_some_and(|sched| sched.reset_on_fork) {
        child_process.nice = child_process.nice.max(0);
    }
    // A thread's CPU time is its process's: the group shares one account.
    if flags & CLONE_THREAD != 0 {
        child_process.cpu = parent.cpu.clone();
        child_process.itimers = parent.itimers.clone();
    }
    // Every task runs the image its creator runs.
    child_process.exe_path = parent.exe_path.clone();
    child_process.cmdline = parent.cmdline.clone();
    child_process.cmdline_area = parent.cmdline_area;
    child_process.umask = parent.umask;
    // A thread is in its process's process group and session.
    child_process.pgid = parent.pgid;
    crate::tty::ctty::inherit(&mut child_process, parent);
    child_process.parent = Some(parent_pid);
    child_process.inherited_cr3 = Some(parent_cr3);
    child_process.address_space_lifetime = parent.address_space_lifetime.clone();
    child_process.thread_group_id = (flags & CLONE_THREAD != 0).then_some(parent_tg_id);
    // A thread shares its process's record locks (POSIX fcntl).
    if flags & CLONE_THREAD != 0 {
        child_process.lock_owner = parent_lock_owner;
    }
    child_process.cwd = parent_cwd;
    // Threads of a process share its signal dispositions (sys_sigaction keeps
    // the group's copies in step) and start with the creator's signal mask.
    child_process.signals = parent_signals;

    // CLONE_FILES: the thread shares its creator's descriptor table, so a
    // descriptor either opens is the other's too (#1307). Without it the
    // thread gets a copy, as fork does.
    let parent_fds = &manager
        .get_process(parent_pid)
        .expect("parent remains present under PM during clone")
        .fd_table;
    child_process.fd_table = if flags & CLONE_FILES != 0 {
        parent_fds.share()
    } else {
        parent_fds.clone()
    };

    child_process.fd_table.set_limit(
        child_process
            .limits
            .get(crate::process::limits::NOFILE)
            .soft,
    );

    // Set clear_child_tid for thread exit notification
    if flags & CLONE_CHILD_CLEARTID != 0 && child_tidptr != 0 {
        child_process.clear_child_tid = Some(child_tidptr);
    }

    // Write child TID to child_tidptr (CLONE_CHILD_SETTID), but only after
    // PROCESS_MANAGER is dropped. The pointer may reference a CoW page.
    let child_tid_write = if flags & CLONE_CHILD_SETTID != 0 && child_tidptr != 0 {
        Some((child_tidptr as *mut u32, child_thread_id as u32))
    } else {
        None
    };

    // Write child TID to parent's tidptr (CLONE_PARENT_SETTID)
    // (handled by caller since we return the tid)

    // A new thread starts with its creator's CPU affinity and scheduling
    // policy (`SchedPolicy::for_child`).
    let creator = manager
        .get_process(parent_pid)
        .and_then(|parent| parent.main_thread.as_ref());
    child_thread.cpu_affinity =
        crate::task::thread::CpuPin::for_child(creator.and_then(|thread| thread.cpu_affinity));
    if let Some(sched) = creator_sched {
        child_thread.sched = sched.for_child();
    }
    child_process.attach_main_thread_unpublished(child_thread);

    // Add child to process manager. From here the row is published and leaves
    // its lock owner through the terminated-state transition, so it joins now.
    if flags & CLONE_THREAD != 0 {
        child_process.lock_owner.join();
    }
    manager.insert_process(child_pid, child_process);

    // Publication is complete only after the row exists. Make both the row and
    // its main thread runnable while the same process-manager guard is held.
    {
        let child = manager
            .get_process_mut(child_pid)
            .expect("clone child remains present immediately after insertion");
        child.set_ready();
        child
            .main_thread
            .as_mut()
            .expect("clone child retains its attached main thread")
            .state = crate::task::thread::ThreadState::Ready;
    }

    // Add child process as parent's child
    if let Some(parent) = manager.get_process_mut(parent_pid) {
        parent.children.push(child_pid);
    }

    // Get the thread from the newly inserted process to add to scheduler
    let scheduler_thread = if let Some(process) = manager.get_process_mut(child_pid) {
        if let Some(ref mut thread) = process.main_thread {
            Some(Box::new(thread.publish_to_scheduler()))
        } else {
            None
        }
    } else {
        None
    };

    drop(manager_guard);

    if let Some((ptr, tid)) = child_tid_write {
        let _ = super::userptr::copy_to_user(ptr, &tid);
    }

    // Add thread to scheduler
    if let Some(thread_box) = scheduler_thread {
        crate::task::scheduler::spawn(thread_box);
    }

    // A group kill that ran between the release of PROCESS_MANAGER and the
    // spawn ended the row while its thread was in no scheduler queue, so the
    // thread was spawned Ready all the same. Kill it again now that the
    // scheduler has it. The creator is still in this syscall, so its row is
    // live and the group's address space with it: the thread cannot have run
    // on a released one in between.
    //
    // Once spawned, the thread may also have run and exited by itself on
    // another CPU, which ends its row too. That row carries no
    // `group_exit_code`, which only a kill sets. Killing it again marked it
    // with SIGKILL and status 0, and the signal took the whole group with
    // status 0 while its creator was still running (#1349).
    let child_ended = crate::process::with_process_manager(|manager| {
        manager
            .get_process(child_pid)
            .map_or(Some(0), |child| match (child.state, child.group_exit_code) {
                (crate::process::ProcessState::Terminated(code), Some(_)) => Some(code),
                _ => None,
            })
    })
    .flatten();
    if let Some(code) = child_ended {
        crate::syscall::signal::kill_process_now(child_pid, code, None);
    }

    log::debug!(
        "clone: created child thread {} (pid {}) for parent pid {}, fn_ptr={:#x}, stack={:#x}",
        child_thread_id,
        child_pid.as_u64(),
        parent_pid.as_u64(),
        fn_ptr,
        child_stack
    );

    // Return child thread ID to parent
    SyscallResult::Ok(child_thread_id)
}

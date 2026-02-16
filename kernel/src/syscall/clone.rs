//! Clone syscall for thread creation
//!
//! Implements clone with CLONE_VM for creating threads that share the parent's
//! address space. Used by pthread_create via libbreenix-libc.
//!
//! Design: Each "thread" is a separate Process in the kernel that shares the
//! parent's page table via inherited_cr3. This minimizes changes to the
//! existing single-threaded process architecture while supporting std::thread.

use alloc::boxed::Box;
use super::SyscallResult;
use crate::process::process::Process;
use crate::task::thread::{Thread, ThreadPrivilege, CpuContext};

/// Clone flags (Linux-compatible)
const CLONE_VM: u64 = 0x00000100;
const CLONE_FILES: u64 = 0x00000400;
const CLONE_CHILD_CLEARTID: u64 = 0x00200000;
const CLONE_CHILD_SETTID: u64 = 0x01000000;

/// sys_clone - create a new thread sharing the parent's address space
///
/// Breenix extension: instead of the standard Linux clone semantics where both
/// parent and child return from the syscall, we support a fn_ptr + fn_arg style:
///   - child_stack: top of the child's user stack
///   - fn_ptr: entry point for the child thread (set as RIP)
///   - fn_arg: argument for the child (set as RDI)
///   - child_tidptr: address to write child TID and clear on exit
///
/// Syscall args: clone(flags, child_stack, fn_ptr, fn_arg, child_tidptr)
///   - arg1 (RDI): flags
///   - arg2 (RSI): child_stack (top of stack, grows down)
///   - arg3 (RDX): fn_ptr (entry point function)
///   - arg4 (R10): fn_arg (argument to pass in RDI)
///   - arg5 (R8):  child_tidptr (for CLONE_CHILD_CLEARTID / CLONE_CHILD_SETTID)
pub fn sys_clone(
    flags: u64,
    child_stack: u64,
    fn_ptr: u64,
    fn_arg: u64,
    child_tidptr: u64,
) -> SyscallResult {
    // Validate required flags
    if flags & CLONE_VM == 0 {
        // Without CLONE_VM, use fork instead
        log::warn!("clone: called without CLONE_VM, use fork instead");
        return SyscallResult::Err(super::errno::EINVAL as u64);
    }

    if child_stack == 0 || fn_ptr == 0 {
        return SyscallResult::Err(super::errno::EINVAL as u64);
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

    // Find the parent process
    let (parent_pid, parent_cr3, parent_tg_id, parent_cwd, parent_fd_table) = {
        let (pid, process) = match manager.find_process_by_thread_mut(thread_id) {
            Some(p) => p,
            None => return SyscallResult::Err(super::errno::ESRCH as u64),
        };

        // Get CR3 from page table or inherited_cr3
        let cr3 = if let Some(ref pt) = process.page_table {
            pt.level_4_frame().start_address().as_u64()
        } else if let Some(cr3) = process.inherited_cr3 {
            cr3
        } else {
            log::error!("clone: parent process {} has no page table", pid.as_u64());
            return SyscallResult::Err(super::errno::ENOMEM as u64);
        };

        // Thread group ID: inherit from parent or use parent's pid
        let tg_id = process.thread_group_id.unwrap_or(pid.as_u64());

        (
            pid,
            cr3,
            tg_id,
            process.cwd.clone(),
            process.fd_table.clone(),
        )
    };

    // Allocate child process ID
    let child_pid = manager.allocate_pid();

    // Allocate a kernel stack for the child thread
    let kernel_stack = match crate::memory::alloc_kernel_stack(16 * 1024) {
        Some(stack) => stack,
        None => {
            log::error!("clone: failed to allocate kernel stack");
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
        ctx
    };

    #[cfg(target_arch = "aarch64")]
    let child_context = {
        let mut ctx = CpuContext::new_user_thread(fn_ptr, child_stack, 0);
        ctx.x0 = fn_arg; // First argument per AAPCS64
        ctx
    };

    // Set up TLS for child
    #[cfg(target_arch = "x86_64")]
    let tls_block = x86_64::VirtAddr::new(0x10000 + child_thread_id * 0x1000);
    #[cfg(not(target_arch = "x86_64"))]
    let tls_block = crate::memory::arch_stub::VirtAddr::new(0x10000 + child_thread_id * 0x1000);

    #[cfg(target_arch = "x86_64")]
    if let Err(e) = crate::tls::register_thread_tls(child_thread_id, tls_block) {
        log::warn!("clone: failed to register TLS for thread {}: {}", child_thread_id, e);
    }

    #[cfg(target_arch = "x86_64")]
    let stack_top_addr = x86_64::VirtAddr::new(child_stack);
    #[cfg(not(target_arch = "x86_64"))]
    let stack_top_addr = crate::memory::arch_stub::VirtAddr::new(child_stack);

    // Calculate stack bottom (assume 2MB stack, doesn't need to be exact)
    let stack_bottom_addr = {
        #[cfg(target_arch = "x86_64")]
        { x86_64::VirtAddr::new(child_stack.saturating_sub(2 * 1024 * 1024)) }
        #[cfg(not(target_arch = "x86_64"))]
        { crate::memory::arch_stub::VirtAddr::new(child_stack.saturating_sub(2 * 1024 * 1024)) }
    };

    let mut child_thread = Thread {
        id: child_thread_id,
        name: alloc::format!("clone-child-{}", child_thread_id),
        state: crate::task::thread::ThreadState::Ready,
        context: child_context,
        stack_top: stack_top_addr,
        stack_bottom: stack_bottom_addr,
        kernel_stack_top: Some(kernel_stack_top),
        kernel_stack_allocation: Some(kernel_stack),
        tls_block,
        priority: 128,
        time_slice: 10,
        entry_point: None,
        privilege: ThreadPrivilege::User,
        has_started: false,  // Will be set up via first_userspace_entry
        blocked_in_syscall: false,
        saved_userspace_context: None,
        wake_time_ns: None,
        run_start_ticks: 0,
        cpu_ticks_total: 0,
        owner_pid: Some(child_pid.as_u64()),
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

    let mut child_process = Process::new(child_pid, alloc::format!("thread-{}", child_pid.as_u64()), entry_point);
    child_process.parent = Some(parent_pid);
    child_process.state = crate::process::process::ProcessState::Ready;
    child_process.inherited_cr3 = Some(parent_cr3);
    child_process.thread_group_id = Some(parent_tg_id);
    child_process.cwd = parent_cwd;

    // Share file descriptors if CLONE_FILES
    if flags & CLONE_FILES != 0 {
        child_process.fd_table = parent_fd_table;
    }

    // Set clear_child_tid for thread exit notification
    if flags & CLONE_CHILD_CLEARTID != 0 && child_tidptr != 0 {
        child_process.clear_child_tid = Some(child_tidptr);
    }

    // Write child TID to child_tidptr (CLONE_CHILD_SETTID)
    if flags & CLONE_CHILD_SETTID != 0 && child_tidptr != 0 {
        unsafe {
            let ptr = child_tidptr as *mut u32;
            if !ptr.is_null() && (child_tidptr as usize) < 0x7FFF_FFFF_FFFF {
                core::ptr::write_volatile(ptr, child_thread_id as u32);
            }
        }
    }

    // Write child TID to parent's tidptr (CLONE_PARENT_SETTID)
    // (handled by caller since we return the tid)

    child_process.set_main_thread(child_thread);

    // Add child to process manager
    manager.insert_process(child_pid, child_process);

    // Add child process as parent's child
    if let Some(parent) = manager.get_process_mut(parent_pid) {
        parent.children.push(child_pid);
    }

    // Get the thread from the newly inserted process to add to scheduler
    let scheduler_thread = if let Some(process) = manager.get_process_mut(child_pid) {
        if let Some(ref thread) = process.main_thread {
            // Create a copy for the scheduler
            Some(Box::new(thread.clone()))
        } else {
            None
        }
    } else {
        None
    };

    drop(manager_guard);

    // Add thread to scheduler
    if let Some(thread_box) = scheduler_thread {
        crate::task::scheduler::spawn(thread_box);
    }

    log::info!(
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

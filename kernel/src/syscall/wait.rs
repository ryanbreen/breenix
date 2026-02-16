//! waitpid/wait4 implementation for ARM64

use super::errno::{ECHILD, EFAULT, EINVAL, ENOSYS};
use super::userptr;
use super::SyscallResult;
use crate::arch_impl::traits::CpuOps;

#[cfg(target_arch = "aarch64")]
type Cpu = crate::arch_impl::aarch64::Aarch64Cpu;

/// Ensure TTBR0 is set to the current thread's process page tables.
///
/// After a syscall blocks and resumes (e.g., waitpid blocking until child exits),
/// TTBR0 may have been changed by context switches to other processes. Before
/// accessing user memory, we must restore TTBR0 to the current thread's page tables.
#[cfg(target_arch = "aarch64")]
fn ensure_current_address_space() {
    let thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => return,
    };

    let manager_guard = crate::process::manager();
    if let Some(ref manager) = *manager_guard {
        if let Some((_pid, process)) = manager.find_process_by_thread(thread_id) {
            if let Some(ref page_table) = process.page_table {
                let ttbr0_value = page_table.level_4_frame().start_address().as_u64();
                unsafe {
                    core::arch::asm!(
                        "dsb ishst",           // Ensure previous stores complete
                        "msr ttbr0_el1, {}",   // Set page table
                        "isb",                 // Synchronize context
                        "tlbi vmalle1is",      // Flush TLB
                        "dsb ish",             // Ensure TLB flush completes
                        "isb",                 // Synchronize instruction stream
                        in(reg) ttbr0_value,
                        options(nomem, nostack)
                    );
                }
            }
        }
    }
}

#[cfg(not(target_arch = "aarch64"))]
fn ensure_current_address_space() {
    // On x86_64, CR3 handling is done differently
}

/// waitpid options constants
pub const WNOHANG: u32 = 1;
#[allow(dead_code)]
pub const WUNTRACED: u32 = 2;

/// sys_waitpid - Wait for a child process to change state
///
/// This implements the wait4/waitpid system call.
pub fn sys_waitpid(pid: i64, status_ptr: u64, options: u32) -> SyscallResult {
    log::debug!(
        "sys_waitpid: pid={}, status_ptr={:#x}, options={}",
        pid,
        status_ptr,
        options
    );

    // Get current thread ID
    let thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => {
            log::error!("sys_waitpid: No current thread");
            return SyscallResult::Err(EINVAL as u64);
        }
    };

    // Find current process.
    let is_wnohang = (options & WNOHANG) != 0;
    let mut manager_guard = crate::process::manager();
    let (current_pid, current_process) = match &mut *manager_guard {
        Some(manager) => match manager.find_process_by_thread_mut(thread_id) {
            Some((pid, process)) => (pid, process),
            None => {
                log::error!("sys_waitpid: Thread {} not in any process", thread_id);
                return SyscallResult::Err(EINVAL as u64);
            }
        },
        None => {
            log::error!("sys_waitpid: No process manager");
            return SyscallResult::Err(EINVAL as u64);
        }
    };

    log::debug!(
        "sys_waitpid: Current process PID={}, has {} children",
        current_pid.as_u64(),
        current_process.children.len()
    );

    // Check for children
    if current_process.children.is_empty() {
        log::debug!("sys_waitpid: No children - returning ECHILD");
        return SyscallResult::Err(ECHILD as u64);
    }

    match pid {
        // pid > 0: Wait for specific child
        p if p > 0 => {
            let target_pid = crate::process::ProcessId::new(p as u64);

            if !current_process.children.contains(&target_pid) {
                log::debug!(
                    "sys_waitpid: PID {} is not a child of {}",
                    p,
                    current_pid.as_u64()
                );
                return SyscallResult::Err(ECHILD as u64);
            }

            drop(manager_guard);

            // Check if the specific child is already terminated
            let child_terminated = {
                let manager_guard = crate::process::manager();
                if let Some(ref manager) = *manager_guard {
                    if let Some(child) = manager.get_process(target_pid) {
                        if let crate::process::ProcessState::Terminated(exit_code) = child.state {
                            Some((target_pid, exit_code))
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                } else {
                    None
                }
            };

            if let Some((child_pid, exit_code)) = child_terminated {
                return complete_wait(child_pid, exit_code, status_ptr);
            }

            if options & WNOHANG != 0 {
                log::debug!("sys_waitpid: WNOHANG set, child {} not terminated", p);
                return SyscallResult::Ok(0);
            }

            // Blocking wait
            crate::task::scheduler::with_scheduler(|sched| {
                sched.block_current_for_child_exit();
            });

            crate::per_cpu::preempt_enable();

            loop {
                // Check for pending signals that should interrupt this syscall
                if let Some(e) = crate::syscall::check_signals_for_eintr() {
                    // Signal pending - clean up thread state and return EINTR
                    crate::task::scheduler::with_scheduler(|sched| {
                        if let Some(thread) = sched.current_thread_mut() {
                            thread.blocked_in_syscall = false;
                            thread.set_ready();
                        }
                    });
                    crate::per_cpu::preempt_disable();
                    log::debug!("sys_waitpid: Thread {} interrupted by signal (EINTR)", thread_id);
                    return SyscallResult::Err(e as u64);
                }

                crate::task::scheduler::yield_current();
                Cpu::halt_with_interrupts();

                let manager_guard = crate::process::manager();
                if let Some(ref manager) = *manager_guard {
                    if let Some(child) = manager.get_process(target_pid) {
                        if let crate::process::ProcessState::Terminated(exit_code) = child.state {
                            drop(manager_guard);
                            crate::per_cpu::preempt_disable();
                            return complete_wait(target_pid, exit_code, status_ptr);
                        }
                    }
                }
            }
        }

        // pid == -1: Wait for any child
        -1 => {
            let children_copy = current_process.children.clone();
            drop(manager_guard);

            let terminated_child = {
                let manager_guard = crate::process::manager();
                if let Some(ref manager) = *manager_guard {
                    let mut result = None;
                    for &child_pid in &children_copy {
                        if let Some(child) = manager.get_process(child_pid) {
                            if let crate::process::ProcessState::Terminated(exit_code) = child.state {
                                result = Some((child_pid, exit_code));
                                break;
                            }
                        }
                    }
                    result
                } else {
                    None
                }
            };

            if let Some((child_pid, exit_code)) = terminated_child {
                return complete_wait(child_pid, exit_code, status_ptr);
            }

            if is_wnohang {
                log::debug!("sys_waitpid: WNOHANG set, no children terminated");
                return SyscallResult::Ok(0);
            }

            crate::task::scheduler::with_scheduler(|sched| {
                sched.block_current_for_child_exit();
            });

            crate::per_cpu::preempt_enable();

            loop {
                // Check for pending signals that should interrupt this syscall
                if let Some(e) = crate::syscall::check_signals_for_eintr() {
                    // Signal pending - clean up thread state and return EINTR
                    crate::task::scheduler::with_scheduler(|sched| {
                        if let Some(thread) = sched.current_thread_mut() {
                            thread.blocked_in_syscall = false;
                            thread.set_ready();
                        }
                    });
                    crate::per_cpu::preempt_disable();
                    log::debug!("sys_waitpid: Thread {} interrupted by signal (EINTR)", thread_id);
                    return SyscallResult::Err(e as u64);
                }

                crate::task::scheduler::yield_current();
                Cpu::halt_with_interrupts();

                let manager_guard = crate::process::manager();
                if let Some(ref manager) = *manager_guard {
                    for &child_pid in &children_copy {
                        if let Some(child) = manager.get_process(child_pid) {
                            if let crate::process::ProcessState::Terminated(exit_code) = child.state {
                                drop(manager_guard);
                                crate::per_cpu::preempt_disable();
                                return complete_wait(child_pid, exit_code, status_ptr);
                            }
                        }
                    }
                }
            }
        }

        // pid == 0 or pid < -1: Process groups not implemented
        _ => {
            log::warn!("sys_waitpid: Process groups not implemented (pid={})", pid);
            SyscallResult::Err(ENOSYS as u64)
        }
    }
}

/// Helper function to complete a wait operation
fn complete_wait(child_pid: crate::process::ProcessId, exit_code: i32, status_ptr: u64) -> SyscallResult {
    let wstatus: i32 = if exit_code < 0 {
        let signal_number = (-exit_code) as i32;
        let core_dump = (signal_number & 0x80) != 0;
        let sig = signal_number & 0x7f;
        sig | (if core_dump { 0x80 } else { 0 })
    } else {
        (exit_code & 0xff) << 8
    };

    log::debug!(
        "complete_wait: child {} exited with code {}, wstatus={:#x}{}",
        child_pid.as_u64(),
        exit_code,
        wstatus,
        if exit_code < 0 {
            " (signal termination)"
        } else {
            " (normal exit)"
        }
    );

    if status_ptr != 0 {
        // CRITICAL: Restore TTBR0 to current process's page tables before accessing user memory.
        // After blocking in waitpid and resuming, TTBR0 may have been changed by context
        // switches to other processes. Without this, we'd fault trying to access the
        // parent's user stack through the wrong page tables.
        ensure_current_address_space();

        let user_ptr = status_ptr as *mut i32;
        if userptr::copy_to_user(user_ptr, &wstatus).is_err() {
            log::error!("complete_wait: Failed to write status");
            return SyscallResult::Err(EFAULT as u64);
        }
    }

    // Remove child from parent's children list and reap from process table
    if let Some(thread_id) = crate::task::scheduler::current_thread_id() {
        let mut manager_guard = crate::process::manager();
        if let Some(ref mut manager) = *manager_guard {
            if let Some((_parent_pid, parent)) = manager.find_process_by_thread_mut(thread_id) {
                parent.children.retain(|&id| id != child_pid);
                log::debug!(
                    "complete_wait: Removed child {} from parent's children list",
                    child_pid.as_u64()
                );
            }
            manager.remove_process(child_pid);
            log::debug!("complete_wait: Reaped process {} from process table", child_pid.as_u64());
        }
    }

    // Clear blocked_in_syscall flag
    crate::task::scheduler::with_scheduler(|sched| {
        if let Some(thread) = sched.current_thread_mut() {
            if thread.blocked_in_syscall {
                thread.blocked_in_syscall = false;
                log::debug!(
                    "complete_wait: Cleared blocked_in_syscall flag for thread {}",
                    thread.id
                );
            }
        }
    });

    SyscallResult::Ok(child_pid.as_u64())
}

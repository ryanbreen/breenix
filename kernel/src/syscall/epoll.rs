//! epoll implementation for Breenix
//!
//! Provides epoll_create1, epoll_ctl, and epoll_wait/epoll_pwait syscalls
//! for I/O event notification.
//!
//! This implementation reuses the existing poll infrastructure
//! (`ipc::poll::poll_fd`) for readiness checking, providing an epoll-compatible
//! API on top.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

use super::errno;
use super::SyscallResult;
use crate::ipc::fd::{FdKind, FileDescriptor};
use crate::ipc::poll;

// =============================================================================
// epoll constants (matching Linux ABI)
// =============================================================================

/// epoll_ctl operations
const EPOLL_CTL_ADD: i32 = 1;
const EPOLL_CTL_DEL: i32 = 2;
const EPOLL_CTL_MOD: i32 = 3;

/// epoll event flags
const EPOLLIN: u32 = 0x001;
const EPOLLOUT: u32 = 0x004;
const EPOLLERR: u32 = 0x008;
const EPOLLHUP: u32 = 0x010;

// =============================================================================
// Data structures
// =============================================================================

/// epoll_event structure matching Linux ABI (packed on x86_64)
#[repr(C)]
#[cfg_attr(target_arch = "x86_64", repr(packed))]
#[derive(Clone, Copy)]
pub struct EpollEvent {
    pub events: u32,
    pub data: u64,
}

/// A single entry in an epoll instance's interest list
#[derive(Clone)]
struct EpollEntry {
    fd: i32,
    events: u32,
    data: u64,
}

/// An epoll instance containing registered file descriptors
struct EpollInstance {
    entries: Vec<EpollEntry>,
}

impl EpollInstance {
    fn new() -> Self {
        EpollInstance {
            entries: Vec::new(),
        }
    }

    fn add(&mut self, fd: i32, events: u32, data: u64) -> Result<(), i32> {
        // Check for duplicate
        if self.entries.iter().any(|e| e.fd == fd) {
            return Err(errno::EEXIST);
        }
        self.entries.push(EpollEntry { fd, events, data });
        Ok(())
    }

    fn modify(&mut self, fd: i32, events: u32, data: u64) -> Result<(), i32> {
        for entry in self.entries.iter_mut() {
            if entry.fd == fd {
                entry.events = events;
                entry.data = data;
                return Ok(());
            }
        }
        Err(errno::ENOENT)
    }

    fn delete(&mut self, fd: i32) -> Result<(), i32> {
        let len_before = self.entries.len();
        self.entries.retain(|e| e.fd != fd);
        if self.entries.len() == len_before {
            Err(errno::ENOENT)
        } else {
            Ok(())
        }
    }
}

// =============================================================================
// Global epoll instance registry
// =============================================================================

/// Next unique epoll instance ID
static NEXT_EPOLL_ID: AtomicU64 = AtomicU64::new(1);

/// Global registry of epoll instances, keyed by instance ID.
/// Protected by a spinlock. The Vec is small (typically <10 entries).
static EPOLL_INSTANCES: Mutex<Vec<(u64, EpollInstance)>> = Mutex::new(Vec::new());

fn alloc_instance() -> u64 {
    let id = NEXT_EPOLL_ID.fetch_add(1, Ordering::Relaxed);
    let mut instances = EPOLL_INSTANCES.lock();
    instances.push((id, EpollInstance::new()));
    id
}

/// Remove an epoll instance by ID. Called from FdTable::Drop.
pub fn remove_instance(id: u64) {
    let mut instances = EPOLL_INSTANCES.lock();
    instances.retain(|(iid, _)| *iid != id);
}

fn with_instance<F, R>(id: u64, f: F) -> Result<R, i32>
where
    F: FnOnce(&mut EpollInstance) -> Result<R, i32>,
{
    let mut instances = EPOLL_INSTANCES.lock();
    for (iid, instance) in instances.iter_mut() {
        if *iid == id {
            return f(instance);
        }
    }
    Err(errno::EBADF)
}

// =============================================================================
// Syscall implementations
// =============================================================================

/// epoll_create1(flags) -> fd
///
/// Creates a new epoll instance. Returns a file descriptor referring to the
/// new epoll instance. The flags argument is currently ignored (EPOLL_CLOEXEC
/// would be the only valid flag).
pub fn sys_epoll_create1(_flags: u32) -> SyscallResult {
    let epoll_id = alloc_instance();

    // Get current thread to find its process and fd table
    let thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => return SyscallResult::Err(errno::ESRCH as u64),
    };

    let mut manager_guard = crate::process::manager();
    let manager = match manager_guard.as_mut() {
        Some(m) => m,
        None => {
            remove_instance(epoll_id);
            return SyscallResult::Err(errno::ENOMEM as u64);
        }
    };

    let (_pid, process) = match manager.find_process_by_thread_mut(thread_id) {
        Some(p) => p,
        None => {
            remove_instance(epoll_id);
            return SyscallResult::Err(errno::ESRCH as u64);
        }
    };

    match process.fd_table.alloc(FdKind::Epoll(epoll_id)) {
        Ok(fd) => SyscallResult::Ok(fd as u64),
        Err(e) => {
            remove_instance(epoll_id);
            SyscallResult::Err(e as u64)
        }
    }
}

/// epoll_ctl(epfd, op, fd, event_ptr) -> 0 or -errno
///
/// Control interface for an epoll instance. Adds, modifies, or removes
/// entries in the interest list of the epoll instance referred to by epfd.
pub fn sys_epoll_ctl(epfd: i32, op: i32, fd: i32, event_ptr: u64) -> SyscallResult {
    // Look up the epoll instance ID from the fd
    let thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => return SyscallResult::Err(errno::ESRCH as u64),
    };

    let manager_guard = crate::process::manager();
    let manager = match manager_guard.as_ref() {
        Some(m) => m,
        None => return SyscallResult::Err(errno::ENOMEM as u64),
    };

    let (_pid, process) = match manager.find_process_by_thread(thread_id) {
        Some(p) => p,
        None => return SyscallResult::Err(errno::ESRCH as u64),
    };

    // Validate epfd is an epoll fd
    let epoll_id = match process.fd_table.get(epfd) {
        Some(entry) => match &entry.kind {
            FdKind::Epoll(id) => *id,
            _ => return SyscallResult::Err(errno::EINVAL as u64),
        },
        None => return SyscallResult::Err(errno::EBADF as u64),
    };

    // Validate the target fd exists (except for DEL which doesn't need event)
    if op != EPOLL_CTL_DEL {
        if process.fd_table.get(fd).is_none() {
            return SyscallResult::Err(errno::EBADF as u64);
        }
    }

    // Read the event structure from userspace (not needed for DEL)
    let (events, data) = if op != EPOLL_CTL_DEL {
        if event_ptr == 0 {
            return SyscallResult::Err(errno::EFAULT as u64);
        }
        let event: EpollEvent = match super::userptr::copy_from_user(event_ptr as *const EpollEvent) {
            Ok(e) => e,
            Err(e) => return SyscallResult::Err(e),
        };
        (event.events, event.data)
    } else {
        (0, 0)
    };

    // Drop manager guard before accessing EPOLL_INSTANCES to avoid lock ordering issues
    drop(manager_guard);

    let result = match op {
        EPOLL_CTL_ADD => with_instance(epoll_id, |inst| inst.add(fd, events, data)),
        EPOLL_CTL_MOD => with_instance(epoll_id, |inst| inst.modify(fd, events, data)),
        EPOLL_CTL_DEL => with_instance(epoll_id, |inst| inst.delete(fd)),
        _ => Err(errno::EINVAL),
    };

    match result {
        Ok(()) => SyscallResult::Ok(0),
        Err(e) => SyscallResult::Err(e as u64),
    }
}

/// epoll_pwait(epfd, events_ptr, maxevents, timeout, sigmask_ptr, sigsetsize) -> count or -errno
///
/// Wait for events on an epoll instance. Returns ready events in the
/// user-provided buffer. Reuses poll_fd for readiness checking.
pub fn sys_epoll_pwait(
    epfd: i32,
    events_ptr: u64,
    maxevents: i32,
    timeout: i32,
    _sigmask_ptr: u64,
    _sigsetsize: u64,
) -> SyscallResult {
    if maxevents <= 0 {
        return SyscallResult::Err(errno::EINVAL as u64);
    }
    if events_ptr == 0 {
        return SyscallResult::Err(errno::EFAULT as u64);
    }

    let maxevents = maxevents as usize;

    // Look up the epoll instance ID
    let thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => return SyscallResult::Err(errno::ESRCH as u64),
    };

    let epoll_id = {
        let manager_guard = crate::process::manager();
        let manager = match manager_guard.as_ref() {
            Some(m) => m,
            None => return SyscallResult::Err(errno::ENOMEM as u64),
        };
        let (_pid, process) = match manager.find_process_by_thread(thread_id) {
            Some(p) => p,
            None => return SyscallResult::Err(errno::ESRCH as u64),
        };
        match process.fd_table.get(epfd) {
            Some(entry) => match &entry.kind {
                FdKind::Epoll(id) => *id,
                _ => return SyscallResult::Err(errno::EINVAL as u64),
            },
            None => return SyscallResult::Err(errno::EBADF as u64),
        }
    }; // manager guard dropped

    // Calculate wake-up deadline for timeouts
    let deadline_ns = if timeout > 0 {
        let (s, n) = crate::time::get_monotonic_time_ns();
        let now = (s as u64) * 1_000_000_000 + (n as u64);
        Some(now + (timeout as u64) * 1_000_000)
    } else if timeout == 0 {
        Some(0) // non-blocking
    } else {
        None // infinite
    };

    loop {
        // Snapshot the registered entries from the epoll instance
        let entries: Vec<EpollEntry> = {
            let instances = EPOLL_INSTANCES.lock();
            let mut found = None;
            for (iid, instance) in instances.iter() {
                if *iid == epoll_id {
                    found = Some(instance.entries.clone());
                    break;
                }
            }
            match found {
                Some(e) => e,
                None => return SyscallResult::Err(errno::EBADF as u64),
            }
        };

        // Snapshot fd table entries for the registered fds
        let fd_snapshots: Vec<(EpollEntry, Option<FileDescriptor>)> = {
            let manager_guard = crate::process::manager();
            let manager = match manager_guard.as_ref() {
                Some(m) => m,
                None => return SyscallResult::Err(errno::ENOMEM as u64),
            };
            let (_pid, process) = match manager.find_process_by_thread(thread_id) {
                Some(p) => p,
                None => return SyscallResult::Err(errno::ESRCH as u64),
            };

            entries
                .into_iter()
                .map(|entry| {
                    let fd_entry = process.fd_table.get(entry.fd).cloned();
                    (entry, fd_entry)
                })
                .collect()
        }; // manager guard dropped

        // Check readiness for each registered fd
        let mut ready_events: Vec<EpollEvent> = Vec::new();
        for (entry, fd_entry) in fd_snapshots.iter() {
            if let Some(ref fd) = fd_entry {
                // Convert epoll event mask to poll events
                let poll_events = epoll_to_poll_events(entry.events);
                let revents = poll::poll_fd(fd, poll_events);
                let epoll_revents = poll_to_epoll_events(revents);

                if epoll_revents != 0 {
                    ready_events.push(EpollEvent {
                        events: epoll_revents,
                        data: entry.data,
                    });
                    if ready_events.len() >= maxevents {
                        break;
                    }
                }
            }
        }

        // If events are ready, copy to userspace and return
        if !ready_events.is_empty() {
            let count = ready_events.len();
            unsafe {
                let dst = events_ptr as *mut EpollEvent;
                for (i, event) in ready_events.iter().enumerate() {
                    core::ptr::write(dst.add(i), *event);
                }
            }
            return SyscallResult::Ok(count as u64);
        }

        // No events ready - check timeout
        if let Some(deadline) = deadline_ns {
            if deadline == 0 {
                // Non-blocking (timeout=0)
                return SyscallResult::Ok(0);
            }
            let (s, n) = crate::time::get_monotonic_time_ns();
            let now = (s as u64) * 1_000_000_000 + (n as u64);
            if now >= deadline {
                return SyscallResult::Ok(0);
            }
        }

        // Check for pending signals that should interrupt the wait
        if let Some(_eintr) = super::check_signals_for_eintr() {
            return SyscallResult::Err(errno::EINTR as u64);
        }

        // Yield and retry
        crate::task::scheduler::yield_current();
    }
}

// =============================================================================
// Event mask conversion helpers
// =============================================================================

/// Convert epoll event mask to poll events (i16)
fn epoll_to_poll_events(epoll_events: u32) -> i16 {
    let mut poll_events: i16 = 0;
    if epoll_events & EPOLLIN != 0 {
        poll_events |= poll::events::POLLIN;
    }
    if epoll_events & EPOLLOUT != 0 {
        poll_events |= poll::events::POLLOUT;
    }
    // EPOLLERR and EPOLLHUP are always reported (output-only in poll)
    poll_events
}

/// Convert poll revents (i16) to epoll event mask
fn poll_to_epoll_events(revents: i16) -> u32 {
    let mut epoll_events: u32 = 0;
    if revents & poll::events::POLLIN != 0 {
        epoll_events |= EPOLLIN;
    }
    if revents & poll::events::POLLOUT != 0 {
        epoll_events |= EPOLLOUT;
    }
    if revents & poll::events::POLLERR != 0 {
        epoll_events |= EPOLLERR;
    }
    if revents & poll::events::POLLHUP != 0 {
        epoll_events |= EPOLLHUP;
    }
    epoll_events
}

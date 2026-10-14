//! epoll: epoll_create1, epoll_ctl and epoll_wait/epoll_pwait.
//!
//! An epoll instance is shared by every descriptor for it (dup, fork) and
//! goes with the last one. As on Linux it watches open file descriptions: an
//! interest is the descriptor number it was added under and the description
//! that number referred to then. Closing that number while another
//! descriptor for the description stays open keeps the interest; a new file
//! opened under the same number is a different file. The interest goes when
//! the description is closed everywhere.
//!
//! Readiness comes from `ipc::poll::poll_fd` on a descriptor in the caller's
//! table that refers to the description. A wait records what it reports
//! under the instance lock, checking each interest's generation, so two
//! concurrent waits never both take one edge or one-shot event, and an
//! EPOLL_CTL_MOD made while a wait looked is never overwritten.

use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::sync::atomic::AtomicU32;
use spin::Mutex;

use super::errno;
use super::multiplex::{with_mask, WaitSet};
use super::SyscallResult;
use crate::ipc::fd::{flags, FdKind, FileDescriptor};
use crate::ipc::poll;
use crate::task::waitqueue::WaitQueueHead;

// =============================================================================
// epoll constants (matching Linux ABI)
// =============================================================================

/// epoll_ctl operations
const EPOLL_CTL_ADD: i32 = 1;
const EPOLL_CTL_DEL: i32 = 2;
const EPOLL_CTL_MOD: i32 = 3;

/// epoll_create1's only flag, O_CLOEXEC's value.
const EPOLL_CLOEXEC: u32 = 0o2000000;

/// epoll event flags
const EPOLLIN: u32 = 0x001;
const EPOLLPRI: u32 = 0x002;
const EPOLLOUT: u32 = 0x004;
const EPOLLERR: u32 = 0x008;
const EPOLLHUP: u32 = 0x010;
const EPOLLRDHUP: u32 = 0x2000;
const EPOLLONESHOT: u32 = 1 << 30;
const EPOLLET: u32 = 1 << 31;

/// The conditions a receive-side change can raise, and a send-side one.
const IN_SIDE: u32 = EPOLLIN | EPOLLPRI | EPOLLRDHUP | EPOLLHUP | EPOLLERR;
const OUT_SIDE: u32 = EPOLLOUT | EPOLLHUP | EPOLLERR;

/// A change count an interest has never seen.
const NEVER_SEEN: u64 = u64::MAX;

/// Linux's EP_MAX_EVENTS: the most events one wait may ask for.
const MAX_EVENTS: usize = i32::MAX as usize / core::mem::size_of::<EpollEvent>();

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

/// One watched file.
#[derive(Clone)]
struct Interest {
    fd: i32,
    file: Weak<AtomicU32>,
    events: u32,
    data: u64,
    /// EPOLLET: the file's receive-side and send-side change counts when this
    /// interest last looked (sockets, pipes and FIFOs keep them).
    last_rx: u64,
    last_tx: u64,
    /// EPOLLONESHOT: reported, and waiting for EPOLL_CTL_MOD.
    disabled: bool,
    /// Changed by EPOLL_CTL_MOD and by every state a wait records.
    gen: u64,
}

impl Interest {
    fn same_state(&self, other: &Interest) -> bool {
        self.last_rx == other.last_rx
            && self.last_tx == other.last_tx
            && self.disabled == other.disabled
    }

    fn set_state(&mut self, from: &Interest) {
        self.last_rx = from.last_rx;
        self.last_tx = from.last_tx;
        self.disabled = from.disabled;
    }
}

struct Interests {
    entries: Vec<Interest>,
    next_gen: u64,
}

impl Interests {
    fn next_gen(&mut self) -> u64 {
        self.next_gen += 1;
        self.next_gen
    }

    /// Forget interests whose file is closed everywhere.
    fn prune(&mut self) {
        self.entries.retain(|entry| entry.file.strong_count() != 0);
    }

    fn find(&mut self, fd: i32, file: &FileDescriptor) -> Option<&mut Interest> {
        self.entries
            .iter_mut()
            .find(|entry| entry.fd == fd && file.refers_to(&entry.file))
    }
}

/// An epoll instance: what `FdKind::Epoll` refers to.
pub struct EpollInstance {
    interests: Mutex<Interests>,
    /// Threads in epoll_wait; EPOLL_CTL_ADD and MOD wake them.
    waiters: Arc<WaitQueueHead>,
}

impl EpollInstance {
    fn new() -> Result<Arc<Self>, i32> {
        let waiters = Arc::try_new(WaitQueueHead::new()).map_err(|_| errno::ENOMEM)?;
        Arc::try_new(EpollInstance {
            interests: Mutex::new(Interests {
                entries: Vec::new(),
                next_gen: 0,
            }),
            waiters,
        })
        .map_err(|_| errno::ENOMEM)
    }

    fn ctl(&self, op: i32, fd: i32, file: &FileDescriptor, events: u32, data: u64) -> Result<(), i32> {
        let mut interests = self.interests.lock();
        interests.prune();
        match op {
            EPOLL_CTL_ADD => {
                if interests.find(fd, file).is_some() {
                    return Err(errno::EEXIST);
                }
                interests.entries.try_reserve(1).map_err(|_| errno::ENOMEM)?;
                let gen = interests.next_gen();
                interests.entries.push(Interest {
                    fd,
                    file: file.description_ref(),
                    events,
                    data,
                    last_rx: NEVER_SEEN,
                    last_tx: NEVER_SEEN,
                    disabled: false,
                    gen,
                });
            }
            EPOLL_CTL_MOD => {
                let gen = interests.next_gen();
                let entry = interests.find(fd, file).ok_or(errno::ENOENT)?;
                // Modifying an interest re-arms it, one-shot or edge-triggered.
                entry.events = events;
                entry.data = data;
                entry.last_rx = NEVER_SEEN;
                entry.last_tx = NEVER_SEEN;
                entry.disabled = false;
                entry.gen = gen;
            }
            EPOLL_CTL_DEL => {
                let before = interests.entries.len();
                interests
                    .entries
                    .retain(|entry| !(entry.fd == fd && file.refers_to(&entry.file)));
                if interests.entries.len() == before {
                    return Err(errno::ENOENT);
                }
            }
            _ => return Err(errno::EINVAL),
        }
        Ok(())
    }

    /// The live interests.
    fn snapshot(&self) -> Result<Vec<Interest>, i32> {
        let mut interests = self.interests.lock();
        interests.prune();
        let mut copy = Vec::new();
        copy.try_reserve_exact(interests.entries.len())
            .map_err(|_| errno::ENOMEM)?;
        copy.extend(interests.entries.iter().cloned());
        Ok(copy)
    }

    /// Record what a wait saw, interest by interest, and take the events it
    /// reports, at most `max`. An interest whose generation moved since the
    /// snapshot (MOD, DEL, or another wait recorded it) is skipped: what this
    /// wait saw of it is stale, and its event, if any, belongs to the other.
    fn commit(&self, seen: Vec<(Interest, u32)>, max: usize) -> Result<Vec<Taken>, i32> {
        let mut taken = Vec::new();
        taken.try_reserve_exact(seen.len().min(max)).map_err(|_| errno::ENOMEM)?;
        let mut interests = self.interests.lock();
        for (next, report) in seen {
            if report != 0 && taken.len() == max {
                // No room: leave it to report at the next wait.
                continue;
            }
            let gen = interests.next_gen();
            let Some(entry) = interests.entries.iter_mut().find(|e| e.gen == next.gen) else {
                continue;
            };
            let before = entry.clone();
            if !entry.same_state(&next) {
                entry.set_state(&next);
                entry.gen = gen;
            }
            if report != 0 {
                taken.push(Taken {
                    event: EpollEvent {
                        events: report,
                        data: entry.data,
                    },
                    before,
                    gen: entry.gen,
                });
            }
        }
        Ok(taken)
    }

    /// Put back the state of events that never reached the caller, so they
    /// report again; an interest changed since is left as it is.
    fn restore(&self, untaken: &[Taken]) {
        let mut interests = self.interests.lock();
        for item in untaken {
            let gen = interests.next_gen();
            if let Some(entry) = interests.entries.iter_mut().find(|e| e.gen == item.gen) {
                entry.set_state(&item.before);
                entry.gen = gen;
            }
        }
    }
}

/// An event a wait took, and the interest's state before it took it.
struct Taken {
    event: EpollEvent,
    before: Interest,
    gen: u64,
}

// =============================================================================
// Readiness
// =============================================================================

/// The receive-side and send-side change counts of what `file` refers to.
/// None for a kind that keeps none, or a socket whose protocol does not
/// publish its changes through `notify`.
fn change_counts(file: &FileDescriptor) -> Option<(u64, u64)> {
    match &file.kind {
        FdKind::Socket(socket) if socket.notifies() => Some(socket.common.edges()),
        FdKind::PipeRead(pipe)
        | FdKind::PipeWrite(pipe)
        | FdKind::FifoRead(_, pipe, _)
        | FdKind::FifoWrite(_, pipe, _)
        | FdKind::FifoReadWrite(_, pipe, _) => Some(pipe.lock().edges()),
        _ => None,
    }
}

/// What `interest` reports for `file` now, and its state afterwards.
///
/// Edge-triggered: a report needs a change on the side of a condition that
/// is now present since the interest last looked. The counts are read before
/// readiness, so a change that lands between the two moves them and the next
/// wait reports again. A file with no change counts reports at every wait
/// while it is ready, as level-triggered does: an edge-triggered caller reads
/// until EAGAIN, so a repeated report costs it a read where a missed one
/// would leave it waiting for good.
fn evaluate(interest: &Interest, file: &FileDescriptor) -> (Interest, u32) {
    let counts = change_counts(file);
    let revents = poll_to_epoll_events(poll::poll_fd(file, epoll_to_poll_events(interest.events)));
    let mut next = interest.clone();
    let mut report = revents;
    if interest.events & EPOLLET != 0 {
        if let Some((rx, tx)) = counts {
            let mut changed = 0;
            if rx != interest.last_rx {
                changed |= IN_SIDE;
            }
            if tx != interest.last_tx {
                changed |= OUT_SIDE;
            }
            if revents & changed == 0 {
                report = 0;
            }
            next.last_rx = rx;
            next.last_tx = tx;
        }
    }
    if report != 0 && interest.events & EPOLLONESHOT != 0 {
        next.disabled = true;
    }
    (next, report)
}

/// The caller's descriptor for each interest's file: the number it was added
/// under while that still refers to it, else the lowest that does. None for
/// a file the caller no longer has open.
fn resolve(snapshot: &[Interest]) -> Result<Vec<Option<FileDescriptor>>, i32> {
    let mut files = Vec::new();
    files
        .try_reserve_exact(snapshot.len())
        .map_err(|_| errno::ENOMEM)?;
    let tid = crate::task::scheduler::current_thread_id().ok_or(errno::ESRCH)?;
    let guard = crate::process::manager();
    let manager = guard.as_ref().ok_or(errno::ESRCH)?;
    let (_, process) = manager.find_process_by_thread(tid).ok_or(errno::ESRCH)?;
    for interest in snapshot {
        let file = process
            .fd_table
            .get(interest.fd)
            .filter(|entry| entry.refers_to(&interest.file))
            .or_else(|| {
                process
                    .fd_table
                    .find_description(&interest.file)
                    .map(|(_, entry)| entry)
            })
            .cloned();
        files.push(file);
    }
    Ok(files)
}

// =============================================================================
// Syscall implementations
// =============================================================================

fn current_entry(fd: i32) -> Result<FileDescriptor, i32> {
    let tid = crate::task::scheduler::current_thread_id().ok_or(errno::ESRCH)?;
    let guard = crate::process::manager();
    let manager = guard.as_ref().ok_or(errno::ESRCH)?;
    let (_, process) = manager.find_process_by_thread(tid).ok_or(errno::ESRCH)?;
    process.fd_table.get(fd).cloned().ok_or(errno::EBADF)
}

/// The epoll instance `epfd` refers to: EBADF for no descriptor, EINVAL for
/// one that is not an epoll instance.
fn instance(epfd: i32) -> Result<Arc<EpollInstance>, i32> {
    match current_entry(epfd)?.kind {
        FdKind::Epoll(instance) => Ok(instance),
        _ => Err(errno::EINVAL),
    }
}

fn to_result(result: Result<u64, i32>) -> SyscallResult {
    match result {
        Ok(value) => SyscallResult::Ok(value),
        Err(errno) => SyscallResult::Err(errno as u64),
    }
}

/// epoll_create1(flags) -> fd. EPOLL_CLOEXEC sets FD_CLOEXEC; any other flag
/// is EINVAL.
pub fn sys_epoll_create1(epoll_flags: u32) -> SyscallResult {
    to_result((|| {
        if epoll_flags & !EPOLL_CLOEXEC != 0 {
            return Err(errno::EINVAL);
        }
        let fd_flags = if epoll_flags & EPOLL_CLOEXEC != 0 {
            flags::FD_CLOEXEC
        } else {
            0
        };
        let entry = FileDescriptor::try_with_flags(FdKind::Epoll(EpollInstance::new()?), fd_flags, 0)?;
        let tid = crate::task::scheduler::current_thread_id().ok_or(errno::ESRCH)?;
        let mut guard = crate::process::manager();
        let manager = guard.as_mut().ok_or(errno::ESRCH)?;
        let (_, process) = manager.find_process_by_thread_mut(tid).ok_or(errno::ESRCH)?;
        Ok(process.fd_table.alloc_with_entry(entry)? as u64)
    })())
}

/// epoll_ctl(epfd, op, fd, event_ptr) -> 0 or -errno
///
/// Control interface for an epoll instance. Adds, modifies, or removes
/// entries in the interest list of the epoll instance referred to by epfd.
pub fn sys_epoll_ctl(epfd: i32, op: i32, fd: i32, event_ptr: u64) -> SyscallResult {
    to_result((|| {
        let epoll = instance(epfd)?;
        let file = current_entry(fd)?;
        if let FdKind::Epoll(target) = &file.kind {
            if Arc::ptr_eq(target, &epoll) {
                return Err(errno::EINVAL);
            }
        }
        // The event is read with PM released: a fault here may need it.
        let (events, data) = if op != EPOLL_CTL_DEL {
            let event: EpollEvent = super::userptr::copy_from_user(event_ptr as *const EpollEvent)
                .map_err(|e| e as i32)?;
            (event.events, event.data)
        } else {
            (0, 0)
        };
        epoll.ctl(op, fd, &file, events, data)?;
        drop(file);
        if op != EPOLL_CTL_DEL {
            // A waiter re-reads the interests, and reports one already ready.
            epoll.waiters.wake_up();
        }
        Ok(0)
    })())
}

/// epoll_pwait(epfd, events_ptr, maxevents, timeout, sigmask_ptr,
/// sigsetsize). A signal mask is in force for the wait, as ppoll's is.
pub fn sys_epoll_pwait(
    epfd: i32,
    events_ptr: u64,
    maxevents: i32,
    timeout: i32,
    sigmask_ptr: u64,
    sigsetsize: u64,
) -> SyscallResult {
    let result = with_mask(sigmask_ptr, sigsetsize, || {
        wait(epfd, events_ptr, maxevents, timeout).map_err(|errno| errno as u64)
    });
    match result {
        Ok(count) => SyscallResult::Ok(count),
        Err(errno) => SyscallResult::Err(errno),
    }
}

fn now_ns() -> u64 {
    let (s, n) = crate::time::get_monotonic_time_ns();
    (s as u64)
        .saturating_mul(1_000_000_000)
        .saturating_add(n as u64)
}

fn wait(epfd: i32, events_ptr: u64, maxevents: i32, timeout: i32) -> Result<u64, i32> {
    if maxevents <= 0 || maxevents as usize > MAX_EVENTS {
        return Err(errno::EINVAL);
    }
    let max = maxevents as usize;
    if !crate::memory::layout::is_valid_user_range(
        events_ptr,
        max * core::mem::size_of::<EpollEvent>(),
    ) {
        return Err(errno::EFAULT);
    }
    let epoll = instance(epfd)?;
    let deadline = match timeout {
        0 => Some(0),
        t if t > 0 => Some(now_ns().saturating_add(t as u64 * 1_000_000)),
        _ => None,
    };

    loop {
        crate::net::drain_loopback_queue();
        let snapshot = epoll.snapshot()?;
        let files = resolve(&snapshot)?;

        let mut seen = Vec::new();
        seen.try_reserve_exact(snapshot.len())
            .map_err(|_| errno::ENOMEM)?;
        for (interest, file) in snapshot.iter().zip(&files) {
            if interest.disabled {
                continue;
            }
            let Some(file) = file else {
                continue;
            };
            let (next, report) = evaluate(interest, file);
            if report != 0 || !next.same_state(interest) {
                seen.push((next, report));
            }
        }
        let taken = epoll.commit(seen, max)?;

        if !taken.is_empty() {
            let dst = events_ptr as *mut EpollEvent;
            for (i, item) in taken.iter().enumerate() {
                if let Err(fault) = super::userptr::copy_to_user(dst.wrapping_add(i), &item.event) {
                    // The events not delivered report again; those delivered
                    // are the result, as Linux's ep_send_events gives.
                    epoll.restore(&taken[i..]);
                    return if i == 0 { Err(fault as i32) } else { Ok(i as u64) };
                }
            }
            return Ok(taken.len() as u64);
        }

        if deadline == Some(0) || deadline.is_some_and(|d| now_ns() >= d) {
            return Ok(0);
        }
        if super::check_signals_for_eintr().is_some() {
            return Err(errno::EINTR);
        }

        let tid = crate::task::scheduler::current_thread_id().ok_or(errno::ESRCH)?;
        let mut waits = WaitSet::new(tid);
        waits.watch_queue(epoll.waiters.clone());
        for (interest, file) in snapshot.iter().zip(&files) {
            if let (false, Some(file)) = (interest.disabled, file) {
                waits.watch(file, epoll_to_poll_events(interest.events));
            }
        }
        waits
            .park(deadline.unwrap_or(u64::MAX), || {
                snapshot.iter().zip(&files).any(|(interest, file)| match file {
                    Some(file) if !interest.disabled => evaluate(interest, file).1 != 0,
                    _ => false,
                })
            })
            .map_err(|errno| errno as i32)?;
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
    if epoll_events & EPOLLPRI != 0 {
        poll_events |= poll::events::POLLPRI;
    }
    if epoll_events & EPOLLOUT != 0 {
        poll_events |= poll::events::POLLOUT;
    }
    if epoll_events & EPOLLRDHUP != 0 {
        poll_events |= poll::events::POLLRDHUP;
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
    if revents & poll::events::POLLPRI != 0 {
        epoll_events |= EPOLLPRI;
    }
    if revents & poll::events::POLLRDHUP != 0 {
        epoll_events |= EPOLLRDHUP;
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

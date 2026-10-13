//! Shared poll/select waits. User buffers are copied before parking, and no
//! process-manager guard survives a readiness check or a scheduler operation.
use super::{userptr, SyscallResult};
use crate::ipc::{
    fd::{FdKind, FileDescriptor, MAX_FDS},
    pipe::PipeBuffer,
    poll::{self, events, PollFd},
};
use crate::socket::object::Socket;
use crate::task::waitqueue::WaitQueueHead;
use alloc::sync::Arc;
use alloc::vec::Vec;

fn result(r: Result<u64, u64>) -> SyscallResult {
    match r {
        Ok(n) => SyscallResult::Ok(n),
        Err(e) => SyscallResult::Err(e),
    }
}
fn now() -> u64 {
    let (s, n) = crate::time::get_monotonic_time_ns();
    (s as u64)
        .saturating_mul(1_000_000_000)
        .saturating_add(n as u64)
}

/// A source a poll-style wait sleeps on.
enum Source {
    /// A pipe or FIFO with data to read.
    PipeReader(Arc<spin::Mutex<PipeBuffer>>),
    /// A socket whose protocol publishes through `notify`.
    Socket(Arc<Socket>),
    /// A wait queue: a pipe's writers, an epoll instance's waiters.
    Queue(Arc<WaitQueueHead>),
}

/// The sources one poll, select or epoll wait sleeps on. A descriptor with
/// no wake source makes the wait rescan every millisecond.
pub(super) struct WaitSet {
    tid: u64,
    sources: Vec<Source>,
    notified: bool,
}

impl WaitSet {
    pub(super) fn new(tid: u64) -> Self {
        WaitSet {
            tid,
            sources: Vec::new(),
            notified: true,
        }
    }

    fn add(&mut self, source: Source) {
        if self.sources.try_reserve(1).is_ok() {
            self.sources.push(source);
        } else {
            self.notified = false;
        }
    }

    /// Sleep on what wakes `entry` for `wanted` (poll events).
    pub(super) fn watch(&mut self, entry: &FileDescriptor, wanted: i16) {
        let mut watched = false;
        if wanted & (events::POLLIN | events::POLLPRI | events::POLLRDHUP) != 0 {
            match &entry.kind {
                FdKind::PipeRead(pipe) | FdKind::FifoRead(_, pipe, _) | FdKind::FifoReadWrite(_, pipe, _) => {
                    self.add(Source::PipeReader(pipe.clone()));
                    watched = true;
                }
                _ => {}
            }
        }
        if wanted & events::POLLOUT != 0 {
            match &entry.kind {
                FdKind::PipeWrite(pipe) | FdKind::FifoWrite(_, pipe, _) | FdKind::FifoReadWrite(_, pipe, _) => {
                    let queue = pipe.lock().write_waiters.clone();
                    self.add(Source::Queue(queue));
                    watched = true;
                }
                _ => {}
            }
        }
        match &entry.kind {
            // A socket whose protocol publishes through notify() wakes the
            // wait; one that still wakes by thread id keeps the rescan.
            FdKind::Socket(socket) if socket.notifies() => {
                self.add(Source::Socket(socket.clone()));
            }
            _ if watched => {}
            _ => self.notified = false,
        }
    }

    /// Sleep on `queue` as well.
    pub(super) fn watch_queue(&mut self, queue: Arc<WaitQueueHead>) {
        self.add(Source::Queue(queue));
    }

    /// Put this thread on every source's waiters. False when one could not
    /// take it (no memory).
    fn register(&self) -> bool {
        let mut all = true;
        for source in &self.sources {
            all &= match source {
                Source::PipeReader(pipe) => pipe.lock().try_add_read_waiter(self.tid).is_ok(),
                Source::Socket(socket) => socket.common.wq.add_waiter(self.tid),
                Source::Queue(queue) => queue.add_waiter(self.tid),
            };
        }
        all
    }

    fn unregister(&self) {
        for source in &self.sources {
            match source {
                Source::PipeReader(pipe) => pipe.lock().remove_read_waiter(self.tid),
                Source::Socket(socket) => {
                    socket.common.wq.take_waiter(self.tid);
                }
                Source::Queue(queue) => {
                    queue.take_waiter(self.tid);
                }
            }
        }
    }

    /// Sleep until a source wakes the thread, `deadline` passes or a signal
    /// arrives (Err(EINTR)); `ready` says whether to stay awake.
    ///
    /// The blocked state is published, then the thread registers on every
    /// source, then `ready` runs. A wake after the registration finds the
    /// thread blocked; a change before it is seen by `ready`. A wake that
    /// emptied a queue before the publication (a change `ready` does not
    /// count) cannot leave the thread asleep unregistered, since it registers
    /// again after publishing.
    ///
    /// The recheck runs before preemption is enabled. It takes descriptor
    /// locks (a pipe buffer's) while the thread is already marked blocked,
    /// and an interrupt that preempted it there would switch it out holding
    /// the lock, for good: the writer that would wake it spins on that lock
    /// first. The signal check runs before it too (#1230): see
    /// `blocking_io::wait_prepared`.
    pub(super) fn park(&self, deadline: u64, ready: impl FnOnce() -> bool) -> Result<(), u64> {
        let wake = if self.notified {
            deadline
        } else {
            deadline.min(now().saturating_add(1_000_000))
        };
        // Registered before the publication too, so the queues have room
        // and the second registration only replaces a waiter a wake took.
        let registered = self.register();
        crate::task::scheduler::with_scheduler(|s| {
            s.block_current_for_io_with_timeout((wake != u64::MAX).then_some(wake));
        });
        let registered = self.register() && registered;
        // A source that could not take the waiter is rescanned at once.
        let already_ready = !registered || ready();
        let mut interrupted = false;
        while !already_ready {
            if super::check_signals_for_wait().is_some() {
                interrupted = true;
                break;
            }
            let blocked = crate::task::scheduler::with_scheduler(|s| {
                s.wake_expired_timers();
                s.current_thread_mut()
                    .is_some_and(|t| t.state == crate::task::thread::ThreadState::BlockedOnIO)
            })
            .unwrap_or(false);
            if !blocked {
                break;
            }
            crate::per_cpu::preempt_enable();
            crate::task::scheduler::yield_current();
            crate::arch_halt_while_blocked();
            crate::per_cpu::preempt_disable();
        }
        crate::task::scheduler::with_scheduler(|s| {
            if let Some(t) = s.current_thread_mut() {
                t.blocked_in_syscall = false;
                t.wake_time_ns = None;
                if interrupted || already_ready {
                    t.set_ready();
                }
            }
        });
        self.unregister();
        #[cfg(target_arch = "aarch64")]
        super::handlers::poll_ensure_address_space();
        if interrupted {
            Err(4)
        } else {
            Ok(())
        }
    }
}

fn snapshots(fds: &[PollFd]) -> Result<Vec<Option<FileDescriptor>>, u64> {
    let tid = crate::task::scheduler::current_thread_id().ok_or(3u64)?;
    let guard = crate::process::manager();
    let manager = guard.as_ref().ok_or(3u64)?;
    let (_, process) = manager.find_process_by_thread(tid).ok_or(3u64)?;
    Ok(fds
        .iter()
        .map(|f| process.fd_table.get(f.fd).cloned())
        .collect())
}

fn wait(fds: &mut [PollFd], timeout: Option<u64>, select: bool) -> Result<u64, u64> {
    let entries = snapshots(fds)?;
    if select && entries.iter().any(Option::is_none) {
        return Err(9);
    }
    let start = now();
    let deadline = timeout.map(|n| start.saturating_add(n)).unwrap_or(u64::MAX);
    loop {
        crate::net::drain_loopback_queue();
        let mut ready = 0;
        for (f, entry) in fds.iter_mut().zip(&entries) {
            f.revents = if f.fd < 0 {
                0
            } else if let Some(entry) = entry {
                poll::poll_fd(entry, f.events)
            } else {
                events::POLLNVAL
            };
            // select's exception set asks for priority data, not HUP/ERR.
            let interested = if select {
                ((f.events & events::POLLIN != 0)
                    && (f.revents & (events::POLLIN | events::POLLHUP | events::POLLERR) != 0))
                    || ((f.events & events::POLLOUT != 0)
                        && (f.revents & (events::POLLOUT | events::POLLERR) != 0))
                    || (f.events & f.revents & events::POLLPRI != 0)
            } else {
                f.revents != 0
            };
            if interested {
                ready += 1;
            }
        }
        if ready != 0 {
            return Ok(ready);
        }
        if now() >= deadline {
            if !select {
                let ms = timeout
                    .unwrap_or(0)
                    .div_ceil(1_000_000)
                    .min(i32::MAX as u64) as i32;
                super::handlers::poll_report_timeout(fds, &entries, start, deadline, ms);
            }
            return Ok(0);
        }
        if super::check_signals_for_wait().is_some() {
            return Err(4);
        }
        // Sleep on every descriptor's wake source until an event or the
        // actual deadline, rather than making an indefinite wait runnable
        // every 1 ms; descriptors without one keep the short rescan.
        let tid = crate::task::scheduler::current_thread_id().ok_or(3u64)?;
        let mut waits = WaitSet::new(tid);
        for (f, entry) in fds.iter().zip(&entries) {
            if let (true, Some(entry)) = (f.fd >= 0, entry) {
                waits.watch(entry, f.events);
            }
        }
        let ret = waits.park(deadline, || {
            fds.iter().zip(&entries).any(|(f, entry)| {
                let Some(entry) = entry else {
                    return false;
                };
                let bits = poll::poll_fd(entry, f.events);
                if select {
                    (f.events & events::POLLIN != 0
                        && bits & (events::POLLIN | events::POLLHUP | events::POLLERR) != 0)
                        || (f.events & events::POLLOUT != 0
                            && bits & (events::POLLOUT | events::POLLERR) != 0)
                        || (f.events & bits & events::POLLPRI != 0)
                } else {
                    bits != 0
                }
            })
        });
        ret?;
    }
}

fn descriptor_limit() -> Result<u64, u64> {
    let tid = crate::syscall::memory_common::get_current_thread_id().ok_or(3u64)?;
    let guard = crate::process::manager();
    let (_, process) = guard
        .as_ref()
        .and_then(|m| m.find_process_by_thread(tid))
        .ok_or(3u64)?;
    Ok(process.limits.get(crate::process::limits::NOFILE).soft)
}

fn poll_wait(ptr: u64, nfds: u64, timeout: Option<u64>) -> Result<u64, u64> {
    if nfds > descriptor_limit()? {
        return Err(22);
    }
    let mut fds = alloc::vec![PollFd::default(); nfds as usize];
    userptr::read_user_bytes(
        fds.as_mut_ptr() as *mut u8,
        ptr,
        fds.len() * core::mem::size_of::<PollFd>(),
    )?;
    let ret = wait(&mut fds, timeout, false);
    userptr::write_user_bytes(
        ptr,
        fds.as_ptr() as *const u8,
        fds.len() * core::mem::size_of::<PollFd>(),
    )?;
    ret
}

pub(super) fn poll(ptr: u64, nfds: u64, ms: i32) -> SyscallResult {
    result(poll_wait(
        ptr,
        nfds,
        if ms < 0 {
            None
        } else {
            Some(ms as u64 * 1_000_000)
        },
    ))
}

#[repr(C)]
#[derive(Clone, Copy)]
struct TimePair {
    sec: i64,
    fraction: i64,
}
fn duration(ptr: u64, scale: i64) -> Result<Option<u64>, u64> {
    if ptr == 0 {
        return Ok(None);
    }
    let t: TimePair = userptr::copy_from_user(ptr as *const TimePair)?;
    if t.sec < 0 || t.fraction < 0 || t.fraction >= scale {
        return Err(22);
    }
    Ok(Some(
        (t.sec as u64)
            .saturating_mul(1_000_000_000)
            .saturating_add(t.fraction as u64 * (1_000_000_000 / scale as u64)),
    ))
}

/// Set the temporary mask before any readiness scan. On EINTR leave it in
/// force until delivery selects the interrupting signal, then restores it.
/// Other exits restore immediately, including validation and copy failures.
pub(super) fn with_mask(
    mask: u64,
    size: u64,
    call: impl FnOnce() -> Result<u64, u64>,
) -> Result<u64, u64> {
    if mask == 0 {
        return call();
    }
    if size != 8 {
        return Err(22);
    }
    let new: u64 = userptr::copy_from_user(mask as *const u64)?;
    let tid = crate::task::scheduler::current_thread_id().ok_or(3u64)?;
    let saved = {
        let mut g = crate::process::manager();
        let m = g.as_mut().ok_or(3u64)?;
        let (_, p) = m.find_process_by_thread_mut(tid).ok_or(3u64)?;
        let saved = p.signals.blocked();
        p.signals
            .set_blocked(new & !crate::signal::constants::UNCATCHABLE_SIGNALS);
        m.route_pending_signals_to(tid);
        saved
    };
    let ret = call();
    let mut g = crate::process::manager();
    let m = g.as_mut().ok_or(3u64)?;
    let (_, p) = m.find_process_by_thread_mut(tid).ok_or(3u64)?;
    if ret == Err(4) {
        p.signals.thread.save_wait_mask(saved);
    } else {
        p.signals.set_blocked(saved);
    }
    ret
}

pub(super) fn ppoll(ptr: u64, nfds: u64, ts: u64, mask: u64, size: u64) -> SyscallResult {
    result(
        duration(ts, 1_000_000_000)
            .and_then(|timeout| with_mask(mask, size, || poll_wait(ptr, nfds, timeout))),
    )
}

fn select_wait(nfds: i32, ptrs: [u64; 3], timeout: Option<u64>) -> Result<u64, u64> {
    if nfds < 0 || nfds as u64 > descriptor_limit()? {
        return Err(22);
    }
    let words = (nfds as usize).div_ceil(64);
    let mut sets = [[0u64; MAX_FDS / 64]; 3];
    for (set, ptr) in sets.iter_mut().zip(ptrs) {
        if ptr != 0 {
            userptr::read_user_bytes(set.as_mut_ptr() as *mut u8, ptr, words * 8)?;
        }
    }
    let mut fds = Vec::new();
    for fd in 0..nfds as usize {
        let bit = 1u64 << (fd % 64);
        let mut events = 0;
        for (i, event) in [events::POLLIN, events::POLLOUT, events::POLLPRI]
            .iter()
            .enumerate()
        {
            if sets[i][fd / 64] & bit != 0 {
                events |= event;
            }
        }
        if events != 0 {
            fds.push(PollFd {
                fd: fd as i32,
                events,
                revents: 0,
            });
        }
    }
    wait(&mut fds, timeout, true)?;
    let mut out = [[0u64; MAX_FDS / 64]; 3];
    let mut count = 0;
    for f in fds {
        let read = f.revents & (events::POLLIN | events::POLLHUP | events::POLLERR) != 0;
        let write = f.revents & (events::POLLOUT | events::POLLERR) != 0;
        let except = f.revents & events::POLLPRI != 0;
        for (i, (event, ready)) in [
            (events::POLLIN, read),
            (events::POLLOUT, write),
            (events::POLLPRI, except),
        ]
        .into_iter()
        .enumerate()
        {
            if f.events & event != 0 && ready {
                out[i][f.fd as usize / 64] |= 1u64 << (f.fd % 64);
                count += 1;
            }
        }
    }
    for (set, ptr) in out.iter().zip(ptrs) {
        if ptr != 0 {
            userptr::write_user_bytes(ptr, set.as_ptr() as *const u8, words * 8)?;
        }
    }
    Ok(count)
}

pub(super) fn select(nfds: i32, read: u64, write: u64, except: u64, tv: u64) -> SyscallResult {
    let start = now();
    result(duration(tv, 1_000_000).and_then(|timeout| {
        let ret = select_wait(nfds, [read, write, except], timeout);
        if let Some(n) = timeout {
            let left = n.saturating_sub(now().saturating_sub(start));
            // Timeout writeback is advisory and cannot erase a completed wait.
            let _ = userptr::copy_to_user(
                tv as *mut TimePair,
                &TimePair {
                    sec: (left / 1_000_000_000) as i64,
                    fraction: (left % 1_000_000_000 / 1000) as i64,
                },
            );
        }
        ret
    }))
}

#[repr(C)]
#[derive(Clone, Copy)]
struct PselectMask {
    ptr: u64,
    size: u64,
}
pub(super) fn pselect6(
    nfds: i32,
    read: u64,
    write: u64,
    except: u64,
    ts: u64,
    arg: u64,
) -> SyscallResult {
    result((|| {
        let timeout = duration(ts, 1_000_000_000)?;
        let mask = if arg == 0 {
            PselectMask { ptr: 0, size: 0 }
        } else {
            userptr::copy_from_user(arg as *const PselectMask)?
        };
        with_mask(mask.ptr, mask.size, || {
            select_wait(nfds, [read, write, except], timeout)
        })
    })())
}

//! POSIX message queues: the queue object, the table of queue names, the
//! operations on them and the system calls (`calls.rs`), which
//! `syscall/mqueue.rs` routes by number.
//!
//! A queue lives while its name is in `MQ_NAMES` or a descriptor refers to
//! it: mq_unlink removes the name, and the last reference frees the queue
//! and its charge in `MQ_USAGE`.
//!
//! A send finds a waiting receiver before it queues: the message goes to the
//! receiver directly and the queue's depth does not change. A receive that
//! makes room moves a waiting sender's message into the queue. Either way the
//! waiter served is the one of highest scheduling rank that has waited
//! longest, and the waiters of that side are woken to check their own entries.
//!
//! Locks: `MQ_NAMES`, then a queue's `inner`, then `MQ_USAGE`, then a wait
//! queue's lock, then the scheduler. No queue lock is held while user memory
//! is copied, a thread is woken, a signal is sent or PROCESS_MANAGER is
//! taken. The queue locks taken under PROCESS_MANAGER are `inner`, by
//! `on_close`, and `MQ_USAGE`, when a queue is freed; neither takes another
//! lock under it.

mod calls;
pub mod dev;
pub mod store;

pub use self::calls::{
    sys_mq_getsetattr, sys_mq_notify, sys_mq_open, sys_mq_timedreceive, sys_mq_timedsend, sys_mq_unlink,
};

use self::store::{Channel, MqAttr, NameTable, Sent, Usage, WaitList};
use super::fd::{status_flags, FileDescriptor};
use super::object::IpcFile;
use super::pipe::PipeBuffer;
use super::wait::{wait_on, IpcDeadline, WaitEnd};
use crate::fs::permissions::Credentials;
use crate::irq_safe_mutex::IrqSafeMutex;
use crate::process::ProcessId;
use crate::syscall::errno::{EACCES, EAGAIN, EBUSY, EEXIST, EINTR, EINVAL, ENOENT, ENOMEM, EPERM, ETIMEDOUT};
use crate::task::waitqueue::WaitQueueHead;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering};
use spin::Mutex;

/// Queue names, without their leading slash, and the queues they name.
pub static MQ_NAMES: IrqSafeMutex<NameTable<Arc<MessageQueue>>> = IrqSafeMutex::new(NameTable::new());

/// The queues that exist and each real user's RLIMIT_MSGQUEUE charge.
static MQ_USAGE: IrqSafeMutex<Usage> = IrqSafeMutex::new(Usage::new());

static NEXT_INO: AtomicU64 = AtomicU64::new(1);

const O_ACCMODE: u32 = 0o3;
const O_RDONLY: u32 = 0o0;
const O_WRONLY: u32 = 0o1;
const O_RDWR: u32 = 0o2;
const O_CREAT: u32 = 0o100;
const O_EXCL: u32 = 0o200;

/// What fstat reports as a queue's size: Linux's fixed size for the text
/// a queue's file in its mqueue filesystem reads as.
const QUEUE_FILE_SIZE: i64 = 80;

/// How a registered process is told a message arrived.
#[derive(Clone)]
pub enum NotifyKind {
    /// SIGEV_NONE: the registration is used up and no signal is sent.
    Nothing,
    /// SIGEV_SIGNAL: this signal is sent with SI_MESGQ and the value.
    Signal(u32),
    /// SIGEV_THREAD: the value is written, as 8 bytes, to this pipe, whose
    /// reader in the C library starts the notification thread.
    Thread(Arc<Mutex<PipeBuffer>>),
}

impl core::fmt::Debug for NotifyKind {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            NotifyKind::Nothing => write!(f, "Nothing"),
            NotifyKind::Signal(signo) => write!(f, "Signal({signo})"),
            NotifyKind::Thread(_) => write!(f, "Thread"),
        }
    }
}

/// An mq_notify registration.
#[derive(Clone, Debug)]
pub struct MqNotify {
    /// The registered process (its thread group ID).
    pub pid: u64,
    /// Its program image at registration (`Process::exec_id`).
    pub exec_id: u64,
    pub value: u64,
    pub kind: NotifyKind,
}

/// The part of a queue its lock guards.
#[derive(Debug)]
pub struct MqInner {
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    /// The messages, the waiting receivers and senders, and the
    /// registration.
    pub chan: Channel<MqNotify>,
}

/// A message queue.
pub struct MessageQueue {
    ino: u64,
    maxmsg: usize,
    msgsize: usize,
    /// The real user charged for the queue, and the bytes charged.
    charge: (u32, u64),
    /// Cleared by mq_unlink: fstat then reports no links.
    linked: AtomicBool,
    /// The number of messages queued, stored under `inner` after each
    /// change, for poll.
    depth: AtomicUsize,
    /// Advanced under `inner` whenever a waiter's operation is completed. A
    /// waiter sleeps only while it has not moved since the waiter last held
    /// `inner`, so no completion is missed.
    events: AtomicU64,
    /// The last send or receive, in CLOCK_REALTIME nanoseconds: fstat's
    /// st_atime, st_mtime and st_ctime.
    changed_ns: AtomicI64,
    inner: IrqSafeMutex<MqInner>,
    receivers: WaitQueueHead,
    senders: WaitQueueHead,
}

/// Which of a queue's wait lists.
#[derive(Clone, Copy, Debug)]
enum Side {
    Senders,
    Receivers,
}

impl MqInner {
    fn waiters(&mut self, side: Side) -> &mut WaitList {
        match side {
            Side::Senders => &mut self.chan.senders,
            Side::Receivers => &mut self.chan.receivers,
        }
    }
}

/// The calling thread, as a waiter on a queue.
#[derive(Clone, Copy, Debug)]
pub struct Caller {
    /// Its thread ID.
    pub tid: u64,
    /// Its scheduling rank (`SchedPolicy::rank`).
    pub rank: u8,
}

fn realtime_ns() -> i64 {
    let (sec, nsec) = crate::time::get_real_time_ns();
    sec.saturating_mul(1_000_000_000).saturating_add(nsec)
}

/// The errno for a wait a signal ended: ERESTARTSYS, which the syscall
/// return path turns into a restart, unless the signal runs a handler
/// installed without SA_RESTART (EINTR). The signal is still pending here,
/// so this decides as the wait did.
fn interrupted() -> u64 {
    crate::syscall::check_signals_for_restartable_wait().unwrap_or(EINTR) as u64
}

impl MessageQueue {
    fn new(maxmsg: usize, msgsize: usize, mode: u32, uid: u32, gid: u32, charge: (u32, u64)) -> Self {
        Self {
            ino: NEXT_INO.fetch_add(1, Ordering::Relaxed),
            maxmsg,
            msgsize,
            charge,
            linked: AtomicBool::new(true),
            depth: AtomicUsize::new(0),
            events: AtomicU64::new(0),
            changed_ns: AtomicI64::new(realtime_ns()),
            inner: IrqSafeMutex::new(MqInner {
                mode,
                uid,
                gid,
                chan: Channel::new(maxmsg),
            }),
            receivers: WaitQueueHead::new(),
            senders: WaitQueueHead::new(),
        }
    }

    pub fn ino(&self) -> u64 {
        self.ino
    }

    pub fn msgsize(&self) -> usize {
        self.msgsize
    }

    fn attr(&self, inner: &MqInner, nonblock: bool) -> MqAttr {
        MqAttr {
            mq_flags: if nonblock { store::O_NONBLOCK } else { 0 },
            mq_maxmsg: self.maxmsg as i64,
            mq_msgsize: self.msgsize as i64,
            mq_curmsgs: inner.chan.msgs.len() as i64,
            reserved: [0; 4],
        }
    }

    /// mq_getsetattr on descriptor `fd`: the attributes, with its O_NONBLOCK
    /// as mq_flags, and with `nonblock` the descriptor's O_NONBLOCK set from
    /// it. The queue's lock is held across both, so a concurrent call sees
    /// the flags as one call or the other left them.
    pub fn getsetattr(&self, fd: &FileDescriptor, nonblock: Option<bool>) -> MqAttr {
        let inner = self.inner.lock();
        let flags = fd.status_flags();
        let attr = self.attr(&inner, flags & status_flags::O_NONBLOCK != 0);
        if let Some(nonblock) = nonblock {
            let kept = flags & status_flags::O_APPEND;
            fd.set_status_flags(kept | if nonblock { status_flags::O_NONBLOCK } else { 0 });
        }
        attr
    }

    /// Record the queue's depth and the time of a send or receive.
    fn touched(&self, inner: &MqInner) {
        self.depth.store(inner.chan.msgs.len(), Ordering::Release);
        self.changed_ns.store(realtime_ns(), Ordering::Relaxed);
    }

    /// Queue `msg` at `prio`, or hand it to a waiting receiver, waiting while
    /// the queue is full unless `nonblock`, until `deadline`. `sender` is the
    /// calling process's thread group ID and real user ID, for a
    /// notification.
    pub fn send(
        &self,
        msg: Vec<u8>,
        prio: u32,
        nonblock: bool,
        deadline: &IpcDeadline,
        sender: (u64, u32),
        me: Caller,
    ) -> Result<(), u64> {
        let mut inner = self.inner.lock();
        let msg = match inner.chan.send(prio, msg) {
            Sent::HandedOff => {
                self.events.fetch_add(1, Ordering::Release);
                self.touched(&inner);
                drop(inner);
                self.receivers.wake_up();
                return Ok(());
            }
            Sent::Queued { notify } => {
                self.touched(&inner);
                drop(inner);
                if let Some(n) = notify {
                    notify_owner(n, sender);
                }
                return Ok(());
            }
            Sent::NoMemory => return Err(ENOMEM as u64),
            Sent::Full(msg) => msg,
        };
        if nonblock {
            return Err(EAGAIN as u64);
        }
        if inner.chan.senders.add(me.tid, me.rank, Some((msg, prio))).is_err() {
            return Err(ENOMEM as u64);
        }
        drop(inner);
        self.wait_done(&self.senders, deadline, me.tid, Side::Senders).map(|_| ())
    }

    /// Take the oldest message of the highest priority, waiting while the
    /// queue is empty unless `nonblock`, until `deadline`.
    pub fn receive(&self, nonblock: bool, deadline: &IpcDeadline, me: Caller) -> Result<(Vec<u8>, u32), u64> {
        let mut inner = self.inner.lock();
        if let Some((taken, served)) = inner.chan.receive() {
            if served {
                self.events.fetch_add(1, Ordering::Release);
            }
            self.touched(&inner);
            drop(inner);
            if served {
                self.senders.wake_up();
            }
            return Ok(taken);
        }
        if nonblock {
            return Err(EAGAIN as u64);
        }
        if inner.chan.receivers.add(me.tid, me.rank, None).is_err() {
            return Err(ENOMEM as u64);
        }
        drop(inner);
        self.wait_done(&self.receivers, deadline, me.tid, Side::Receivers)?
            .msg
            .ok_or(EINVAL as u64)
    }

    /// Wait on `q` until the entry of thread `tid` in the `side` list is
    /// completed, and remove it: the completed entry, or
    /// ETIMEDOUT at `deadline`, or the errno of a signal that ended the wait
    /// (`interrupted`). An entry completed by the time the wait ends is
    /// taken as completed, whatever ended it.
    fn wait_done(
        &self,
        q: &WaitQueueHead,
        deadline: &IpcDeadline,
        tid: u64,
        side: Side,
    ) -> Result<store::Waiter, u64> {
        loop {
            let seen = self.events.load(Ordering::Acquire);
            {
                let mut inner = self.inner.lock();
                let list = inner.waiters(side);
                if list.get(tid).is_some_and(|w| w.done) {
                    return list.remove(tid).ok_or(EINVAL as u64);
                }
            }
            let end = wait_on(q, deadline, || self.events.load(Ordering::Acquire) == seen);
            let failed = match end {
                WaitEnd::Woken => continue,
                WaitEnd::TimedOut => ETIMEDOUT as u64,
                WaitEnd::Interrupted => interrupted(),
            };
            let mut inner = self.inner.lock();
            let entry = inner.waiters(side).remove(tid);
            drop(inner);
            return match entry {
                Some(w) if w.done => Ok(w),
                _ => Err(failed),
            };
        }
    }

    /// mq_notify from process `pid`: register `request`, or, given no
    /// request, remove the process's registration. Registering while any
    /// process is registered is EBUSY.
    pub fn set_notify(&self, pid: u64, request: Option<MqNotify>) -> Result<(), u64> {
        let mut inner = self.inner.lock();
        let removed = match request {
            None => {
                if inner.chan.notify.as_ref().is_some_and(|n| n.pid == pid) {
                    inner.chan.notify.take()
                } else {
                    None
                }
            }
            Some(_) if inner.chan.notify.is_some() => return Err(EBUSY as u64),
            Some(n) => {
                inner.chan.notify = Some(n);
                None
            }
        };
        drop(inner);
        drop(removed);
        Ok(())
    }
}

impl Drop for MessageQueue {
    fn drop(&mut self) {
        MQ_USAGE.lock().release(self.charge.0, self.charge.1);
    }
}

/// Tell a registered process a message arrived: `sender` is the sending
/// process's thread group ID and real user ID. A process that has exited or
/// exec'd since it registered is not told.
fn notify_owner(n: MqNotify, sender: (u64, u32)) {
    match n.kind {
        NotifyKind::Nothing => {}
        NotifyKind::Signal(signo) => {
            let info = crate::signal::SigInfo::mesgq(sender.0 as u32, sender.1, n.value);
            let _ = crate::syscall::signal::send_kernel_signal(ProcessId::new(n.pid), signo, info, n.exec_id);
        }
        NotifyKind::Thread(pipe) => {
            let same_image = crate::process::with_process_manager(|manager| {
                manager.get_process(ProcessId::new(n.pid)).is_some_and(|p| p.exec_id == n.exec_id)
            })
            .unwrap_or(false);
            if same_image {
                // A pipe with no reader or no room loses the notification;
                // the sender is not signalled for it.
                let _ = pipe.lock().try_write(&n.value.to_ne_bytes(), true);
            }
        }
    }
}

/// The permission bits mq_open's access mode needs: EINVAL for O_ACCMODE.
fn wanted_access(oflag: u32) -> Result<u32, u64> {
    match oflag & O_ACCMODE {
        O_RDONLY => Ok(4),
        O_WRONLY => Ok(2),
        O_RDWR => Ok(6),
        _ => Err(EINVAL as u64),
    }
}

/// Who creates a queue: the real user charged for it and their
/// RLIMIT_MSGQUEUE.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Creator {
    pub uid: u32,
    pub limit: u64,
}

/// mq_open of `name` (checked, without its slash) with `oflag`, as `cred`:
/// the queue, created with `mode` and `attr` and charged to `creator` when
/// O_CREAT finds no queue of that name.
pub(crate) fn open(
    name: &[u8],
    oflag: u32,
    mode: u32,
    attr: Option<MqAttr>,
    cred: &Credentials,
    creator: Creator,
) -> Result<Arc<MessageQueue>, u64> {
    let wanted = wanted_access(oflag)?;
    let mut names = MQ_NAMES.lock();
    if let Some(queue) = names.get(name) {
        if oflag & (O_CREAT | O_EXCL) == O_CREAT | O_EXCL {
            return Err(EEXIST as u64);
        }
        let (uid, gid, mode) = {
            let inner = queue.inner.lock();
            (inner.uid, inner.gid, inner.mode)
        };
        if !cred.permits_mode(uid, gid, mode, wanted) {
            return Err(EACCES as u64);
        }
        return Ok(queue.clone());
    }
    if oflag & O_CREAT == 0 {
        return Err(ENOENT as u64);
    }
    let privileged = cred.euid == 0;
    let (maxmsg, msgsize) = store::check_attr(attr.as_ref(), privileged)?;
    let charge = (creator.uid, store::queue_bytes(maxmsg, msgsize));
    MQ_USAGE.lock().charge(charge.0, charge.1, creator.limit, privileged)?;
    // From here a failure drops the queue, whose Drop releases the charge.
    let queue = MessageQueue::new(maxmsg, msgsize, mode & 0o777 & !cred.umask, cred.euid, cred.egid, charge);
    let queue = Arc::try_new(queue).map_err(|_| ENOMEM as u64)?;
    names.insert(name, queue.clone()).map_err(|(errno, _)| errno)?;
    Ok(queue)
}

/// mq_unlink of `name` as `cred`: only the queue's owner or root may remove
/// it. The queue directory is sticky on Linux, so another user's unlink is
/// EPERM there, which this follows.
pub(crate) fn unlink(name: &[u8], cred: &Credentials) -> Result<(), u64> {
    let mut names = MQ_NAMES.lock();
    let queue = names.get(name).ok_or(ENOENT as u64)?;
    let owner = queue.inner.lock().uid;
    if cred.euid != 0 && cred.euid != owner {
        return Err(EPERM as u64);
    }
    let removed = names.remove(name);
    drop(names);
    if let Some(queue) = removed {
        queue.linked.store(false, Ordering::Release);
    }
    Ok(())
}

/// A descriptor's reference to a queue.
pub struct MqFile(pub Arc<MessageQueue>);

impl core::fmt::Debug for MqFile {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "MqFile(ino={})", self.0.ino)
    }
}

impl IpcFile for MqFile {
    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    fn fill_stat(&self, st: &mut crate::syscall::fs::Stat) {
        let inner = self.0.inner.lock();
        st.st_ino = self.0.ino;
        st.st_mode = 0o100000 | inner.mode;
        st.st_nlink = self.0.linked.load(Ordering::Acquire).into();
        st.st_uid = inner.uid;
        st.st_gid = inner.gid;
        st.st_size = QUEUE_FILE_SIZE;
        st.st_blksize = 4096;
        let changed = self.0.changed_ns.load(Ordering::Relaxed);
        let (sec, nsec) = (changed.div_euclid(1_000_000_000), changed.rem_euclid(1_000_000_000));
        st.st_atime = sec;
        st.st_atime_nsec = nsec;
        st.st_mtime = sec;
        st.st_mtime_nsec = nsec;
        st.st_ctime = sec;
        st.st_ctime_nsec = nsec;
    }

    fn poll_events(&self) -> i16 {
        const POLLIN: i16 = 0x1;
        const POLLOUT: i16 = 0x4;
        const POLLRDNORM: i16 = 0x40;
        const POLLWRNORM: i16 = 0x100;
        let depth = self.0.depth.load(Ordering::Acquire);
        let mut events = 0;
        if depth != 0 {
            events |= POLLIN | POLLRDNORM;
        }
        if depth < self.0.maxmsg {
            events |= POLLOUT | POLLWRNORM;
        }
        events
    }

    /// Any close by the registered process removes its registration
    /// (POSIX), including the closes of exit and exec. `pid` is the closing
    /// process's thread group ID (`ipc::object::close_owner`), whichever of
    /// its threads closed.
    fn on_close(&self, pid: ProcessId) {
        let mut inner = self.0.inner.lock();
        let removed = if inner.chan.notify.as_ref().is_some_and(|n| n.pid == pid.as_u64()) {
            inner.chan.notify.take()
        } else {
            None
        };
        drop(inner);
        drop(removed);
    }
}

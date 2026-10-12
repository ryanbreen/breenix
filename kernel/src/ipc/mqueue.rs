//! POSIX message queues: the queue object, the table of queue names, the
//! operations on them and the system calls (`calls.rs`), which
//! `syscall/mqueue.rs` routes by number.
//!
//! A queue lives while its name is in `MQ_NAMES` or a descriptor refers to
//! it: mq_unlink removes the name, and the last close frees the queue.
//!
//! Locks: `MQ_NAMES`, then a queue's `inner`, then a wait queue's lock, then
//! the scheduler. No queue lock is held while user memory is copied, a
//! thread is woken, a signal is sent or PROCESS_MANAGER is taken. The one
//! queue lock taken under PROCESS_MANAGER is `inner`, by `on_close`, which
//! takes no other lock under it.

mod calls;
pub mod dev;
pub mod store;

pub use self::calls::{
    sys_mq_getsetattr, sys_mq_notify, sys_mq_open, sys_mq_timedreceive, sys_mq_timedsend, sys_mq_unlink,
};

use self::store::{MessageStore, MqAttr};
use super::object::IpcFile;
use super::wait::{wait_on, IpcDeadline, WaitEnd};
use crate::fs::permissions::Credentials;
use crate::irq_safe_mutex::IrqSafeMutex;
use crate::process::ProcessId;
use crate::syscall::errno::{EACCES, EAGAIN, EBUSY, EEXIST, EINTR, EINVAL, ENOENT, ETIMEDOUT};
use crate::task::waitqueue::WaitQueueHead;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

/// Queue names, without their leading slash, and the queues they name.
pub static MQ_NAMES: IrqSafeMutex<BTreeMap<String, Arc<MessageQueue>>> =
    IrqSafeMutex::new(BTreeMap::new());

static NEXT_INO: AtomicU64 = AtomicU64::new(1);

const O_ACCMODE: u32 = 0o3;
const O_RDONLY: u32 = 0o0;
const O_WRONLY: u32 = 0o1;
const O_RDWR: u32 = 0o2;
const O_CREAT: u32 = 0o100;
const O_EXCL: u32 = 0o200;

/// How a registered process is told a message arrived.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotifyKind {
    /// SIGEV_NONE: the registration is used up and no signal is sent.
    Nothing,
    /// SIGEV_SIGNAL: the signal is sent with SI_MESGQ and the value.
    Signal,
}

/// An mq_notify registration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MqNotify {
    /// The registered process (its thread group ID).
    pub pid: u64,
    pub signo: u32,
    pub value: u64,
    pub kind: NotifyKind,
}

/// The part of a queue its lock guards.
#[derive(Debug)]
pub struct MqInner {
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub msgs: MessageStore,
    pub notify: Option<MqNotify>,
    /// Receivers that found the queue empty and are waiting for a message.
    /// A message they will take raises no notification.
    pub receivers_waiting: u32,
}

/// A message queue.
pub struct MessageQueue {
    ino: u64,
    maxmsg: usize,
    msgsize: usize,
    /// The number of messages queued, stored under `inner` after each
    /// change. A waiter's condition reads it under the wait queue's lock,
    /// where it may not take `inner`.
    depth: AtomicUsize,
    inner: IrqSafeMutex<MqInner>,
    receivers: WaitQueueHead,
    senders: WaitQueueHead,
}

impl MessageQueue {
    fn new(maxmsg: usize, msgsize: usize, mode: u32, uid: u32, gid: u32) -> Self {
        Self {
            ino: NEXT_INO.fetch_add(1, Ordering::Relaxed),
            maxmsg,
            msgsize,
            depth: AtomicUsize::new(0),
            inner: IrqSafeMutex::new(MqInner {
                mode,
                uid,
                gid,
                msgs: MessageStore::new(maxmsg),
                notify: None,
                receivers_waiting: 0,
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

    /// The queue's attributes, with `nonblock` (the descriptor's O_NONBLOCK)
    /// as mq_flags.
    pub fn attr(&self, nonblock: bool) -> MqAttr {
        let curmsgs = self.inner.lock().msgs.len();
        MqAttr {
            mq_flags: if nonblock { store::O_NONBLOCK } else { 0 },
            mq_maxmsg: self.maxmsg as i64,
            mq_msgsize: self.msgsize as i64,
            mq_curmsgs: curmsgs as i64,
            reserved: [0; 4],
        }
    }

    /// Queue `msg` at `prio`, waiting while the queue is full unless
    /// `nonblock`, until `deadline` (EINVAL or EFAULT in it is reported only
    /// once the call would wait). `sender` is the calling process's thread
    /// group ID and real user ID, for a notification.
    pub fn send(
        &self,
        msg: Vec<u8>,
        prio: u32,
        nonblock: bool,
        deadline: &Result<IpcDeadline, u64>,
        sender: (u64, u32),
    ) -> Result<(), u64> {
        let mut msg = msg;
        let mut waited = false;
        let mut timed_out = false;
        loop {
            let mut inner = self.inner.lock();
            let was_empty = inner.msgs.is_empty();
            match inner.msgs.push(prio, msg) {
                Ok(()) => {
                    self.depth.store(inner.msgs.len(), Ordering::Release);
                    let notify = if was_empty && inner.receivers_waiting == 0 {
                        inner.notify.take()
                    } else {
                        None
                    };
                    drop(inner);
                    self.receivers.wake_up_one();
                    if let Some(n) = notify {
                        notify_owner(n, sender);
                    }
                    return Ok(());
                }
                Err(refused) => msg = refused,
            }
            drop(inner);
            let end = match (nonblock, deadline) {
                (true, _) => Err(EAGAIN as u64),
                (false, Err(e)) => Err(*e),
                (false, Ok(_)) if timed_out => Err(ETIMEDOUT as u64),
                (false, Ok(d)) => {
                    waited = true;
                    match wait_on(&self.senders, d, || self.depth.load(Ordering::Acquire) >= self.maxmsg) {
                        WaitEnd::Interrupted => Err(EINTR as u64),
                        // Try once more before reporting the timeout: room
                        // that came with the deadline is used.
                        WaitEnd::TimedOut => {
                            timed_out = true;
                            Ok(())
                        }
                        WaitEnd::Woken => Ok(()),
                    }
                }
            };
            if let Err(e) = end {
                // A wake this sender took and did not use goes to the next.
                if waited && self.depth.load(Ordering::Acquire) < self.maxmsg {
                    self.senders.wake_up_one();
                }
                return Err(e);
            }
        }
    }

    /// Take the oldest message of the highest priority, waiting while the
    /// queue is empty unless `nonblock`, until `deadline` (as `send`).
    pub fn receive(&self, nonblock: bool, deadline: &Result<IpcDeadline, u64>) -> Result<(Vec<u8>, u32), u64> {
        let mut waited = false;
        let mut timed_out = false;
        loop {
            let mut inner = self.inner.lock();
            if let Some(taken) = inner.msgs.pop() {
                self.depth.store(inner.msgs.len(), Ordering::Release);
                drop(inner);
                self.senders.wake_up_one();
                return Ok(taken);
            }
            let d = match (nonblock, deadline) {
                (true, _) => Err(EAGAIN as u64),
                (false, Err(e)) => Err(*e),
                (false, Ok(_)) if timed_out => Err(ETIMEDOUT as u64),
                (false, Ok(d)) => Ok(*d),
            };
            let d = match d {
                Ok(d) => d,
                Err(e) => {
                    drop(inner);
                    // A wake this receiver took and did not use goes to the next.
                    if waited && self.depth.load(Ordering::Acquire) != 0 {
                        self.receivers.wake_up_one();
                    }
                    return Err(e);
                }
            };
            inner.receivers_waiting += 1;
            drop(inner);
            waited = true;
            let end = wait_on(&self.receivers, &d, || self.depth.load(Ordering::Acquire) == 0);
            self.inner.lock().receivers_waiting -= 1;
            match end {
                WaitEnd::Interrupted => {
                    if self.depth.load(Ordering::Acquire) != 0 {
                        self.receivers.wake_up_one();
                    }
                    return Err(EINTR as u64);
                }
                // Try once more before reporting the timeout.
                WaitEnd::TimedOut => timed_out = true,
                WaitEnd::Woken => {}
            }
        }
    }

    /// mq_notify from process `pid`: register `request`, or, given no
    /// request, remove the process's registration. Registering while any process is
    /// registered is EBUSY.
    pub fn set_notify(&self, pid: u64, request: Option<MqNotify>) -> Result<(), u64> {
        let mut inner = self.inner.lock();
        match request {
            None => {
                if inner.notify.is_some_and(|n| n.pid == pid) {
                    inner.notify = None;
                }
                Ok(())
            }
            Some(_) if inner.notify.is_some() => Err(EBUSY as u64),
            Some(n) => {
                inner.notify = Some(n);
                Ok(())
            }
        }
    }
}

/// Tell a registered process a message arrived: `sender` is the sending
/// process's thread group ID and real user ID.
fn notify_owner(n: MqNotify, sender: (u64, u32)) {
    if n.kind == NotifyKind::Signal {
        let info = crate::signal::SigInfo::mesgq(sender.0 as u32, sender.1, n.value);
        // A registered process that has exited is not told.
        let _ = crate::syscall::signal::send_kernel_signal(ProcessId::new(n.pid), n.signo, info);
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

/// mq_open of `name` (checked, without its slash) with `oflag`, as `cred`:
/// the queue, created with `mode` and `attr` when O_CREAT finds no queue of
/// that name.
pub(crate) fn open(name: &str, oflag: u32, mode: u32, attr: Option<MqAttr>, cred: &Credentials) -> Result<Arc<MessageQueue>, u64> {
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
    let (maxmsg, msgsize) = store::check_attr(attr.as_ref(), cred.euid == 0)?;
    let queue = Arc::new(MessageQueue::new(maxmsg, msgsize, mode & 0o777 & !cred.umask, cred.euid, cred.egid));
    names.insert(String::from(name), queue.clone());
    Ok(queue)
}

/// mq_unlink of `name` as `cred`: only the queue's owner or root may remove
/// it, as the sticky queue directory allows on Linux.
pub(crate) fn unlink(name: &str, cred: &Credentials) -> Result<(), u64> {
    let mut names = MQ_NAMES.lock();
    let queue = names.get(name).ok_or(ENOENT as u64)?;
    let owner = queue.inner.lock().uid;
    if cred.euid != 0 && cred.euid != owner {
        return Err(EACCES as u64);
    }
    names.remove(name);
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
        st.st_nlink = 1;
        st.st_uid = inner.uid;
        st.st_gid = inner.gid;
        st.st_blksize = 4096;
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
    /// (POSIX), including the closes of exit and exec.
    fn on_close(&self, pid: ProcessId) {
        let mut inner = self.0.inner.lock();
        if inner.notify.is_some_and(|n| n.pid == pid.as_u64()) {
            inner.notify = None;
        }
    }
}

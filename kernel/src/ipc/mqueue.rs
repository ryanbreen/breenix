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
//! waiter served is the one of highest scheduling rank, read when it is
//! served, that has waited longest, and the waiters of that side are woken to
//! check their own entries.
//!
//! A SIGEV_THREAD registration names a second queue, the notification queue
//! its process's C library reads, and a 32-byte cookie. The registration
//! sends that queue exactly one message: the cookie with its last byte set to
//! NOTIFY_WOKENUP when it notifies, or to NOTIFY_REMOVED when it is removed
//! without notifying (Linux's protocol, which Linux runs over a netlink
//! socket). The message is allocated and its place in the notification queue
//! reserved when the process registers (`Delivery`), so sending it needs no
//! memory and cannot find the queue full.
//!
//! Locks: `MQ_NAMES`, then a queue's `inner`, then `MQ_USAGE`, then a wait
//! queue's lock, then the scheduler. No queue lock is held while user memory
//! is copied, a thread is woken, a signal is sent or PROCESS_MANAGER is
//! taken, and no two queues' `inner` locks are held at once. The locks taken
//! under PROCESS_MANAGER are a queue's `inner`, by `on_close` and by a
//! SIGEV_THREAD delivery, `MQ_USAGE`, when a queue is freed, and a wait
//! queue's lock, for a deferred wake; none takes the scheduler.

mod calls;
pub mod dev;
pub mod store;

pub use self::calls::{
    sys_mq_getsetattr, sys_mq_notify, sys_mq_open, sys_mq_timedreceive, sys_mq_timedsend, sys_mq_unlink,
};

use self::store::{Channel, MqAttr, NameTable, Sent, Usage, WaitList};
use super::fd::{status_flags, FileDescriptor};
use super::object::IpcFile;
use super::wait::{wait_on, IpcDeadline, WaitEnd};
use crate::fs::permissions::Credentials;
use crate::irq_safe_mutex::IrqSafeMutex;
use crate::process::ProcessId;
use crate::syscall::errno::{EACCES, EAGAIN, EBUSY, EEXIST, EINVAL, ENOENT, ENOMEM, EPERM, ETIMEDOUT};
use crate::task::waitqueue::WaitQueueHead;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering};

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
/// a queue's file in its mqueue filesystem reads as (`MqFile::read`).
const QUEUE_FILE_SIZE: usize = 80;

/// The length of a SIGEV_THREAD cookie (Linux's NOTIFY_COOKIE_LEN).
pub const NOTIFY_COOKIE_LEN: usize = 32;
/// What the last byte of a delivered cookie says (Linux's values): the
/// registration notified, or it was removed without notifying.
const NOTIFY_WOKENUP: u8 = 1;
const NOTIFY_REMOVED: u8 = 2;

/// How a registered process is told a message arrived.
#[derive(Debug)]
pub enum NotifyKind {
    /// SIGEV_NONE: the registration is used up and no signal is sent.
    Nothing,
    /// SIGEV_SIGNAL: this signal is sent with SI_MESGQ and the value. Signal
    /// 0 is accepted and sends nothing, as on Linux.
    Signal(u32),
    /// SIGEV_THREAD: the cookie message for the notification queue.
    Thread(Delivery),
}

/// A SIGEV_THREAD registration's one message: its cookie, allocated when
/// the process registered, and the place in the notification queue reserved
/// for it then. Dropped unsent, it gives the place back. It takes the
/// notification queue's lock, so it is never sent or dropped while another
/// queue's lock is held.
pub struct Delivery {
    channel: Arc<MessageQueue>,
    cookie: Option<Vec<u8>>,
}

impl core::fmt::Debug for Delivery {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Delivery(ino={})", self.channel.ino)
    }
}

impl Delivery {
    /// The message `cookie` for `channel`: EINVAL for a queue whose messages
    /// cannot hold a cookie, ENOMEM with no memory for it or its place.
    fn new(channel: Arc<MessageQueue>, cookie: &[u8; NOTIFY_COOKIE_LEN]) -> Result<Self, u64> {
        if channel.msgsize < NOTIFY_COOKIE_LEN {
            return Err(EINVAL as u64);
        }
        let mut msg = Vec::new();
        msg.try_reserve_exact(NOTIFY_COOKIE_LEN).map_err(|_| ENOMEM as u64)?;
        msg.extend_from_slice(cookie);
        channel.inner.lock().chan.msgs.reserve()?;
        Ok(Self { channel, cookie: Some(msg) })
    }

    /// Queue the cookie, its last byte set to `status`, in its reserved
    /// place, and return the notification queue, whose receivers the caller
    /// wakes. A receiver already waiting is served by the queue's next
    /// operation (`Channel::serve`), so this reads no thread's rank and may
    /// run under PROCESS_MANAGER.
    fn send(mut self, status: u8) -> Arc<MessageQueue> {
        if let Some(mut msg) = self.cookie.take() {
            msg[NOTIFY_COOKIE_LEN - 1] = status;
            let mut inner = self.channel.inner.lock();
            inner.chan.msgs.push_reserved(0, msg);
            self.channel.events.fetch_add(1, Ordering::Release);
            self.channel.touched(&inner);
        }
        self.channel.clone()
    }
}

impl Drop for Delivery {
    fn drop(&mut self) {
        if self.cookie.take().is_some() {
            self.channel.inner.lock().chan.msgs.unreserve();
        }
    }
}

/// An mq_notify registration.
#[derive(Debug)]
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

/// Thread `tid`'s scheduling rank now (`SchedPolicy::rank`), which decides
/// the order waiters are served in; 0 for a thread the scheduler no longer
/// has. It can take the scheduler's lock, so it is never called under
/// PROCESS_MANAGER.
fn rank_of(tid: u64) -> u8 {
    crate::task::scheduler::sched_policy(tid).map_or(0, |sched| sched.rank())
}

fn realtime_ns() -> i64 {
    let (sec, nsec) = crate::time::get_real_time_ns();
    sec.saturating_mul(1_000_000_000).saturating_add(nsec)
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
    /// it. The flag is read and set in one atomic step on the description
    /// word that F_SETFL also changes, which leaves the word's other flags
    /// alone, so neither call can undo the other; the queue's lock makes the
    /// reported attributes and that step one snapshot.
    pub fn getsetattr(&self, fd: &FileDescriptor, nonblock: Option<bool>) -> MqAttr {
        let inner = self.inner.lock();
        let flags = match nonblock {
            Some(nonblock) => fd.update_status_flags(
                status_flags::O_NONBLOCK,
                if nonblock { status_flags::O_NONBLOCK } else { 0 },
            ),
            None => fd.status_flags(),
        };
        self.attr(&inner, flags & status_flags::O_NONBLOCK != 0)
    }

    /// Record the queue's depth and the time of a send or receive.
    fn touched(&self, inner: &MqInner) {
        self.depth.store(inner.chan.msgs.len(), Ordering::Release);
        self.changed_ns.store(realtime_ns(), Ordering::Relaxed);
    }

    /// Wake the waiters `served` (receivers, senders) says were completed.
    fn wake(&self, served: (bool, bool)) {
        if served.0 {
            self.receivers.wake_up();
        }
        if served.1 {
            self.senders.wake_up();
        }
    }

    /// Hand any messages a delivery left queued to the receivers waiting,
    /// under `inner` (`Channel::serve`): the waiters completed.
    fn serve(&self, inner: &mut MqInner) -> (bool, bool) {
        let served = inner.chan.serve(&rank_of);
        if served.0 || served.1 {
            self.events.fetch_add(1, Ordering::Release);
            self.touched(inner);
        }
        served
    }

    /// Queue `msg` at `prio`, or hand it to a waiting receiver, waiting while
    /// the queue is full unless `nonblock`, until `deadline`. `sender` is the
    /// calling process's thread group ID and real user ID, for a
    /// notification; `me` is the calling thread.
    pub fn send(
        &self,
        msg: Vec<u8>,
        prio: u32,
        nonblock: bool,
        deadline: &IpcDeadline,
        sender: (u64, u32),
        me: u64,
    ) -> Result<(), u64> {
        let mut inner = self.inner.lock();
        let msg = match inner.chan.send(prio, msg, &rank_of) {
            Sent::HandedOff => {
                self.events.fetch_add(1, Ordering::Release);
                self.touched(&inner);
                drop(inner);
                self.receivers.wake_up();
                return Ok(());
            }
            Sent::Queued { notify, served } => {
                if served {
                    self.events.fetch_add(1, Ordering::Release);
                }
                self.touched(&inner);
                drop(inner);
                self.wake((served, served));
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
        if inner.chan.senders.add(me, Some((msg, prio))).is_err() {
            return Err(ENOMEM as u64);
        }
        drop(inner);
        self.wait_done(&self.senders, deadline, me, Side::Senders).map(|_| ())
    }

    /// Take the oldest message of the highest priority, waiting while the
    /// queue is empty unless `nonblock`, until `deadline`. Receivers already
    /// waiting are served first.
    pub fn receive(&self, nonblock: bool, deadline: &IpcDeadline, me: u64) -> Result<(Vec<u8>, u32), u64> {
        let mut inner = self.inner.lock();
        let mut served = self.serve(&mut inner);
        if let Some((taken, sender)) = inner.chan.receive(&rank_of) {
            if sender {
                self.events.fetch_add(1, Ordering::Release);
                served.1 = true;
            }
            self.touched(&inner);
            drop(inner);
            self.wake(served);
            return Ok(taken);
        }
        let queued = !nonblock && inner.chan.receivers.add(me, None).is_ok();
        drop(inner);
        self.wake(served);
        if nonblock {
            return Err(EAGAIN as u64);
        }
        if !queued {
            return Err(ENOMEM as u64);
        }
        self.wait_done(&self.receivers, deadline, me, Side::Receivers)?
            .msg
            .ok_or(EINVAL as u64)
    }

    /// Wait on `q` until the entry of thread `tid` in the `side` list is
    /// completed, and remove it: the completed entry, or
    /// ETIMEDOUT at `deadline`, or the errno that ended the wait otherwise
    /// (`WaitEnd::Interrupted`: ERESTARTSYS or EINTR for a signal, as the
    /// wait decided, or ENOMEM). An entry completed by the time the wait ends is
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
            let (entry, served) = {
                let mut inner = self.inner.lock();
                // A delivery can leave a message queued while receivers
                // wait; a woken receiver serves it.
                let served = match side {
                    Side::Receivers => self.serve(&mut inner),
                    Side::Senders => (false, false),
                };
                let list = inner.waiters(side);
                let entry = if list.get(tid).is_some_and(|w| w.done) { list.remove(tid) } else { None };
                (entry, served)
            };
            self.wake(served);
            if let Some(entry) = entry {
                return Ok(entry);
            }
            let end = wait_on(q, deadline, || self.events.load(Ordering::Acquire) == seen);
            let failed = match end {
                WaitEnd::Woken => continue,
                WaitEnd::TimedOut => ETIMEDOUT as u64,
                WaitEnd::Interrupted(errno) => errno,
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
        let outcome = match request {
            None => Ok(inner.chan.notify.take_if(|n| n.pid == pid)),
            Some(n) if inner.chan.notify.is_some() => Err(n),
            Some(n) => {
                inner.chan.notify = Some(n);
                Ok(None)
            }
        };
        drop(inner);
        // A refused or removed registration's message is sent or dropped
        // only now, with this queue's lock released.
        match outcome {
            Ok(removed) => {
                if let Some(n) = removed {
                    removed_without_notifying(n, false);
                }
                Ok(())
            }
            Err(refused) => {
                drop(refused);
                Err(EBUSY as u64)
            }
        }
    }
}

/// Tell the process of registration `n`, removed without notifying, that it
/// is gone: a SIGEV_THREAD registration sends its cookie with
/// NOTIFY_REMOVED, so the C library can release what it kept for it.
/// `deferred` says the caller holds PROCESS_MANAGER, so the notification
/// queue's receivers are woken through the deferred path.
fn removed_without_notifying(n: MqNotify, deferred: bool) {
    if let NotifyKind::Thread(delivery) = n.kind {
        let channel = delivery.send(NOTIFY_REMOVED);
        if deferred {
            channel.receivers.wake_up_deferred();
        } else {
            channel.receivers.wake_up();
        }
    }
}

impl Drop for MessageQueue {
    fn drop(&mut self) {
        MQ_USAGE.lock().release(self.charge.0, self.charge.1);
    }
}

/// Tell a registered process a message arrived: `sender` is the sending
/// process's thread group ID and real user ID. A process that has exec'd
/// since it registered is not told: the program image is checked, and the
/// signal generated or the cookie queued, under PROCESS_MANAGER, which exec
/// holds while it changes the image.
fn notify_owner(n: MqNotify, sender: (u64, u32)) {
    match n.kind {
        NotifyKind::Nothing | NotifyKind::Signal(0) => {}
        NotifyKind::Signal(signo) => {
            let info = crate::signal::SigInfo::mesgq(sender.0 as u32, sender.1, n.value);
            let _ = crate::syscall::signal::send_kernel_signal(ProcessId::new(n.pid), signo, info, n.exec_id);
        }
        NotifyKind::Thread(delivery) => {
            let (pid, exec_id) = (ProcessId::new(n.pid), n.exec_id);
            let channel = crate::process::with_process_manager(move |manager| {
                let same_image = manager.get_process(pid).is_some_and(|p| p.exec_id == exec_id);
                same_image.then(|| delivery.send(NOTIFY_WOKENUP))
            })
            .flatten();
            if let Some(channel) = channel {
                channel.receivers.wake_up();
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
        if !cred.may(crate::fs::attrs::InodeAttrs::new(mode, uid, gid), wanted) {
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

/// An open file description's reference to a queue, and its read position
/// in the queue's status text.
pub struct MqFile(pub Arc<MessageQueue>, AtomicU64);

impl MqFile {
    pub fn new(queue: Arc<MessageQueue>) -> Self {
        Self(queue, AtomicU64::new(0))
    }
}

impl core::fmt::Debug for MqFile {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "MqFile(ino={})", self.0.ino)
    }
}

/// A queue's status text, as a read of its file in Linux's mqueue
/// filesystem gives it.
struct StatusText {
    bytes: [u8; QUEUE_FILE_SIZE],
    len: usize,
}

impl core::fmt::Write for StatusText {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let end = self.len.checked_add(s.len()).filter(|&end| end <= self.bytes.len()).ok_or(core::fmt::Error)?;
        self.bytes[self.len..end].copy_from_slice(s.as_bytes());
        self.len = end;
        Ok(())
    }
}

impl MessageQueue {
    /// The status text: the bytes of the messages queued, and the
    /// registration's notification method (sigev_notify), signal and
    /// process, each 0 with no registration (Linux's format).
    fn status_text(&self) -> StatusText {
        use crate::signal::timers::{SIGEV_NONE, SIGEV_SIGNAL, SIGEV_THREAD};
        let inner = self.inner.lock();
        let bytes = inner.chan.msgs.bytes();
        let (notify, signo, pid) = match &inner.chan.notify {
            None => (0, 0, 0),
            Some(n) => match n.kind {
                NotifyKind::Nothing => (SIGEV_NONE, 0, n.pid),
                NotifyKind::Signal(signo) => (SIGEV_SIGNAL, signo, n.pid),
                NotifyKind::Thread(_) => (SIGEV_THREAD, 0, n.pid),
            },
        };
        drop(inner);
        let mut text = StatusText { bytes: [0; QUEUE_FILE_SIZE], len: 0 };
        let _ = core::fmt::Write::write_fmt(
            &mut text,
            format_args!("QSIZE:{bytes:<10} NOTIFY:{notify:<5} SIGNO:{signo:<5} NOTIFY_PID:{pid:<6}\n"),
        );
        text
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
        st.st_size = QUEUE_FILE_SIZE as i64;
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

    /// Linux's MQUEUE_MAGIC, the filesystem its queues live in.
    fn statfs_magic(&self) -> u64 {
        0x19800202
    }

    /// read(2) gives the queue's status text from this description's
    /// position, which the read advances.
    fn read(&self, buf: u64, count: usize) -> Result<u64, u64> {
        let text = self.0.status_text();
        let at = (self.1.load(Ordering::Acquire) as usize).min(text.len);
        let n = count.min(text.len - at);
        crate::syscall::userptr::write_user_bytes(buf, text.bytes[at..].as_ptr(), n)?;
        self.1.fetch_add(n as u64, Ordering::AcqRel);
        Ok(n as u64)
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
    /// its threads closed. This runs under PROCESS_MANAGER, so a SIGEV_THREAD
    /// registration's NOTIFY_REMOVED wake is deferred.
    fn on_close(&self, pid: ProcessId) {
        let removed = self.0.inner.lock().chan.notify.take_if(|n| n.pid == pid.as_u64());
        if let Some(n) = removed {
            removed_without_notifying(n, true);
        }
    }
}

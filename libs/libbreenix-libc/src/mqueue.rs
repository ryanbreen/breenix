//! POSIX message queues (<mqueue.h>): mq_open, mq_close, mq_unlink,
//! mq_send, mq_receive, mq_timedsend, mq_timedreceive, mq_notify,
//! mq_getattr and mq_setattr, over the kernel's message queue calls.
//!
//! A queue name begins with a slash, which the library strips: the kernel
//! takes the rest of the name. A name that does not begin with one is
//! EINVAL. mq_open's `mode` and `attr` are variadic in C; both Linux ABIs
//! pass them in the registers a fixed third and fourth argument use, and
//! they are read only when O_CREAT is set.
//!
//! SIGEV_THREAD is built here, as POSIX leaves it to the library: one helper
//! thread per process reads the messages the kernel sends to the process's
//! notification queue (see the kernel's mq_notify) and starts, for each
//! notification, a new thread with the registration's attributes that calls
//! its function with its value. The helper is stopped while the process
//! execs (`before_exec`).

use crate::{__errno_location, syscall_result_to_c_int, syscall_result_to_c_ssize, EFAULT, EINVAL};
use core::cell::UnsafeCell;
use core::ffi::c_void;
use core::ptr::null_mut;
use core::sync::atomic::{AtomicU32, Ordering};
use libbreenix::raw;
use libbreenix::syscall::nr;

/// A message queue descriptor.
pub type MqdT = i32;

/// struct mq_attr, as the Linux ABI lays it out.
#[repr(C)]
pub struct MqAttr {
    pub mq_flags: i64,
    pub mq_maxmsg: i64,
    pub mq_msgsize: i64,
    pub mq_curmsgs: i64,
    pub __reserved: [i64; 4],
}

const O_CREAT: i32 = 0o100;

/// The kernel's name for queue `name`: the address past its leading slash,
/// or the errno for a name that is null or does not begin with one.
unsafe fn kernel_name(name: *const u8) -> Result<u64, i32> {
    if name.is_null() {
        return Err(EFAULT);
    }
    if *name != b'/' {
        return Err(EINVAL);
    }
    Ok(name.add(1) as u64)
}

unsafe fn fail(errno: i32) -> i32 {
    *__errno_location() = errno;
    -1
}

/// mq_open - open a message queue, creating it with O_CREAT
#[no_mangle]
pub unsafe extern "C" fn mq_open(name: *const u8, oflag: i32, mode: u32, attr: *const MqAttr) -> MqdT {
    let name = match kernel_name(name) {
        Ok(name) => name,
        Err(errno) => return fail(errno),
    };
    let (mode, attr) = if oflag & O_CREAT != 0 { (mode, attr) } else { (0, core::ptr::null()) };
    let r = raw::syscall4(nr::MQ_OPEN, name, oflag as u64, u64::from(mode), attr as u64) as i64;
    syscall_result_to_c_int(r)
}

/// mq_close - close a message queue descriptor. The close removes the
/// process's registration in the kernel, which tells the helper of a
/// SIGEV_THREAD one.
#[no_mangle]
pub extern "C" fn mq_close(mqdes: MqdT) -> i32 {
    crate::close(mqdes)
}

/// mq_unlink - remove a message queue's name
#[no_mangle]
pub unsafe extern "C" fn mq_unlink(name: *const u8) -> i32 {
    let name = match kernel_name(name) {
        Ok(name) => name,
        Err(errno) => return fail(errno),
    };
    syscall_result_to_c_int(raw::syscall1(nr::MQ_UNLINK, name) as i64)
}

/// mq_timedsend - send a message, waiting for room until `abs_timeout`
/// (CLOCK_REALTIME)
#[no_mangle]
pub unsafe extern "C" fn mq_timedsend(
    mqdes: MqdT,
    msg_ptr: *const u8,
    msg_len: usize,
    msg_prio: u32,
    abs_timeout: *const c_void,
) -> i32 {
    let r = raw::syscall5(
        nr::MQ_TIMEDSEND,
        mqdes as u64,
        msg_ptr as u64,
        msg_len as u64,
        u64::from(msg_prio),
        abs_timeout as u64,
    );
    syscall_result_to_c_int(r as i64)
}

/// mq_send - send a message, waiting for room
#[no_mangle]
pub unsafe extern "C" fn mq_send(mqdes: MqdT, msg_ptr: *const u8, msg_len: usize, msg_prio: u32) -> i32 {
    mq_timedsend(mqdes, msg_ptr, msg_len, msg_prio, core::ptr::null())
}

/// mq_timedreceive - receive the oldest message of the highest priority,
/// waiting for one until `abs_timeout` (CLOCK_REALTIME)
#[no_mangle]
pub unsafe extern "C" fn mq_timedreceive(
    mqdes: MqdT,
    msg_ptr: *mut u8,
    msg_len: usize,
    msg_prio: *mut u32,
    abs_timeout: *const c_void,
) -> isize {
    let r = raw::syscall5(
        nr::MQ_TIMEDRECEIVE,
        mqdes as u64,
        msg_ptr as u64,
        msg_len as u64,
        msg_prio as u64,
        abs_timeout as u64,
    );
    syscall_result_to_c_ssize(r as i64)
}

/// mq_receive - receive the oldest message of the highest priority,
/// waiting for one
#[no_mangle]
pub unsafe extern "C" fn mq_receive(mqdes: MqdT, msg_ptr: *mut u8, msg_len: usize, msg_prio: *mut u32) -> isize {
    mq_timedreceive(mqdes, msg_ptr, msg_len, msg_prio, core::ptr::null())
}

/// mq_notify - register for, or with a null `notification` cancel, a
/// notification when a message arrives on the empty queue. SIGEV_NONE and
/// SIGEV_SIGNAL are the kernel's; SIGEV_THREAD is this library's.
#[no_mangle]
pub unsafe extern "C" fn mq_notify(mqdes: MqdT, notification: *const c_void) -> i32 {
    let ev = notification as *const SigEvent;
    if !ev.is_null() && (*ev).notify == SIGEV_THREAD {
        return match register_thread_notify(mqdes, &*ev) {
            Ok(()) => 0,
            Err(errno) => fail(errno),
        };
    }
    syscall_result_to_c_int(raw::syscall2(nr::MQ_NOTIFY, mqdes as u64, notification as u64) as i64)
}

/// mq_getattr - the queue's attributes and the descriptor's O_NONBLOCK
#[no_mangle]
pub unsafe extern "C" fn mq_getattr(mqdes: MqdT, mqstat: *mut MqAttr) -> i32 {
    syscall_result_to_c_int(raw::syscall3(nr::MQ_GETSETATTR, mqdes as u64, 0, mqstat as u64) as i64)
}

/// mq_setattr - set the descriptor's O_NONBLOCK from `mqstat`'s mq_flags,
/// storing the previous attributes in `omqstat` when it is not null
#[no_mangle]
pub unsafe extern "C" fn mq_setattr(mqdes: MqdT, mqstat: *const MqAttr, omqstat: *mut MqAttr) -> i32 {
    syscall_result_to_c_int(raw::syscall3(nr::MQ_GETSETATTR, mqdes as u64, mqstat as u64, omqstat as u64) as i64)
}

// ---------------------------------------------------------------------------
// SIGEV_THREAD.
//
// Each registration has a record, allocated here, whose address is the first
// 8 bytes of its 32-byte cookie. The kernel sends the cookie to the process's
// notification queue exactly once: with its last byte NOTIFY_WOKENUP when the
// registration notifies, or NOTIFY_REMOVED when it is removed without
// notifying (mq_close, mq_notify(NULL), any close of the queue). The helper
// frees the record when that message arrives, so a notification already sent
// is never lost to a later registration, close or cancellation, and no record
// is freed twice.

const SIGEV_THREAD: i32 = 2;
const NOTIFY_COOKIE_LEN: usize = 32;
const NOTIFY_WOKENUP: u8 = 1;
const NOTIFY_REMOVED: u8 = 2;
/// The library's message telling its helper to stop, sent at a priority
/// above the kernel's cookies (0) so it is read first.
const NOTIFY_STOP: u8 = 0x80;
const STOP_PRIO: u64 = 1;
const O_RDWR: i32 = 0o2;
const O_EXCL: i32 = 0o200;
const SIG_BLOCK: u64 = 0;
const SIG_SETMASK: u64 = 2;
const EINTR: i64 = 4;
const ENOMEM: i32 = 12;
const EEXIST: i64 = 17;
const PTHREAD_CREATE_DETACHED: i32 = 1;
const FUTEX_WAIT: u64 = 0;
const FUTEX_WAKE: u64 = 1;

/// The notification function of a SIGEV_THREAD registration: it takes the
/// union sigval, which both ABIs pass as one 8-byte integer.
type NotifyFn = unsafe extern "C" fn(u64);

/// struct sigevent as the Linux ABI lays it out: sigev_value, sigev_signo,
/// sigev_notify, then for SIGEV_THREAD the function and the attributes.
#[repr(C)]
struct SigEvent {
    value: u64,
    signo: i32,
    notify: i32,
    function: Option<NotifyFn>,
    attributes: *const u8,
    pad: [u64; 4],
}

const _: () = assert!(core::mem::size_of::<SigEvent>() == 64);

/// A SIGEV_THREAD registration: what its notification thread is started
/// with, and its place in the process's list of records.
struct Record {
    function: NotifyFn,
    value: u64,
    attr: Option<[u8; crate::pthread::ATTR_SIZE]>,
    /// The registering thread's signal mask, which the notification thread
    /// starts with.
    mask: u64,
    prev: *mut Record,
    next: *mut Record,
}

/// The process's SIGEV_THREAD state. `pid` is the process the notification
/// queue and the helper belong to; a fork child starts again.
struct Notifier {
    pid: i32,
    /// The notification queue's descriptor, or -1.
    queue: i32,
    /// The helper's thread, or 0 when none runs.
    helper: usize,
    /// Set while the helper is being stopped for an exec: it then keeps the
    /// notifications it reads in `deferred` instead of starting them.
    stopping: bool,
    /// The records of live registrations.
    records: *mut Record,
    /// Notifications read while the helper was stopping, started once the
    /// exec has failed.
    deferred: *mut Record,
    /// Records a fork child inherited, freed at its next use of the state.
    orphans: *mut Record,
}

struct Shared(UnsafeCell<Notifier>);

// SAFETY: `with_notifier` is the one accessor, and it holds NOTIFY_LOCK.
unsafe impl Sync for Shared {}

/// A futex lock: 0 free, 1 held, 2 held with waiters.
static NOTIFY_LOCK: AtomicU32 = AtomicU32::new(0);
static NOTIFIER: Shared = Shared(UnsafeCell::new(Notifier {
    pid: 0,
    queue: -1,
    helper: 0,
    stopping: false,
    records: null_mut(),
    deferred: null_mut(),
    orphans: null_mut(),
}));

unsafe fn futex(op: u64, value: u32) {
    raw::syscall6(nr::FUTEX, NOTIFY_LOCK.as_ptr() as u64, op, u64::from(value), 0, 0, 0);
}

/// Run `f` on the notifier state under its lock, with every signal blocked
/// so a signal handler on this thread cannot wait for the lock it holds. A
/// waiter sleeps on the lock's futex rather than spinning.
unsafe fn with_notifier<R>(f: impl FnOnce(&mut Notifier) -> R) -> R {
    let all = !0u64;
    let mut saved = 0u64;
    raw::syscall4(nr::SIGPROCMASK, SIG_BLOCK, &all as *const u64 as u64, &mut saved as *mut u64 as u64, 8);
    if NOTIFY_LOCK.compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed).is_err() {
        while NOTIFY_LOCK.swap(2, Ordering::Acquire) != 0 {
            futex(FUTEX_WAIT, 2);
        }
    }
    let n = &mut *NOTIFIER.0.get();
    let orphans = core::mem::replace(&mut n.orphans, null_mut());
    free_list(orphans);
    let r = f(n);
    if NOTIFY_LOCK.swap(0, Ordering::Release) == 2 {
        futex(FUTEX_WAKE, 1);
    }
    raw::syscall4(nr::SIGPROCMASK, SIG_SETMASK, &saved as *const u64 as u64, 0, 8);
    r
}

/// Free a list of records linked through `next`.
unsafe fn free_list(mut record: *mut Record) {
    while !record.is_null() {
        let next = (*record).next;
        crate::free(record as *mut u8);
        record = next;
    }
}

/// In a fork child, called before anything else runs: no other thread
/// survived to hold the lock, the helper did not survive, and no
/// registration was inherited. The inherited notification queue descriptor
/// still has its number, as nothing has run to close or reuse it, and is
/// closed now so the parent's cookies stay the parent's. The inherited
/// records are freed at the child's next use of this state.
pub(crate) fn after_fork() {
    NOTIFY_LOCK.store(0, Ordering::Release);
    // SAFETY: only this thread exists in the child, and the lock is free.
    unsafe {
        let n = &mut *NOTIFIER.0.get();
        if n.queue >= 0 {
            raw::syscall1(nr::CLOSE, n.queue as u64);
        }
        n.queue = -1;
        n.helper = 0;
        n.stopping = false;
        n.pid = 0;
        for list in [&mut n.records, &mut n.deferred] {
            let mut record = core::mem::replace(list, null_mut());
            while !record.is_null() {
                let next = (*record).next;
                (*record).next = n.orphans;
                n.orphans = record;
                record = next;
            }
        }
    }
}

fn getpid() -> i32 {
    // SAFETY: getpid takes no arguments and touches no memory.
    unsafe { raw::syscall0(nr::GETPID) as i32 }
}

fn signal_mask() -> u64 {
    let mut mask = 0u64;
    unsafe { raw::syscall4(nr::SIGPROCMASK, SIG_BLOCK, 0, &mut mask as *mut u64 as u64, 8) };
    mask
}

/// Write `value` in decimal at `buf[at..]`, returning the index past it.
fn put_decimal(buf: &mut [u8], mut at: usize, mut value: u64) -> usize {
    let mut digits = [0u8; 20];
    let mut len = 0;
    loop {
        digits[len] = b'0' + (value % 10) as u8;
        len += 1;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    while len > 0 {
        len -= 1;
        buf[at] = digits[len];
        at += 1;
    }
    at
}

/// Create the process's notification queue: a queue of one 32-byte message,
/// whose name is removed at once, so only its descriptor (close-on-exec, as
/// every queue descriptor is) refers to it.
unsafe fn open_queue(pid: i32) -> Result<i32, i32> {
    let attr = MqAttr { mq_flags: 0, mq_maxmsg: 1, mq_msgsize: NOTIFY_COOKIE_LEN as i64, mq_curmsgs: 0, __reserved: [0; 4] };
    let mut attempt = 0u64;
    loop {
        // ".mq-notify-<pid>-<attempt>", the kernel's form of the name.
        let mut name = [0u8; 64];
        let prefix = b".mq-notify-";
        name[..prefix.len()].copy_from_slice(prefix);
        let mut at = put_decimal(&mut name, prefix.len(), pid as u32 as u64);
        name[at] = b'-';
        at = put_decimal(&mut name, at + 1, attempt);
        name[at] = 0;
        let r = raw::syscall4(
            nr::MQ_OPEN,
            name.as_ptr() as u64,
            (O_RDWR | O_CREAT | O_EXCL) as u64,
            0o600,
            &attr as *const MqAttr as u64,
        ) as i64;
        if r == -EEXIST {
            // A name left by an earlier process that had this PID.
            attempt += 1;
            continue;
        }
        if r < 0 {
            return Err(-r as i32);
        }
        raw::syscall1(nr::MQ_UNLINK, name.as_ptr() as u64);
        return Ok(r as i32);
    }
}

impl Notifier {
    /// Make sure this process has its notification queue and a running
    /// helper.
    unsafe fn ready(&mut self) -> Result<(), i32> {
        let me = getpid();
        if self.pid != me {
            // Not set up in this process. A child forked without the C
            // library's fork has a copy of the parent's state: the queue
            // descriptor's number may be in use for something else by now,
            // so it is not closed, and the records are the parent's.
            self.helper = 0;
            self.stopping = false;
            self.records = null_mut();
            self.deferred = null_mut();
            self.queue = open_queue(me)?;
            self.pid = me;
        }
        if self.helper == 0 {
            self.start_helper()?;
        }
        Ok(())
    }

    unsafe fn start_helper(&mut self) -> Result<(), i32> {
        let mut helper = 0usize;
        let r = crate::pthread::pthread_create(&mut helper, core::ptr::null(), notify_helper, self.queue as usize as *mut u8);
        if r != 0 {
            return Err(r);
        }
        self.helper = helper;
        Ok(())
    }

    unsafe fn link(&mut self, record: *mut Record) {
        (*record).prev = null_mut();
        (*record).next = self.records;
        if !self.records.is_null() {
            (*self.records).prev = record;
        }
        self.records = record;
    }

    unsafe fn unlink(&mut self, record: *mut Record) {
        let (prev, next) = ((*record).prev, (*record).next);
        if prev.is_null() {
            self.records = next;
        } else {
            (*prev).next = next;
        }
        if !next.is_null() {
            (*next).prev = prev;
        }
        (*record).prev = null_mut();
        (*record).next = null_mut();
    }
}

/// Register `ev`, a SIGEV_THREAD sigevent, on `mqdes`.
unsafe fn register_thread_notify(mqdes: MqdT, ev: &SigEvent) -> Result<(), i32> {
    let Some(function) = ev.function else {
        return Err(EINVAL);
    };
    let attr = if ev.attributes.is_null() {
        None
    } else {
        let mut copy = [0u8; crate::pthread::ATTR_SIZE];
        core::ptr::copy_nonoverlapping(ev.attributes, copy.as_mut_ptr(), copy.len());
        Some(copy)
    };
    let record = crate::malloc(core::mem::size_of::<Record>()) as *mut Record;
    if record.is_null() {
        return Err(ENOMEM);
    }
    record.write(Record { function, value: ev.value, attr, mask: signal_mask(), prev: null_mut(), next: null_mut() });
    let queue = with_notifier(|n| {
        n.ready()?;
        n.link(record);
        Ok(n.queue)
    });
    let queue = match queue {
        Ok(queue) => queue,
        Err(errno) => {
            crate::free(record as *mut u8);
            return Err(errno);
        }
    };
    let mut cookie = [0u8; NOTIFY_COOKIE_LEN];
    cookie[..8].copy_from_slice(&(record as usize as u64).to_ne_bytes());
    let kernel_ev = SigEvent {
        value: cookie.as_ptr() as u64,
        signo: queue,
        notify: SIGEV_THREAD,
        function: None,
        attributes: core::ptr::null(),
        pad: [0; 4],
    };
    let r = raw::syscall2(nr::MQ_NOTIFY, mqdes as u64, &kernel_ev as *const SigEvent as u64) as i64;
    if r < 0 {
        // No registration was made, so no message will name the record.
        with_notifier(|n| n.unlink(record));
        crate::free(record as *mut u8);
        return Err(-r as i32);
    }
    Ok(())
}

/// What `before_exec` stopped, for `after_failed_exec`.
pub(crate) struct Stopped(bool);

/// Stop this process's helper before an exec: a thread that kept running
/// would make the exec fail while it shares the address space. The helper is
/// told to stop through its queue and joined; notifications it reads while
/// stopping are kept for `after_failed_exec`. A successful exec discards
/// them with the image, as POSIX discards the process's registrations.
pub(crate) fn before_exec() -> Stopped {
    // SAFETY: the state is reached through `with_notifier` only.
    unsafe {
        let running = with_notifier(|n| {
            if n.pid != getpid() || n.helper == 0 {
                return None;
            }
            n.stopping = true;
            Some((n.helper, n.queue))
        });
        let Some((helper, queue)) = running else {
            return Stopped(false);
        };
        let mut stop = [0u8; NOTIFY_COOKIE_LEN];
        stop[NOTIFY_COOKIE_LEN - 1] = NOTIFY_STOP;
        let sent = loop {
            let r = raw::syscall5(nr::MQ_TIMEDSEND, queue as u64, stop.as_ptr() as u64, stop.len() as u64, STOP_PRIO, 0) as i64;
            if r != -EINTR {
                break r == 0;
            }
        };
        if !sent {
            // The helper runs on; the exec reports the thread it finds.
            with_notifier(|n| n.stopping = false);
            return Stopped(false);
        }
        crate::pthread::pthread_join(helper, null_mut());
        with_notifier(|n| n.helper = 0);
        Stopped(true)
    }
}

/// After an exec that failed, start the helper again and the notifications
/// it kept while stopping. If no helper can be started, the next
/// SIGEV_THREAD registration tries again, and the queue keeps what arrives
/// meanwhile.
pub(crate) fn after_failed_exec(stopped: Stopped) {
    if !stopped.0 {
        return;
    }
    // SAFETY: the state is reached through `with_notifier` only.
    unsafe {
        let deferred = with_notifier(|n| {
            n.stopping = false;
            let _ = n.start_helper();
            core::mem::replace(&mut n.deferred, null_mut())
        });
        let mut record = deferred;
        while !record.is_null() {
            let next = (*record).next;
            start_notification(record);
            record = next;
        }
    }
}

/// The helper thread: with its signals blocked, read the messages of the
/// notification queue `arg` and act on each, until the library's stop
/// message or an error ends it.
extern "C" fn notify_helper(arg: *mut u8) -> *mut u8 {
    let queue = arg as usize as u64;
    unsafe {
        let all = !0u64;
        raw::syscall4(nr::SIGPROCMASK, SIG_BLOCK, &all as *const u64 as u64, 0, 8);
        loop {
            let mut msg = [0u8; NOTIFY_COOKIE_LEN];
            let r = raw::syscall5(nr::MQ_TIMEDRECEIVE, queue, msg.as_mut_ptr() as u64, msg.len() as u64, 0, 0) as i64;
            if r == -EINTR {
                continue;
            }
            if r < 0 {
                return null_mut();
            }
            let record = u64::from_ne_bytes(msg[..8].try_into().unwrap()) as usize as *mut Record;
            match msg[NOTIFY_COOKIE_LEN - 1] {
                NOTIFY_STOP => return null_mut(),
                NOTIFY_WOKENUP => {
                    let start = with_notifier(|n| {
                        n.unlink(record);
                        if n.stopping {
                            (*record).next = n.deferred;
                            n.deferred = record;
                            false
                        } else {
                            true
                        }
                    });
                    if start {
                        start_notification(record);
                    }
                }
                NOTIFY_REMOVED => {
                    with_notifier(|n| n.unlink(record));
                    crate::free(record as *mut u8);
                }
                _ => {}
            }
        }
    }
}

/// Start the thread for one notification, which frees `record`. A thread
/// that cannot be created loses the notification, as one whose signal
/// cannot be queued is lost.
unsafe fn start_notification(record: *mut Record) {
    let mut attr = [0u64; 8];
    let attr_ptr = attr.as_mut_ptr() as *mut u8;
    match (*record).attr {
        Some(copy) => core::ptr::copy_nonoverlapping(copy.as_ptr(), attr_ptr, copy.len()),
        None => {
            crate::pthread::pthread_attr_init(attr_ptr);
        }
    }
    // No caller has the thread's handle to join it, so it is detached
    // whatever the attributes say.
    crate::pthread::pthread_attr_setdetachstate(attr_ptr, PTHREAD_CREATE_DETACHED);
    let mut thread = 0usize;
    if crate::pthread::pthread_create(&mut thread, attr_ptr, notification_thread, record as *mut u8) != 0 {
        crate::free(record as *mut u8);
    }
}

/// A notification thread: the registering thread's signal mask, then the
/// function with the registration's value.
extern "C" fn notification_thread(arg: *mut u8) -> *mut u8 {
    unsafe {
        let record = arg as *mut Record;
        let (function, value, mask) = ((*record).function, (*record).value, (*record).mask);
        crate::free(arg);
        raw::syscall4(nr::SIGPROCMASK, SIG_SETMASK, &mask as *const u64 as u64, 0, 8);
        function(value);
    }
    null_mut()
}

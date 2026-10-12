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
//! thread per process reads the notifications the kernel writes to a pipe
//! (see the kernel's mq_notify) and starts, for each, a new thread with the
//! registration's attributes that calls its function with its value.

use crate::{__errno_location, syscall_result_to_c_int, syscall_result_to_c_ssize, EFAULT, EINVAL};
use core::cell::UnsafeCell;
use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, Ordering};
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

/// mq_close - close a message queue descriptor
#[no_mangle]
pub extern "C" fn mq_close(mqdes: MqdT) -> i32 {
    // The close removes the process's registration in the kernel; a
    // SIGEV_THREAD one goes from the library's table too.
    unsafe { forget_thread_notify(mqdes) };
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
    let r = syscall_result_to_c_int(raw::syscall2(nr::MQ_NOTIFY, mqdes as u64, notification as u64) as i64);
    if r == 0 {
        // Any SIGEV_THREAD registration this descriptor had is gone.
        forget_thread_notify(mqdes);
    }
    r
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

const SIGEV_THREAD: i32 = 2;
const O_CLOEXEC: u64 = 0o2000000;
const SIG_BLOCK: u64 = 0;
const SIG_SETMASK: u64 = 2;
const EINTR: i64 = 4;
const EBADF: i32 = 9;
const ENOMEM: i32 = 12;
const PTHREAD_CREATE_DETACHED: i32 = 1;

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

/// A SIGEV_THREAD registration, kept for the descriptor it is on. `gen`
/// tells it from earlier ones on the same descriptor, so a notification of
/// one that has since been replaced or removed starts no thread.
#[derive(Clone, Copy)]
struct Slot {
    gen: u32,
    live: bool,
    function: Option<NotifyFn>,
    value: u64,
    attr: Option<[u8; crate::pthread::ATTR_SIZE]>,
    /// The registering thread's signal mask, which the notification thread
    /// starts with.
    mask: u64,
}

const EMPTY: Slot = Slot { gen: 0, live: false, function: None, value: 0, attr: None, mask: 0 };

/// The process's SIGEV_THREAD state: the helper's pipe and the
/// registrations, indexed by descriptor. `pid` is the process it was set
/// up in; a fork child, which has no helper, starts again.
struct Notifier {
    pid: i32,
    read_fd: i32,
    write_fd: i32,
    slots: *mut Slot,
    len: usize,
    next_gen: u32,
}

struct Shared(UnsafeCell<Notifier>);

// SAFETY: `with_notifier` is the one accessor, and it holds NOTIFY_LOCK.
unsafe impl Sync for Shared {}

static NOTIFY_LOCK: AtomicBool = AtomicBool::new(false);
static NOTIFIER: Shared = Shared(UnsafeCell::new(Notifier {
    pid: 0,
    read_fd: -1,
    write_fd: -1,
    slots: core::ptr::null_mut(),
    len: 0,
    next_gen: 1,
}));

/// Run `f` on the notifier state under its lock.
unsafe fn with_notifier<R>(f: impl FnOnce(&mut Notifier) -> R) -> R {
    while NOTIFY_LOCK
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        raw::syscall0(nr::YIELD);
    }
    let r = f(&mut *NOTIFIER.0.get());
    NOTIFY_LOCK.store(false, Ordering::Release);
    r
}

/// In a fork child, called before anything else runs: no other thread
/// survived to hold the lock.
pub(crate) fn after_fork() {
    NOTIFY_LOCK.store(false, Ordering::Release);
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

impl Notifier {
    /// The slot for descriptor `fd`, growing the table to hold it.
    unsafe fn slot(&mut self, fd: i32) -> Result<&mut Slot, i32> {
        if fd < 0 {
            return Err(EBADF);
        }
        let fd = fd as usize;
        if fd >= self.len {
            let len = (fd + 1).next_power_of_two().max(16);
            let slots = crate::malloc(len * core::mem::size_of::<Slot>()) as *mut Slot;
            if slots.is_null() {
                return Err(ENOMEM);
            }
            for i in 0..len {
                slots.add(i).write(if i < self.len { *self.slots.add(i) } else { EMPTY });
            }
            if !self.slots.is_null() {
                crate::free(self.slots as *mut u8);
            }
            self.slots = slots;
            self.len = len;
        }
        Ok(&mut *self.slots.add(fd))
    }

    /// Make sure this process has its helper thread and pipe.
    unsafe fn ready(&mut self) -> Result<(), i32> {
        let me = getpid();
        if self.pid == me && self.write_fd >= 0 {
            return Ok(());
        }
        if self.pid != me {
            // A fork child: the descriptors are copies and no helper reads
            // them, and no registration was inherited.
            for fd in [self.read_fd, self.write_fd] {
                if fd >= 0 {
                    raw::syscall1(nr::CLOSE, fd as u64);
                }
            }
            for i in 0..self.len {
                (*self.slots.add(i)).live = false;
            }
            self.read_fd = -1;
            self.write_fd = -1;
            self.pid = me;
        }
        let mut fds = [-1i32; 2];
        let r = raw::syscall2(nr::PIPE2, fds.as_mut_ptr() as u64, O_CLOEXEC) as i64;
        if r < 0 {
            return Err(-r as i32);
        }
        let mut attr = [0u64; 8];
        let attr = attr.as_mut_ptr() as *mut u8;
        crate::pthread::pthread_attr_init(attr);
        crate::pthread::pthread_attr_setdetachstate(attr, PTHREAD_CREATE_DETACHED);
        let mut helper = 0usize;
        let r = crate::pthread::pthread_create(&mut helper, attr, notify_helper, fds[0] as usize as *mut u8);
        if r != 0 {
            raw::syscall1(nr::CLOSE, fds[0] as u64);
            raw::syscall1(nr::CLOSE, fds[1] as u64);
            return Err(r);
        }
        self.read_fd = fds[0];
        self.write_fd = fds[1];
        Ok(())
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
    let mask = signal_mask();
    with_notifier(|n| {
        n.ready()?;
        let write_fd = n.write_fd;
        let gen = n.next_gen;
        let slot = n.slot(mqdes)?;
        // The kernel writes the cookie to the pipe; the slot is filled
        // before the lock is released, so the helper finds it.
        let cookie = (u64::from(gen) << 32) | mqdes as u32 as u64;
        let kernel_ev = SigEvent {
            value: cookie,
            signo: write_fd,
            notify: SIGEV_THREAD,
            function: None,
            attributes: core::ptr::null(),
            pad: [0; 4],
        };
        let r = raw::syscall2(nr::MQ_NOTIFY, mqdes as u64, &kernel_ev as *const SigEvent as u64) as i64;
        if r < 0 {
            return Err(-r as i32);
        }
        *slot = Slot { gen, live: true, function: Some(function), value: ev.value, attr, mask };
        n.next_gen = gen.wrapping_add(1).max(1);
        Ok(())
    })
}

/// Drop any SIGEV_THREAD registration the library holds for `mqdes`.
unsafe fn forget_thread_notify(mqdes: MqdT) {
    with_notifier(|n| {
        if mqdes >= 0 && (mqdes as usize) < n.len {
            (*n.slots.add(mqdes as usize)).live = false;
        }
    });
}

/// What a notification thread is started with.
struct Notice {
    function: NotifyFn,
    value: u64,
    mask: u64,
}

/// The helper thread: with its signals blocked, read 8-byte cookies from
/// the pipe and start a thread for each live registration they name.
extern "C" fn notify_helper(arg: *mut u8) -> *mut u8 {
    let fd = arg as usize as u64;
    unsafe {
        let all = !0u64;
        raw::syscall4(nr::SIGPROCMASK, SIG_BLOCK, &all as *const u64 as u64, 0, 8);
        loop {
            let mut cookie = [0u8; 8];
            let mut got = 0;
            while got < cookie.len() {
                let r = raw::syscall3(nr::READ, fd, cookie.as_mut_ptr().add(got) as u64, (cookie.len() - got) as u64) as i64;
                if r == -EINTR {
                    continue;
                }
                if r <= 0 {
                    return core::ptr::null_mut();
                }
                got += r as usize;
            }
            let cookie = u64::from_ne_bytes(cookie);
            let (fd_of, gen) = (cookie as u32 as i32, (cookie >> 32) as u32);
            let taken = with_notifier(|n| {
                let slot = n.slot(fd_of).ok()?;
                if !slot.live || slot.gen != gen {
                    return None;
                }
                slot.live = false;
                Some(*slot)
            });
            if let Some(slot) = taken {
                start_notification(&slot);
            }
        }
    }
}

/// Start the thread for one notification. A thread that cannot be created
/// loses the notification, as one whose signal cannot be queued is lost.
unsafe fn start_notification(slot: &Slot) {
    let Some(function) = slot.function else { return };
    let notice = crate::malloc(core::mem::size_of::<Notice>()) as *mut Notice;
    if notice.is_null() {
        return;
    }
    notice.write(Notice { function, value: slot.value, mask: slot.mask });
    let mut attr = [0u64; 8];
    let attr_ptr = attr.as_mut_ptr() as *mut u8;
    match slot.attr {
        Some(copy) => core::ptr::copy_nonoverlapping(copy.as_ptr(), attr_ptr, copy.len()),
        None => {
            crate::pthread::pthread_attr_init(attr_ptr);
        }
    }
    // No caller has the thread's handle to join it, so it is detached
    // whatever the attributes say.
    crate::pthread::pthread_attr_setdetachstate(attr_ptr, PTHREAD_CREATE_DETACHED);
    let mut thread = 0usize;
    if crate::pthread::pthread_create(&mut thread, attr_ptr, notification_thread, notice as *mut u8) != 0 {
        crate::free(notice as *mut u8);
    }
}

/// A notification thread: the registering thread's signal mask, then the
/// function with the registration's value.
extern "C" fn notification_thread(arg: *mut u8) -> *mut u8 {
    unsafe {
        let notice = (arg as *const Notice).read();
        crate::free(arg);
        raw::syscall4(nr::SIGPROCMASK, SIG_SETMASK, &notice.mask as *const u64 as u64, 0, 8);
        (notice.function)(notice.value);
    }
    core::ptr::null_mut()
}

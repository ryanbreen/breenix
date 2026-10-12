//! POSIX message queues (<mqueue.h>): mq_open, mq_close, mq_unlink,
//! mq_send, mq_receive, mq_timedsend, mq_timedreceive, mq_notify,
//! mq_getattr and mq_setattr, over the kernel's message queue calls.
//!
//! A queue name begins with a slash, which the library strips: the kernel
//! takes the rest of the name. A name that does not begin with one is
//! EINVAL. mq_open's `mode` and `attr` are variadic in C; both Linux ABIs
//! pass them in the registers a fixed third and fourth argument use, and
//! they are read only when O_CREAT is set.

use crate::{__errno_location, syscall_result_to_c_int, syscall_result_to_c_ssize, EFAULT, EINVAL};
use core::ffi::c_void;
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
/// notification when a message arrives on the empty queue
#[no_mangle]
pub unsafe extern "C" fn mq_notify(mqdes: MqdT, notification: *const c_void) -> i32 {
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

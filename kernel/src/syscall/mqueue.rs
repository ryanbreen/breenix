//! The POSIX message queue system calls, routed by Linux's numbers with
//! Linux's argument types: an int descriptor, int flags, mode_t mode,
//! size_t lengths, unsigned priority. The calls are in
//! `crate::ipc::mqueue`.

use super::SyscallResult;
use super::{
    MQ_GETSETATTR_SYSCALL_NUMBER, MQ_NOTIFY_SYSCALL_NUMBER, MQ_OPEN_SYSCALL_NUMBER, MQ_TIMEDRECEIVE_SYSCALL_NUMBER,
    MQ_TIMEDSEND_SYSCALL_NUMBER, MQ_UNLINK_SYSCALL_NUMBER,
};
use crate::ipc::mqueue;

/// The message queue call numbered `n`, or None for any other number.
pub fn dispatch(n: u64, a: [u64; 6]) -> Option<SyscallResult> {
    let int = |i: usize| a[i] as i32;
    let word = |i: usize| a[i] as u32;
    Some(match n {
        n if n == MQ_OPEN_SYSCALL_NUMBER => mqueue::sys_mq_open(a[0], word(1), word(2), a[3]),
        n if n == MQ_UNLINK_SYSCALL_NUMBER => mqueue::sys_mq_unlink(a[0]),
        n if n == MQ_TIMEDSEND_SYSCALL_NUMBER => mqueue::sys_mq_timedsend(int(0), a[1], a[2], word(3), a[4]),
        n if n == MQ_TIMEDRECEIVE_SYSCALL_NUMBER => mqueue::sys_mq_timedreceive(int(0), a[1], a[2], a[3], a[4]),
        n if n == MQ_NOTIFY_SYSCALL_NUMBER => mqueue::sys_mq_notify(int(0), a[1]),
        n if n == MQ_GETSETATTR_SYSCALL_NUMBER => mqueue::sys_mq_getsetattr(int(0), a[1], a[2]),
        _ => return None,
    })
}

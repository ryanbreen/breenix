//! System V IPC: ftok and the msg and sem calls.
//!
//! Written from POSIX.1-2024 (sys/ipc.h, sys/msg.h, sys/sem.h)
//! and Linux's system call ABI: each call but ftok is one system call, and
//! the kernel's structures are Linux's 64-bit ones.

use crate::{syscall_result_to_c_int, syscall_result_to_c_ssize};
use libbreenix::raw;
use libbreenix::syscall::nr;

/// ftok: a key from the file `path` names and the low eight bits of `id`.
/// The same file and id always give the same key: the id in bits 24-31, the
/// low eight bits of the file's device in bits 16-23 and the low sixteen
/// bits of its inode number below them. A path stat cannot resolve returns
/// -1 with stat's errno.
#[no_mangle]
pub unsafe extern "C" fn ftok(path: *const u8, id: i32) -> i32 {
    // struct stat begins with st_dev and st_ino, 64 bits each, on both
    // architectures.
    let mut st = [0u64; 32];
    if crate::stat(path, st.as_mut_ptr() as *mut u8) != 0 {
        return -1;
    }
    let (dev, ino) = (st[0] as u32, st[1] as u32);
    (((id as u32 & 0xff) << 24) | ((dev & 0xff) << 16) | (ino & 0xffff)) as i32
}

/// msgget: the identifier of the message queue `key` names.
#[no_mangle]
pub extern "C" fn msgget(key: i32, msgflg: i32) -> i32 {
    syscall_result_to_c_int(unsafe { raw::syscall2(nr::MSGGET, key as u64, msgflg as u64) } as i64)
}

/// msgsnd: send the message at `msgp`, a long type then `msgsz` bytes.
#[no_mangle]
pub unsafe extern "C" fn msgsnd(msqid: i32, msgp: *const u8, msgsz: usize, msgflg: i32) -> i32 {
    syscall_result_to_c_int(raw::syscall4(nr::MSGSND, msqid as u64, msgp as u64, msgsz as u64, msgflg as u64) as i64)
}

/// msgrcv: receive a message into `msgp`: the number of bytes of text.
#[no_mangle]
pub unsafe extern "C" fn msgrcv(msqid: i32, msgp: *mut u8, msgsz: usize, msgtyp: i64, msgflg: i32) -> isize {
    syscall_result_to_c_ssize(
        raw::syscall5(nr::MSGRCV, msqid as u64, msgp as u64, msgsz as u64, msgtyp as u64, msgflg as u64) as i64,
    )
}

/// msgctl: IPC_STAT, IPC_SET or IPC_RMID on a message queue.
#[no_mangle]
pub unsafe extern "C" fn msgctl(msqid: i32, cmd: i32, buf: *mut u8) -> i32 {
    syscall_result_to_c_int(raw::syscall3(nr::MSGCTL, msqid as u64, cmd as u64, buf as u64) as i64)
}

/// semget: the identifier of the semaphore set `key` names.
#[no_mangle]
pub extern "C" fn semget(key: i32, nsems: i32, semflg: i32) -> i32 {
    syscall_result_to_c_int(unsafe { raw::syscall3(nr::SEMGET, key as u64, nsems as u64, semflg as u64) } as i64)
}

/// semop: apply `nsops` operations to a semaphore set at once.
#[no_mangle]
pub unsafe extern "C" fn semop(semid: i32, sops: *mut u8, nsops: usize) -> i32 {
    syscall_result_to_c_int(raw::syscall3(nr::SEMOP, semid as u64, sops as u64, nsops as u64) as i64)
}

/// semtimedop: semop that gives up with EAGAIN once the relative `timeout`,
/// if not null, has passed.
#[no_mangle]
pub unsafe extern "C" fn semtimedop(semid: i32, sops: *mut u8, nsops: usize, timeout: *const u8) -> i32 {
    syscall_result_to_c_int(raw::syscall4(nr::SEMTIMEDOP, semid as u64, sops as u64, nsops as u64, timeout as u64) as i64)
}

/// semctl: a control command on a semaphore set or one semaphore. The
/// fourth argument, when the command takes one, is union semun, which the
/// caller passes as one register-sized argument: the value for SETVAL,
/// otherwise a pointer.
#[no_mangle]
pub extern "C" fn semctl(semid: i32, semnum: i32, cmd: i32, arg: u64) -> i32 {
    syscall_result_to_c_int(unsafe { raw::syscall4(nr::SEMCTL, semid as u64, semnum as u64, cmd as u64, arg) } as i64)
}

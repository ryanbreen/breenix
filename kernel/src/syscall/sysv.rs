//! System V IPC system calls, with Linux's argument types: int identifiers,
//! keys, flags and commands; size_t sizes; unsigned nsops.

use super::{
    MSGCTL_SYSCALL_NUMBER, MSGGET_SYSCALL_NUMBER, MSGRCV_SYSCALL_NUMBER, MSGSND_SYSCALL_NUMBER, SEMCTL_SYSCALL_NUMBER,
    SEMGET_SYSCALL_NUMBER, SEMOP_SYSCALL_NUMBER, SEMTIMEDOP_SYSCALL_NUMBER, SHMAT_SYSCALL_NUMBER, SHMCTL_SYSCALL_NUMBER,
    SHMDT_SYSCALL_NUMBER, SHMGET_SYSCALL_NUMBER,
};
use super::SyscallResult;
use crate::ipc::sysv::{msg, sem, shm};

/// The System V IPC call numbered `n`, or None.
pub fn dispatch(n: u64, a: [u64; 6]) -> Option<SyscallResult> {
    let int = |i: usize| a[i] as i32;
    let flags = |i: usize| a[i] as u32;
    Some(match n {
        n if n == MSGGET_SYSCALL_NUMBER => msg::sys_msgget(int(0), flags(1)),
        n if n == MSGSND_SYSCALL_NUMBER => msg::sys_msgsnd(int(0), a[1], a[2], flags(3)),
        n if n == MSGRCV_SYSCALL_NUMBER => msg::sys_msgrcv(int(0), a[1], a[2], a[3] as i64, flags(4)),
        n if n == MSGCTL_SYSCALL_NUMBER => msg::sys_msgctl(int(0), flags(1), a[2]),
        n if n == SEMGET_SYSCALL_NUMBER => sem::sys_semget(int(0), int(1), flags(2)),
        n if n == SEMOP_SYSCALL_NUMBER => sem::sys_semtimedop(int(0), a[1], u64::from(flags(2)), 0),
        n if n == SEMTIMEDOP_SYSCALL_NUMBER => sem::sys_semtimedop(int(0), a[1], u64::from(flags(2)), a[3]),
        n if n == SEMCTL_SYSCALL_NUMBER => sem::sys_semctl(int(0), int(1), flags(2), a[3]),
        n if n == SHMGET_SYSCALL_NUMBER => shm::sys_shmget(int(0), a[1], flags(2)),
        n if n == SHMAT_SYSCALL_NUMBER => shm::sys_shmat(int(0), a[1], flags(2)),
        n if n == SHMDT_SYSCALL_NUMBER => shm::sys_shmdt(a[0]),
        n if n == SHMCTL_SYSCALL_NUMBER => shm::sys_shmctl(int(0), flags(1), a[2]),
        _ => return None,
    })
}

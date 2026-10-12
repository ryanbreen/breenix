//! Host execution of production IPC logic with scheduler/user-copy shims.
#![cfg(test)]
#![feature(allocator_api)]
extern crate alloc;
pub use syscall::SyscallResult;
include!(concat!(env!("OUT_DIR"), "/ipc_numbers.rs"));
#[path = "../../ext2-host-tests/src/block.rs"]
pub mod block;
#[path = "../../ext2-host-tests/src/ext2.rs"]
pub mod ext2;
#[path = "../../../kernel/src/fs/permissions.rs"]
pub mod permissions;
pub mod fs {
    pub use crate::{ext2, permissions};
}
#[path = "../../../kernel/src/syscall/ipc.rs"]
pub mod dispatch;
#[path = "../../../kernel/src/ipc/object.rs"]
pub mod object;
#[path = "../../../kernel/src/ipc/wait.rs"]
pub mod wait;

#[path = "../../../kernel/src/syscall/mqueue.rs"]
pub mod mqueue;
#[path = "../../../kernel/src/syscall/sysv.rs"]
pub mod sysv;

/// The System V calls as `sysv::dispatch` reaches them. Each records its name
/// and arguments, so a test sees which call a number routes to and how its
/// registers were narrowed to Linux's argument types.
pub mod ipc {
    pub mod sysv {
        use crate::SyscallResult;
        use std::cell::RefCell;
        use std::string::String;
        use std::vec::Vec;
        std::thread_local! {
            pub static CALLS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
        }
        fn record(call: String) -> SyscallResult {
            CALLS.with(|calls| calls.borrow_mut().push(call));
            SyscallResult::Ok(0)
        }
        pub mod msg {
            use super::record;
            use crate::SyscallResult;
            pub fn sys_msgget(key: i32, flags: u32) -> SyscallResult {
                record(format!("msgget {key} {flags:o}"))
            }
            pub fn sys_msgsnd(id: i32, msgp: u64, msgsz: u64, flags: u32) -> SyscallResult {
                record(format!("msgsnd {id} {msgp:#x} {msgsz} {flags:o}"))
            }
            pub fn sys_msgrcv(id: i32, msgp: u64, msgsz: u64, msgtyp: i64, flags: u32) -> SyscallResult {
                record(format!("msgrcv {id} {msgp:#x} {msgsz} {msgtyp} {flags:o}"))
            }
            pub fn sys_msgctl(id: i32, cmd: u32, buf: u64) -> SyscallResult {
                record(format!("msgctl {id} {cmd} {buf:#x}"))
            }
        }
        pub mod sem {
            use super::record;
            use crate::SyscallResult;
            pub fn sys_semget(key: i32, nsems: i32, flags: u32) -> SyscallResult {
                record(format!("semget {key} {nsems} {flags:o}"))
            }
            pub fn sys_semtimedop(id: i32, sops: u64, nsops: u64, timeout: u64) -> SyscallResult {
                record(format!("semtimedop {id} {sops:#x} {nsops} {timeout:#x}"))
            }
            pub fn sys_semctl(id: i32, num: i32, cmd: u32, arg: u64) -> SyscallResult {
                record(format!("semctl {id} {num} {cmd} {arg:#x}"))
            }
        }
        pub mod shm {
            use super::record;
            use crate::SyscallResult;
            pub fn sys_shmget(key: i32, size: u64, flags: u32) -> SyscallResult {
                record(format!("shmget {key} {size} {flags:o}"))
            }
            pub fn sys_shmat(id: i32, addr: u64, flags: u32) -> SyscallResult {
                record(format!("shmat {id} {addr:#x} {flags:o}"))
            }
            pub fn sys_shmdt(addr: u64) -> SyscallResult {
                record(format!("shmdt {addr:#x}"))
            }
            pub fn sys_shmctl(id: i32, cmd: u32, buf: u64) -> SyscallResult {
                record(format!("shmctl {id} {cmd} {buf:#x}"))
            }
        }
    }
}
pub mod syscall {
    pub mod errno {
        pub const EINVAL: i32 = 22;
        pub const EFAULT: i32 = 14;
        pub const EAGAIN: i32 = 11;
        pub const EPIPE: i32 = 32;
    }
    #[derive(Debug, PartialEq, Eq)]
    pub enum SyscallResult {
        Ok(u64),
        Err(u64),
    }
    pub mod fs {
        #[derive(Default)]
        pub struct Stat {
            pub st_size: i64,
        }
    }
    pub mod userptr {
        // These host tests pass valid pointers to stack-owned ABI values.
        pub fn copy_from_user<T: Copy>(ptr: *const T) -> Result<T, u64> {
            if ptr.is_null() {
                return Err(super::errno::EFAULT as u64);
            }
            Ok(unsafe { ptr.read_unaligned() })
        }
    }
    pub mod blocking_io {
        use crate::task::waitqueue::{PrepareOutcome, WaitQueueHead};
        pub fn wait_prepared(q: &WaitQueueHead, outcome: PrepareOutcome) -> Result<(), i32> {
            if outcome == PrepareOutcome::Queued {
                q.resume();
            }
            if q.interrupted.get() {
                Err(if q.error.get() == 0 { 4 } else { q.error.get() })
            } else {
                Ok(())
            }
        }
    }
    pub use crate::{mqueue, sysv};
}
pub mod time {
    std::thread_local! {
        pub static MONO: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
        pub static REAL: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    }
    pub fn get_monotonic_time_ns() -> (u64, u64) {
        split(MONO.get())
    }
    pub fn get_real_time_ns() -> (i64, i64) {
        let (s, n) = split(REAL.get());
        (s as i64, n as i64)
    }
    fn split(n: u64) -> (u64, u64) {
        (n / 1_000_000_000, n % 1_000_000_000)
    }
    pub fn current_unix_time() -> u64 {
        1
    }
}
pub mod process {
    use alloc::sync::Arc;
    #[derive(Clone, Copy, Debug)]
    pub struct ProcessId(pub u64);
    impl ProcessId {
        pub fn new(value: u64) -> Self { Self(value) }
        pub fn as_u64(self) -> u64 { self.0 }
    }
    pub struct Ids {
        pub uid: u32,
        pub gid: u32,
        pub euid: u32,
        pub egid: u32,
        pub groups: Arc<alloc::vec::Vec<u32>>,
    }
    pub struct Process {
        pub cred: Ids,
        pub umask: u32,
    }
    pub struct Manager;
    impl Manager {
        pub fn find_process_by_thread(&self, _tid: u64) -> Option<(ProcessId, &Process)> {
            None
        }
    }
    pub fn manager() -> Option<Manager> {
        None
    }
}
pub mod task {
    pub mod thread {
        #[derive(Clone, Copy, PartialEq, Eq)]
        pub enum ThreadState {
            BlockedOnIO,
        }
    }
    pub mod scheduler {
        pub struct Scheduler;
        impl Scheduler {
            pub fn unblock(&mut self, _tid: u64) {}
        }
        pub fn with_scheduler<R>(f: impl FnOnce(&mut Scheduler) -> R) -> Option<R> {
            Some(f(&mut Scheduler))
        }
        pub fn current_thread_id() -> Option<u64> {
            Some(1)
        }
    }
    pub mod waitqueue {
        pub struct Cell<T: Copy>(spin::Mutex<T>);
        impl<T: Copy + Default> Default for Cell<T> {
            fn default() -> Self {
                Self(spin::Mutex::new(T::default()))
            }
        }
        impl<T: Copy> Cell<T> {
            pub fn get(&self) -> T {
                *self.0.lock()
            }
            pub fn set(&self, value: T) {
                *self.0.lock() = value;
            }
        }
        #[derive(Clone, Copy, PartialEq, Eq)]
        pub enum PrepareOutcome {
            Mismatch,
            Queued,
        }
        #[derive(Default)]
        pub struct WaitQueueHead {
            pub interrupted: Cell<bool>,
            pub error: Cell<i32>,
            pub wake: Cell<Option<u64>>,
            pub realtime_after: Cell<Option<u64>>,
        }
        impl WaitQueueHead {
            pub fn new() -> Self {
                Self::default()
            }
            pub fn wake_up(&self) {}
            pub fn wake_up_deferred(&self) {}
            pub fn prepare_to_wait_checked(
                &self,
                _state: super::thread::ThreadState,
                wake: Option<u64>,
                cond: impl FnOnce() -> bool,
            ) -> PrepareOutcome {
                if !cond() {
                    return PrepareOutcome::Mismatch;
                }
                self.wake.set(wake);
                PrepareOutcome::Queued
            }
            pub fn resume(&self) {
                if let Some(wake) = self.wake.get() {
                    crate::time::MONO.set(wake);
                }
                if let Some(real) = self.realtime_after.get() {
                    crate::time::REAL.set(real);
                }
            }
        }
    }
}

#[test]
fn foundation_does_not_claim_unimplemented_calls() {
    for number in [
        dispatch::MQ_OPEN_SYSCALL_NUMBER,
        u64::MAX,
    ] {
        assert_eq!(dispatch::dispatch(number, [1, 2, 3, 4, 5, 6]), None);
    }
}

#[test]
fn sysv_dispatch_routes_each_call_with_linux_argument_types() {
    let wide = |low: u64| low | 0xdead_0000_0000;
    let neg = |v: i64| v as u64;
    let cases = [
        (dispatch::MSGGET_SYSCALL_NUMBER, [neg(-1), wide(0o1640), 0, 0, 0, 0], "msgget -1 1640"),
        (dispatch::MSGSND_SYSCALL_NUMBER, [3, 0x1000, 64, wide(0o4000), 0, 0], "msgsnd 3 0x1000 64 4000"),
        (dispatch::MSGRCV_SYSCALL_NUMBER, [3, 0x1000, 64, neg(-2), wide(0o14000), 0], "msgrcv 3 0x1000 64 -2 14000"),
        (dispatch::MSGCTL_SYSCALL_NUMBER, [3, wide(0x102), 0x2000, 0, 0, 0], "msgctl 3 258 0x2000"),
        (dispatch::SEMGET_SYSCALL_NUMBER, [wide(7), 3, 0o1600, 0, 0, 0], "semget 7 3 1600"),
        (dispatch::SEMOP_SYSCALL_NUMBER, [5, 0x3000, wide(2), 0x4000, 0, 0], "semtimedop 5 0x3000 2 0x0"),
        (dispatch::SEMTIMEDOP_SYSCALL_NUMBER, [5, 0x3000, 1, 0x4000, 0, 0], "semtimedop 5 0x3000 1 0x4000"),
        (dispatch::SEMCTL_SYSCALL_NUMBER, [5, 1, 16, wide(65536), 0, 0], "semctl 5 1 16 0xdead00010000"),
        (dispatch::SHMGET_SYSCALL_NUMBER, [wide(9), 4196, wide(0o1640), 0, 0, 0], "shmget 9 4196 1640"),
        (dispatch::SHMAT_SYSCALL_NUMBER, [9, 0x5000, wide(0o10000), 0, 0, 0], "shmat 9 0x5000 10000"),
        (dispatch::SHMDT_SYSCALL_NUMBER, [0x5000, 0, 0, 0, 0, 0], "shmdt 0x5000"),
        (dispatch::SHMCTL_SYSCALL_NUMBER, [9, wide(2), 0x6000, 0, 0, 0], "shmctl 9 2 0x6000"),
    ];
    for (number, args, want) in cases {
        ipc::sysv::CALLS.with(|calls| calls.borrow_mut().clear());
        assert_eq!(dispatch::dispatch(number, args), Some(SyscallResult::Ok(0)), "{want}");
        let got = ipc::sysv::CALLS.with(|calls| calls.borrow().clone());
        assert_eq!(got, [want]);
    }
}

#[test]
fn deadline_waits_distinguish_readiness_timeout_and_interrupt() {
    use wait::{IpcDeadline, WaitEnd};
    let q = task::waitqueue::WaitQueueHead::new();
    time::MONO.set(20);
    assert_eq!(
        wait::wait_on(&q, &IpcDeadline::Monotonic(10), || false),
        WaitEnd::Woken
    );
    assert_eq!(
        wait::wait_on(&q, &IpcDeadline::Monotonic(10), || true),
        WaitEnd::TimedOut
    );
    q.interrupted.set(true);
    assert_eq!(
        wait::wait_on(&q, &IpcDeadline::Never, || true),
        WaitEnd::Interrupted(4)
    );
    q.interrupted.set(false);
    time::MONO.set(0);
    time::REAL.set(0);
    q.realtime_after.set(Some(5));
    assert_eq!(
        wait::wait_on(&q, &IpcDeadline::Realtime(50), || true),
        WaitEnd::Woken
    );
    q.realtime_after.set(Some(60));
    assert_eq!(
        wait::wait_on(&q, &IpcDeadline::Realtime(50), || true),
        WaitEnd::TimedOut
    );
}

#[test]
fn ipc_file_defaults_and_downcast() {
    #[derive(Debug)]
    struct File;
    impl object::IpcFile for File {
        fn as_any(&self) -> &dyn core::any::Any {
            self
        }
        fn fill_stat(&self, st: &mut syscall::fs::Stat) {
            st.st_size = 32;
        }
    }
    let object: alloc::sync::Arc<dyn object::IpcFile> = alloc::sync::Arc::new(File);
    assert!(object.as_any().is::<File>());
    let mut stat = syscall::fs::Stat::default();
    object.fill_stat(&mut stat);
    assert_eq!(stat.st_size, 32);
    assert_eq!(object.truncate(8), Err(22));
    assert_eq!(object.poll_events(), 0);
    let cred = permissions::Credentials::current(false);
    assert_eq!(object.chmod(0o600, &cred), Err(22));
    assert_eq!(object.chown(1, 2, &cred), Err(22));
    assert_eq!(object.getdents(0, 4096), Err(20));
    assert_eq!(object.statfs_magic(), 0x01021994);
    object.on_close(process::ProcessId(7));
    #[derive(Debug)]
    struct Queue;
    impl object::IpcFile for Queue {
        fn as_any(&self) -> &dyn core::any::Any { self }
        fn fill_stat(&self, _: &mut syscall::fs::Stat) {}
        fn statfs_magic(&self) -> u64 { 0x19800202 }
    }
    let queue: alloc::sync::Arc<dyn object::IpcFile> = alloc::sync::Arc::new(Queue);
    assert_eq!(queue.statfs_magic(), 0x19800202);
}

#[path = "../../../kernel/src/fs/devfs/entries.rs"]
mod devfs_entries;

#[path = "../../../kernel/src/syscall/at_path.rs"]
mod at_path;

#[path = "../../../kernel/src/fs/ext2/inode_index.rs"]
mod inode_index;

#[path = "../../../kernel/src/fs/ext2/dir.rs"]
pub mod dir_entries;

#[path = "../../../kernel/src/ipc/fd_flags.rs"]
mod fd_flags;
#[test]
fn interrupted_wait_preserves_the_original_restart_or_exit_errno() {
    let q = task::waitqueue::WaitQueueHead::new();
    q.interrupted.set(true);
    for error in [4,512,3,12] {
        q.error.set(error);
        assert_eq!(wait::wait_on(&q,&wait::IpcDeadline::Never,||true),wait::WaitEnd::Interrupted(error as u64));
    }
}

#[path = "../../../kernel/src/task/waitqueue_alloc.rs"]
mod waitqueue_alloc;

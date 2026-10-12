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
                Err(4)
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
        dispatch::MSGRCV_SYSCALL_NUMBER,
        dispatch::SEMTIMEDOP_SYSCALL_NUMBER,
        dispatch::SHMCTL_SYSCALL_NUMBER,
        u64::MAX,
    ] {
        assert_eq!(dispatch::dispatch(number, [1, 2, 3, 4, 5, 6]), None);
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
        WaitEnd::Interrupted
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

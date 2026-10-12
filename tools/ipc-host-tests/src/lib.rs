//! Host execution of production IPC logic with scheduler/user-copy shims.
#![cfg(test)]
#![feature(allocator_api)]
extern crate alloc;
pub mod irq_safe_mutex {
    pub type IrqSafeMutex<T> = spin::Mutex<T>;
}
pub use syscall::SyscallResult;
include!(concat!(env!("OUT_DIR"), "/ipc_numbers.rs"));
#[path = "../../ext2-host-tests/src/block.rs"]
pub mod block;
#[path = "../../ext2-host-tests/src/ext2.rs"]
pub mod ext2;
#[path = "../../../kernel/src/fs/permissions.rs"]
pub mod permissions;
pub mod fs {
    pub use crate::permissions;
    pub mod ext2 {
        pub use crate::ext2::*;
        pub mod live_inode {
            use alloc::sync::Arc;
            #[derive(Clone, Copy, Debug, PartialEq, Eq)]
            pub struct FileKey {
                pub mount_instance: u64,
                pub inode: u32,
                pub incarnation: u64,
            }
            #[derive(Debug)]
            pub struct LiveInode {
                pub key: FileKey,
            }
            #[derive(Clone, Debug)]
            pub struct FileHandle {
                pub object: Arc<LiveInode>,
            }
        }
    }
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
        pub fn new(value: u64) -> Self {
            Self(value)
        }
        pub fn as_u64(self) -> u64 {
            self.0
        }
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
    assert_eq!(object.write(&[], false), Err(22));
    assert_eq!(object.seek(0, 0), Err(29));
    let mut updates = 0;
    object.update_status_flags(&mut || updates += 1);
    assert_eq!(updates, 1);
    object.on_close(process::ProcessId(7));
    #[derive(Debug)]
    struct Queue;
    impl object::IpcFile for Queue {
        fn as_any(&self) -> &dyn core::any::Any {
            self
        }
        fn fill_stat(&self, _: &mut syscall::fs::Stat) {}
        fn statfs_magic(&self) -> u64 {
            0x19800202
        }
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
    for error in [4, 512, 3, 12] {
        q.error.set(error);
        assert_eq!(
            wait::wait_on(&q, &wait::IpcDeadline::Never, || true),
            wait::WaitEnd::Interrupted(error as u64)
        );
    }
}

#[path = "../../../kernel/src/ipc/fifo.rs"]
pub mod fifo;
#[path = "../../../kernel/src/ipc/pipe.rs"]
pub mod pipe;
#[path = "../../../kernel/src/task/waitqueue_alloc.rs"]
mod waitqueue_alloc;
pub mod arch_impl {
    pub mod traits {
        pub trait CpuOps {
            fn without_interrupts<F: FnOnce() -> R, R>(f: F) -> R;
        }
    }
    pub mod x86_64 {
        pub use crate::HostCpu as X86Cpu;
    }
    pub mod aarch64 {
        pub use crate::HostCpu as Aarch64Cpu;
    }
}
pub struct HostCpu;
impl arch_impl::traits::CpuOps for HostCpu {
    fn without_interrupts<F: FnOnce() -> R, R>(f: F) -> R {
        f()
    }
}

#[test]
fn fifo_reopen_discards_data_after_either_close_order() {
    for writer_first in [false, true] {
        let entry = alloc::sync::Arc::new(spin::Mutex::new(fifo::FifoEntry::new()));
        let fifo::FifoOpenResult::Ready(buffer) = fifo::open_fifo_read(&entry, true) else {
            panic!("read open");
        };
        assert!(matches!(
            fifo::open_fifo_write(&entry, true),
            fifo::FifoOpenResult::Ready(_)
        ));
        assert_eq!(buffer.lock().write(b"old"), Ok(3));
        if writer_first {
            buffer.lock().close_write().deliver();
            fifo::close_fifo_write(&entry);
            assert!(entry.lock().buffer.is_some());
            buffer.lock().close_read().deliver();
            fifo::close_fifo_read(&entry);
        } else {
            buffer.lock().close_read().deliver();
            fifo::close_fifo_read(&entry);
            assert!(entry.lock().buffer.is_some());
            buffer.lock().close_write().deliver();
            fifo::close_fifo_write(&entry);
        }
        assert!(entry.lock().buffer.is_none());
        let fifo::FifoOpenResult::Ready(reopened) = fifo::open_fifo_read(&entry, true) else {
            panic!("reopen");
        };
        assert!(!alloc::sync::Arc::ptr_eq(&buffer, &reopened));
        assert_eq!(reopened.lock().read(&mut [0; 3]), Ok(0));
        assert!(matches!(
            fifo::open_fifo_write(&entry, true),
            fifo::FifoOpenResult::Ready(_)
        ));
        assert_eq!(reopened.lock().read(&mut [0; 3]), Err(11));
    }
}

#[test]
fn fifo_abandoned_pending_open_resets_only_the_last_endpoint() {
    for for_write in [false, true] {
        let entry = alloc::sync::Arc::new(spin::Mutex::new(fifo::FifoEntry::new()));
        let result = if for_write {
            fifo::open_fifo_write(&entry, false)
        } else {
            fifo::open_fifo_read(&entry, false)
        };
        assert!(matches!(result, fifo::FifoOpenResult::Block(0)));
        let old = entry.lock().buffer.clone().unwrap();
        fifo::abandon_fifo_open(&fifo::PendingFifoOpen {
            tid: 1,
            entry: entry.clone(),
            for_write,
        });
        let entry = entry.lock();
        assert_eq!((entry.readers, entry.writers), (0, 0));
        assert!(entry.buffer.is_none());
        assert!(entry.read_waiters.is_empty() && entry.write_waiters.is_empty());
        assert!(!old.lock().has_readers() && !old.lock().has_writers());
    }
}

#[test]
fn fifo_open_partner_history_survives_a_short_lived_peer() {
    let entry = alloc::sync::Arc::new(spin::Mutex::new(fifo::FifoEntry::new()));
    let fifo::FifoOpenResult::Block(seen) = fifo::open_fifo_read(&entry, false) else {
        panic!("blocking reader");
    };
    let fifo::FifoOpenResult::Ready(buffer) = fifo::open_fifo_write(&entry, true) else {
        panic!("partner");
    };
    buffer.lock().close_write().deliver();
    fifo::close_fifo_write(&entry);
    assert!(entry.lock().buffer.is_some());
    assert!(matches!(
        fifo::recheck_fifo_open(&entry, false, seen),
        fifo::FifoOpenResult::Ready(_)
    ));
    assert_eq!(buffer.lock().read(&mut [0; 1]), Ok(0));
    fifo::abandon_fifo_open(&fifo::PendingFifoOpen {
        tid: 1,
        entry: entry.clone(),
        for_write: false,
    });
    assert!(entry.lock().buffer.is_none());
}

#[test]
fn fifo_nonblocking_writer_enxio_takes_no_reference() {
    let entry = alloc::sync::Arc::new(spin::Mutex::new(fifo::FifoEntry::new()));
    assert!(matches!(
        fifo::open_fifo_write(&entry, true),
        fifo::FifoOpenResult::Error(6)
    ));
    let entry = entry.lock();
    assert_eq!((entry.readers, entry.writers), (0, 0));
    assert_eq!(entry.writer_opens, 0);
    assert!(entry.buffer.is_none());
}

fn handle(ino: u32, incarnation: u64) -> fs::ext2::live_inode::FileHandle {
    use fs::ext2::live_inode::*;
    FileHandle {
        object: alloc::sync::Arc::new(LiveInode {
            key: FileKey {
                mount_instance: 1,
                inode: ino,
                incarnation,
            },
        }),
    }
}
#[test]
fn inode_aliases_share_runtime_and_recreated_names_do_not() {
    let registry = fifo::FifoRegistry::new();
    let original = registry.get_or_create(handle(11, 1)).unwrap();
    let alias = registry.get_or_create(handle(11, 1)).unwrap();
    assert!(alloc::sync::Arc::ptr_eq(&original, &alias));
    let fifo::FifoOpenResult::Ready(buffer) = fifo::open_fifo_readwrite(&original) else {
        panic!("duplex");
    };
    assert_eq!(buffer.lock().write(b"old"), Ok(3));
    let replacement = registry.get_or_create(handle(11, 2)).unwrap();
    assert!(!alloc::sync::Arc::ptr_eq(&original, &replacement));
    let fifo::FifoOpenResult::Ready(new_buffer) = fifo::open_fifo_readwrite(&replacement) else {
        panic!("new duplex");
    };
    assert_eq!(new_buffer.lock().read(&mut [0; 3]), Err(11));
    let mut got = [0; 3];
    assert_eq!(buffer.lock().read(&mut got), Ok(3));
    assert_eq!(&got, b"old");
}

#[test]
fn pipe_atomic_writes_partial_reads_eof_and_broken_pipe() {
    let mut pipe = pipe::PipeBuffer::new();
    assert_eq!(
        pipe.write(&vec![7; pipe::PIPE_BUF_SIZE - 1]),
        Ok(pipe::PIPE_BUF_SIZE - 1)
    );
    assert!(matches!(
        pipe.try_write(&[1, 2], true),
        Err(pipe::WriteAttempt::WouldBlock)
    ));
    assert_eq!(pipe.available(), pipe::PIPE_BUF_SIZE - 1);
    assert_eq!(
        pipe.read(&mut [0; pipe::PIPE_BUF_SIZE]),
        Ok(pipe::PIPE_BUF_SIZE - 1)
    );
    assert_eq!(pipe.write(b"abc"), Ok(3));
    let mut got = [0; 8];
    assert_eq!(pipe.read(&mut got), Ok(3));
    assert_eq!(&got[..3], b"abc");
    assert_eq!(pipe.read(&mut got), Err(11));
    pipe.close_write().deliver();
    assert_eq!(pipe.read(&mut got), Ok(0));
    pipe.close_read().deliver();
    assert_eq!(pipe.write(b"x"), Err(32));
}

#[test]
fn closing_a_duplicated_end_preserves_the_surviving_descriptors_data() {
    let entry = alloc::sync::Arc::new(spin::Mutex::new(fifo::FifoEntry::new()));
    let fifo::FifoOpenResult::Ready(buffer) = fifo::open_fifo_read(&entry, true) else {
        panic!("reader");
    };
    assert!(matches!(
        fifo::open_fifo_write(&entry, true),
        fifo::FifoOpenResult::Ready(_)
    ));
    // This is the endpoint reference increment used by fd-table dup/fork.
    entry.lock().readers += 1;
    buffer.lock().add_reader();
    assert_eq!(buffer.lock().write(b"held"), Ok(4));
    buffer.lock().close_write().deliver();
    fifo::close_fifo_write(&entry);
    buffer.lock().close_read().deliver();
    fifo::close_fifo_read(&entry);
    assert!(entry.lock().buffer.is_some());
    let mut got = [0; 4];
    assert_eq!(buffer.lock().read(&mut got), Ok(4));
    assert_eq!(&got, b"held");
    buffer.lock().close_read().deliver();
    fifo::close_fifo_read(&entry);
    assert!(entry.lock().buffer.is_none());
}

#[test]
fn abandoned_writer_leaves_buffered_data_then_eof_for_the_reader() {
    let entry = alloc::sync::Arc::new(spin::Mutex::new(fifo::FifoEntry::new()));
    let fifo::FifoOpenResult::Block(seen) = fifo::open_fifo_write(&entry, false) else {
        panic!("writer");
    };
    let fifo::FifoOpenResult::Ready(buffer) = fifo::open_fifo_read(&entry, true) else {
        panic!("reader");
    };
    assert!(matches!(
        fifo::recheck_fifo_open(&entry, true, seen),
        fifo::FifoOpenResult::Ready(_)
    ));
    assert_eq!(buffer.lock().write(b"held"), Ok(4));
    fifo::abandon_fifo_open(&fifo::PendingFifoOpen {
        tid: 1,
        entry: entry.clone(),
        for_write: true,
    });
    assert!(entry.lock().buffer.is_some());
    let mut got = [0; 4];
    assert_eq!(buffer.lock().read(&mut got), Ok(4));
    assert_eq!(&got, b"held");
    assert_eq!(buffer.lock().read(&mut got), Ok(0));
    buffer.lock().close_read().deliver();
    fifo::close_fifo_read(&entry);
    assert!(entry.lock().buffer.is_none());
}

#[test]
fn runtime_table_drops_the_last_inode_observer() {
    let registry = fifo::FifoRegistry::new();
    let pin = handle(11, 1);
    let object = alloc::sync::Arc::downgrade(&pin.object);
    let entry = registry.get_or_create(pin).unwrap();
    assert!(object.upgrade().is_some());
    drop(entry);
    assert!(object.upgrade().is_none());
}

#[test]
fn duplex_close_preserves_other_endpoints_and_reopen_discards_bytes() {
    let entry = alloc::sync::Arc::new(spin::Mutex::new(fifo::FifoEntry::new()));
    let fifo::FifoOpenResult::Ready(buffer) = fifo::open_fifo_readwrite(&entry) else {
        panic!("open");
    };
    assert_eq!(buffer.lock().write(b"abc"), Ok(3));
    assert!(matches!(
        fifo::open_fifo_read(&entry, true),
        fifo::FifoOpenResult::Ready(_)
    ));
    let read = buffer.lock().close_read();
    read.deliver();
    fifo::close_fifo_read(&entry);
    let write = buffer.lock().close_write();
    write.deliver();
    fifo::close_fifo_write(&entry);
    let mut got = [0; 3];
    assert_eq!(buffer.lock().read(&mut got), Ok(3));
    assert_eq!(&got, b"abc");
    assert_eq!(buffer.lock().read(&mut got), Ok(0));
    let read = buffer.lock().close_read();
    read.deliver();
    fifo::close_fifo_read(&entry);
    assert!(entry.lock().buffer.is_none());
    let fifo::FifoOpenResult::Ready(reopened) = fifo::open_fifo_readwrite(&entry) else {
        panic!("reopen");
    };
    assert!(!alloc::sync::Arc::ptr_eq(&buffer, &reopened));
    assert_eq!(reopened.lock().read(&mut got), Err(11));
}

#[path = "../../../kernel/src/syscall/fifo_policy.rs"]
mod fifo_policy;

mod fifo_disk;

mod allocation;

#[path = "../../../kernel/src/fs/ext2/namespace_alloc.rs"]
mod namespace_alloc;

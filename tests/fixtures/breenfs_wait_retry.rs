// Force the checked waitqueue's outcomes while exercising the real wait callers.
extern crate self as spin;

use std::cell::{Cell, RefCell};
use std::sync::OnceLock;

pub struct Once<T>(OnceLock<T>);
impl<T> Once<T> {
    pub const fn new() -> Self {
        Self(OnceLock::new())
    }
    pub fn call_once(&self, init: impl FnOnce() -> T) -> &T {
        self.0.get_or_init(init)
    }
}

thread_local! {
    static BRAKE: Cell<u32> = const { Cell::new(0) };
    static IRQ_ENABLED: Cell<bool> = const { Cell::new(true) };
    static CPU: Cell<u64> = const { Cell::new(0) };
    static TID: Cell<u64> = const { Cell::new(7) };
    static MIGRATE_TO: Cell<Option<u64>> = const { Cell::new(None) };
    static KILL_DURING_PREPARE: Cell<bool> = const { Cell::new(false) };
    static KILL_PENDING: Cell<bool> = const { Cell::new(false) };
    static HALTS: Cell<u32> = const { Cell::new(0) };
    static PREPARES: Cell<u32> = const { Cell::new(0) };
    static HOLDER: RefCell<Option<fs::mount_lock::MountGuard<'static, u32>>> =
        const { RefCell::new(None) };
}

pub mod per_cpu {
    pub fn preempt_count() -> u32 {
        super::BRAKE.with(|count| count.get())
    }
    pub fn preempt_disable() {
        super::BRAKE.with(|count| count.set(count.get() + 1));
    }
    pub fn preempt_enable() {
        super::BRAKE.with(|count| count.set(count.get() - 1));
    }
    pub fn in_interrupt() -> bool {
        false
    }
}
pub use per_cpu as per_cpu_aarch64;

pub mod arch_impl {
    pub mod aarch64 {
        pub mod cpu {
            pub fn interrupts_enabled() -> bool {
                true
            }
        }
        pub mod timer_interrupt {
            pub fn is_initialized() -> bool {
                true
            }
        }
    }
}

pub fn arch_without_interrupts<T>(f: impl FnOnce() -> T) -> T {
    let enabled = IRQ_ENABLED.replace(false);
    let result = f();
    IRQ_ENABLED.set(enabled);
    run_pending_migration();
    result
}

fn run_pending_migration() {
    if IRQ_ENABLED.get() && per_cpu::preempt_count() == 0 {
        if let Some(cpu) = MIGRATE_TO.take() {
            CPU.set(cpu);
        }
    }
}
pub fn arch_halt_with_interrupts() {
    assert_eq!(per_cpu::preempt_count(), 0, "the owner may need this CPU");
    HALTS.with(|count| count.set(count.get() + 1));
    HOLDER.with(|holder| drop(holder.borrow_mut().take()));
}

pub mod time {
    pub fn get_monotonic_time_ns() -> (u64, u64) {
        (0, 1)
    }
}
pub mod memory {
    pub mod frame_allocator {
        pub struct Stats {
            pub total_bytes: u64,
        }
        pub fn memory_stats() -> Stats {
            Stats { total_bytes: 0 }
        }
    }
}
pub mod process {
    use std::sync::atomic::{AtomicU64, Ordering};

    const PM_LOCK_OWNER_NONE: u64 = u64::MAX;
    static OWNER_CPU: AtomicU64 = AtomicU64::new(PM_LOCK_OWNER_NONE);
    static PROCESS_MANAGER_OWNER_TID: AtomicU64 = AtomicU64::new(PM_LOCK_OWNER_NONE);

    struct OwnerCpu;
    static PROCESS_MANAGER_OWNER_CPU: OwnerCpu = OwnerCpu;
    impl OwnerCpu {
        fn load(&self, ordering: Ordering) -> u64 {
            let sampled = OWNER_CPU.load(ordering);
            if super::MIGRATE_TO.get().is_some() {
                // The remote holder releases after the load. The requesting
                // thread can then resume on that CPU if the mask permits it.
                PROCESS_MANAGER_OWNER_TID.store(PM_LOCK_OWNER_NONE, Ordering::Relaxed);
                OWNER_CPU.store(PM_LOCK_OWNER_NONE, Ordering::Release);
                super::run_pending_migration();
            }
            sampled
        }
    }

    pub fn hold(cpu: u64, tid: u64) {
        PROCESS_MANAGER_OWNER_TID.store(tid, Ordering::Relaxed);
        OWNER_CPU.store(cpu, Ordering::Release);
    }

    pub fn current_process_manager_cpu() -> u64 {
        super::CPU.get()
    }

    pub fn current_process_manager_tid() -> u64 {
        super::TID.get()
    }

    pub fn current_process_manager_owner_identity() -> (u64, u64) {
        (current_process_manager_cpu(), current_process_manager_tid())
    }

    PM_OWNER_QUERIES
}
pub mod tracing {
    pub mod output {
        pub fn raw_serial_char(byte: u8) -> bool {
            eprint!("{}", char::from(byte));
            true
        }
    }
}
pub mod serial_aarch64 {
    pub use crate::tracing::output::raw_serial_char as write_failure_byte;
}
pub mod syscall {
    pub mod errno {
        pub const EINTR: i32 = 4;
    }
}

pub mod task {
    pub mod scheduler {
        pub fn current_thread_id() -> Option<u64> {
            Some(7)
        }
        pub fn set_need_resched() {}
    }
    pub mod thread {
        pub fn current_wait_must_unwind() -> bool {
            super::super::KILL_PENDING.get()
        }
        pub enum ThreadState {
            BlockedOnIO,
        }
    }
    pub mod waitqueue {
        use super::thread::ThreadState;
        use super::super::{KILL_DURING_PREPARE, KILL_PENDING};
        use std::cell::Cell;

        #[derive(Clone, Copy, PartialEq, Eq)]
        pub enum PrepareOutcome {
            Queued,
            Mismatch,
            PublishFailed,
            OutOfMemory,
        }
        thread_local! {
            pub static REFUSAL: Cell<PrepareOutcome> = const { Cell::new(PrepareOutcome::OutOfMemory) };
        }
        pub struct WaitQueueHead;
        impl WaitQueueHead {
            pub fn new() -> Self {
                Self
            }
            pub fn prepare_to_wait_checked(
                &self,
                _: ThreadState,
                _: Option<u64>,
                matches: impl FnOnce() -> bool,
            ) -> PrepareOutcome {
                assert_eq!(super::super::per_cpu::preempt_count(), 1);
                super::super::PREPARES.with(|count| {
                    assert_eq!(count.replace(count.get() + 1), 0, "retried before sleeping");
                });
                if KILL_DURING_PREPARE.get() {
                    KILL_PENDING.set(true);
                }
                if matches() {
                    REFUSAL.get()
                } else {
                    PrepareOutcome::Mismatch
                }
            }
            pub fn finish_wait(&self) {
                assert!(KILL_PENDING.get(), "no waiter was published");
            }
            pub fn wake_up(&self) {}
            pub fn wake_up_deferred(&self) {}
        }
        pub fn schedule_current_wait() {
            assert!(KILL_PENDING.get(), "no waiter was published");
        }
    }
}

pub mod block {
    pub mod cache {
        pub trait Wait {
            fn generation(&self) -> u64;
            fn wait(&self, observed: u64);
            fn wake_all(&self);
        }
        pub struct BufferCache<W>(pub W);
        impl<W> BufferCache<W> {
            pub fn with_capacity(wait: W, _: usize) -> Self {
                Self(wait)
            }
        }
        pub fn bound_for_memory(_: usize) -> usize {
            0
        }
    }
    #[path = "CACHE_PATH"]
    pub mod cache_kernel;
}
pub mod fs {
    pub mod errno {
        #[derive(Debug)]
        pub struct Errno(pub i32);
    }
    #[path = "SIGNAL_PATH"]
    pub mod commit_signal;
    #[path = "MOUNT_PATH"]
    pub mod mount_lock;
}

fn main() {
    use block::cache::Wait;
    use fs::{commit_signal::CommitSignal, mount_lock::MountLock};
    use task::waitqueue::{PrepareOutcome, REFUSAL};

    static MOUNT: OnceLock<MountLock<u32>> = OnceLock::new();
    let scenario = std::env::args().nth(1).unwrap();
    let inherited_brake = scenario == "worker" || scenario == "publish-refused";
    if inherited_brake {
        per_cpu::preempt_disable();
    }
    match scenario.as_str() {
        "pm-held" => {
            process::hold(CPU.get(), TID.get());
            // Exit before unwinding across the assertion's architecture entry.
            std::panic::set_hook(Box::new(|info| {
                eprintln!("{info}");
                std::process::exit(17);
            }));
            block::cache_kernel::assert_io_context();
            return;
        }
        "pm-migrate" => {
            process::hold(3, 19);
            MIGRATE_TO.set(Some(3));
            block::cache_kernel::assert_io_context();
            assert_eq!(CPU.get(), 3, "the attempted migration must run");
            assert!(IRQ_ENABLED.get(), "the IRQ mask must be restored");
            assert!(process::process_manager_owner_snapshot().is_none());
            return;
        }
        "worker" => {
            let signal = CommitSignal::new();
            let _ = signal.worker.wait(signal.worker.epoch(), Some(10));
        }
        "foreground" => {
            let signal = CommitSignal::new();
            let _ = signal.wait(signal.epoch(), None);
        }
        "mount" => {
            let mount = MOUNT.get_or_init(|| MountLock::new(4));
            HOLDER.with(|holder| holder.replace(Some(mount.lock())));
            assert_eq!(*mount.lock(), 4);
        }
        "cache" => {
            let cache = &block::cache_kernel::system_cache().0;
            cache.wait(cache.generation());
        }
        "publish-refused" => {
            REFUSAL.set(PrepareOutcome::PublishFailed);
            let signal = CommitSignal::new();
            let outcome = signal.wait(signal.epoch(), None);
            assert_eq!(format!("{outcome:?}"), "Err(Errno(4))");
        }
        "killed-during-wake" => {
            KILL_DURING_PREPARE.set(true);
            REFUSAL.set(PrepareOutcome::Queued);
            let signal = CommitSignal::new();
            assert_eq!(format!("{:?}", signal.wait(signal.epoch(), None)), "Err(Errno(4))");
        }
        "killed-allocation-refused" => {
            KILL_DURING_PREPARE.set(true);
            let signal = CommitSignal::new();
            assert_eq!(format!("{:?}", signal.wait(signal.epoch(), None)), "Err(Errno(4))");
        }
        "killed-mount-contender" => {
            let mount = MOUNT.get_or_init(|| MountLock::new(4));
            HOLDER.with(|holder| holder.replace(Some(mount.lock())));
            KILL_DURING_PREPARE.set(true);
            REFUSAL.set(PrepareOutcome::PublishFailed);
            assert!(mount.lock_interruptible().is_err());
            assert!(HOLDER.with(|holder| holder.borrow().is_some()), "lock owner had to finish");
        }
        "epoch-moved" => {
            let signal = CommitSignal::new();
            let observed = signal.epoch();
            signal.wake_waiters();
            let _ = signal.wait(observed, None);
        }
        _ => panic!("unknown scenario"),
    }
    assert_eq!(PREPARES.get(), 1);
    assert_eq!(per_cpu::preempt_count(), u32::from(inherited_brake));
    let should_halt = matches!(scenario.as_str(), "worker" | "foreground" | "mount" | "cache");
    assert_eq!(HALTS.get(), u32::from(should_halt));
}

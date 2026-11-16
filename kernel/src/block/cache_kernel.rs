//! System cache ownership and scheduler waits (breenfs design section 1.2).
use super::cache::{bound_for_memory, BufferCache, Wait};
use crate::task::{
    thread::ThreadState,
    waitqueue::{PrepareOutcome, WaitQueueHead},
};
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Once;

pub struct KernelWait {
    generation: AtomicU64,
    queue: WaitQueueHead,
}

impl Wait for KernelWait {
    fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    fn wait(&self, observed: u64) {
        assert_io_context();
        if self.generation() != observed {
            return;
        }
        if crate::task::scheduler::current_thread_id().is_none() {
            return;
        }
        // schedule_current_wait temporarily releases one preemption brake.
        // Workers may need an added brake; syscall entry already holds one.
        #[cfg(target_arch = "x86_64")]
        use crate::per_cpu as cpu;
        #[cfg(target_arch = "aarch64")]
        use crate::per_cpu_aarch64 as cpu;
        let add_brake = cpu::preempt_count() == 0;
        if add_brake {
            cpu::preempt_disable();
        }
        assert_eq!(
            cpu::preempt_count(),
            1,
            "block cache wait has nested preemption guards"
        );
        let prepared = self
            .queue
            .prepare_to_wait_checked(ThreadState::BlockedOnIO, None, || {
                self.generation() == observed
            });
        if prepared == PrepareOutcome::Queued {
            crate::task::waitqueue::schedule_current_wait();
            self.queue.finish_wait();
        }
        if add_brake {
            cpu::preempt_enable();
        }
    }

    fn wake_all(&self) {
        self.generation.fetch_add(1, Ordering::Release);
        self.queue.wake_up();
    }
}

pub type SystemCache = BufferCache<KernelWait>;
static CACHE: Once<SystemCache> = Once::new();

pub fn system_cache() -> &'static SystemCache {
    crate::arch_without_interrupts(|| {
        CACHE.call_once(|| {
            let memory = crate::memory::frame_allocator::memory_stats().total_bytes;
            SystemCache::with_capacity(
                KernelWait {
                    generation: AtomicU64::new(0),
                    queue: WaitQueueHead::new(),
                },
                bound_for_memory(usize::try_from(memory).unwrap_or(usize::MAX)),
            )
        })
    })
}

/// Mount I/O and waits enter with the process-manager guard released.
pub fn assert_io_context() {
    assert!(
        !crate::process::process_manager_held_on_current_cpu(),
        "block cache entered while holding the process manager"
    );
}

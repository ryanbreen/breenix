//! System cache ownership and scheduler waits (breenfs design section 1.2).
use super::cache::{bound_for_memory, BufferCache, Wait};
use crate::task::{
    thread::ThreadState,
    waitqueue::{PrepareOutcome, WaitQueueHead},
};
use core::sync::atomic::{compiler_fence, AtomicU64, Ordering};
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
        match prepared {
            PrepareOutcome::Queued => {
                crate::task::waitqueue::schedule_current_wait();
                self.queue.finish_wait();
            }
            PrepareOutcome::OutOfMemory | PrepareOutcome::PublishFailed => {
                park_for_wait_retry();
            }
            PrepareOutcome::Mismatch => {}
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

/// Back off if no waiter could be published. Use the next interrupt instead
/// of allocating a scheduler timer entry, and allow the owner to run here.
/// The queue guard is gone and the caller holds one preemption brake.
pub(crate) fn park_for_wait_retry() {
    crate::task::scheduler::set_need_resched();
    crate::per_cpu::preempt_enable();
    crate::arch_halt_with_interrupts();
    crate::per_cpu::preempt_disable();
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
/// Pass the incoming return address before a Rust prologue can change the stack or LR.
#[unsafe(naked)]
pub extern "C" fn assert_io_context() {
    #[cfg(target_arch = "x86_64")]
    core::arch::naked_asm!("mov rdi, [rsp]", "jmp {}", sym assert_io_context_at);
    #[cfg(target_arch = "aarch64")]
    core::arch::naked_asm!("mov x0, x30", "b {}", sym assert_io_context_at);
}

extern "C" fn assert_io_context_at(caller: usize) {
    use crate::process;

    // A preemptible worker could otherwise sample a remote holder, resume on
    // its CPU after the release, and mistake the old hold for its own.
    let held = crate::arch_without_interrupts(|| {
        compiler_fence(Ordering::SeqCst);
        let held = process::process_manager_owner_snapshot().and_then(|(owner_cpu, owner_tid)| {
            let current_cpu = process::current_process_manager_cpu();
            (owner_cpu == current_cpu).then(|| {
                (
                    owner_cpu,
                    owner_tid,
                    current_cpu,
                    process::current_process_manager_tid(),
                    crate::per_cpu::preempt_count(),
                )
            })
        });
        compiler_fence(Ordering::SeqCst);
        held
    });
    if let Some((owner_cpu, owner_tid, current_cpu, current_tid, preempt)) = held {
        io_context_failure(caller, owner_cpu, owner_tid, current_cpu, current_tid, preempt);
    }
}

#[cold]
#[inline(never)]
fn io_context_failure(
    caller: usize,
    owner_cpu: u64,
    owner_tid: u64,
    current_cpu: u64,
    current_tid: u64,
    preempt: u32,
) -> ! {
    use core::fmt::{self, Write};
    #[cfg(target_arch = "aarch64")]
    use crate::serial_aarch64::write_failure_byte;
    #[cfg(target_arch = "x86_64")]
    use crate::tracing::output::raw_serial_char as write_failure_byte;

    struct Receipt;
    impl Write for Receipt {
        fn write_str(&mut self, text: &str) -> fmt::Result {
            for byte in text.bytes() {
                if !write_failure_byte(byte) {
                    return Err(fmt::Error);
                }
            }
            Ok(())
        }
    }

    let _ = write!(
        Receipt,
        "\r\n[PM_IO_ASSERT caller={caller:#x} owner_cpu={owner_cpu} owner_tid={owner_tid} \
         current_cpu={current_cpu} current_tid={current_tid} preempt={preempt}]\r\n"
    );
    panic!("block cache entered while holding the process manager");
}

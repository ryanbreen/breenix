#[cfg(test)]
mod inline_entry;

#[cfg(test)]
mod arch_impl {
    pub mod aarch64 {
        pub mod switch_save {
            pub fn is_saving(tid: u64) -> bool {
                tid == 1 && crate::SAVING_CPU.get().is_some()
            }
        }

        pub mod cpu {
            pub unsafe fn disable_interrupts() {}
        }
    }
}

#[cfg(test)]
mod task {
    pub mod scheduler {
        pub fn expire_signal_timers_arm64() {}
    }
}

#[cfg(test)]
mod queue;

#[cfg(test)]
#[allow(dead_code)] // The host fixture only drives publication and drain.
#[path = "../../../kernel/src/task/wake_inbox.rs"]
mod wake_inbox;

#[cfg(test)]
use queue::current_cpu_id_raw;

#[cfg(test)]
use std::{
    cell::Cell,
    sync::atomic::{AtomicU32, AtomicU64},
};

#[cfg(test)]
static SCHEDULER_HELD: [AtomicU32; 3] = [const { AtomicU32::new(0) }; 3];
#[cfg(test)]
static PINNED_HOLDS_OUTSTANDING: [AtomicU64; 3] = [const { AtomicU64::new(0) }; 3];
#[cfg(test)]
static QUEUED_CPUS: AtomicU64 = AtomicU64::new(0);
#[cfg(test)]
static PINNED_WAKES_DELIVERED: AtomicU64 = AtomicU64::new(0);

#[cfg(test)]
thread_local! {
    static SAVING_CPU: Cell<Option<usize>> = const { Cell::new(None) };
}

#[cfg(test)]
fn send_deferred_sgis(_: usize) {}

#[cfg(test)]
mod tracing {
    pub fn trace_timestamp() -> u64 {
        0
    }
}

#[cfg(test)]
fn post_resched_arm64(_: usize) {}

use core::sync::atomic::{AtomicU64, Ordering};

use crate::arch_without_interrupts as without_interrupts;
use crate::task::{kthread, scheduler};

static LOOPBACK_PUMP_TID: AtomicU64 = AtomicU64::new(0);
static LOOPBACK_PUMP_PASSES: AtomicU64 = AtomicU64::new(0);
static LOOPBACK_PUMP_REARMS: AtomicU64 = AtomicU64::new(0);
static LOOPBACK_PUMP_WAKES: AtomicU64 = AtomicU64::new(0);
static LOOPBACK_PUMP_WAKE_REJECTED: AtomicU64 = AtomicU64::new(0);

const PUMP_ROUNDS_PER_PASS: usize = 4;

pub fn init_loopback_pump() {
    if LOOPBACK_PUMP_TID.load(Ordering::Acquire) != 0 {
        return;
    }

    match kthread::kthread_run(loopback_pump_fn, "kloopbackd") {
        Ok(handle) => {
            let _ = LOOPBACK_PUMP_TID.compare_exchange(
                0,
                handle.tid(),
                Ordering::AcqRel,
                Ordering::Acquire,
            );
        }
        Err(error) => log::error!("failed to start kloopbackd: {:?}", error),
    }
}

pub(crate) fn wake_loopback_pump() {
    let tid = LOOPBACK_PUMP_TID.load(Ordering::Acquire);
    if tid == 0 {
        return;
    }

    LOOPBACK_PUMP_WAKES.fetch_add(1, Ordering::Relaxed);
    if scheduler::wake_thread_any_context(tid) == scheduler::WakeOutcome::Rejected {
        LOOPBACK_PUMP_WAKE_REJECTED.fetch_add(1, Ordering::Relaxed);
    }
}

fn loopback_pump_fn() {
    let Some(my_tid) = scheduler::current_thread_id() else {
        return;
    };
    let _ = LOOPBACK_PUMP_TID.compare_exchange(0, my_tid, Ordering::AcqRel, Ordering::Acquire);

    loop {
        LOOPBACK_PUMP_PASSES.fetch_add(1, Ordering::Relaxed);
        let more = super::drain_loopback_rounds(PUMP_ROUNDS_PER_PASS);
        if more {
            LOOPBACK_PUMP_REARMS.fetch_add(1, Ordering::Relaxed);
            scheduler::yield_current();
            crate::arch_halt_with_interrupts();
            continue;
        }

        without_interrupts(|| {
            scheduler::with_scheduler(|sched| {
                sched.block_current();
            });
            if !super::loopback_queue_is_empty() {
                scheduler::with_scheduler(|sched| {
                    sched.unblock(my_tid);
                });
            }
        });
        scheduler::yield_current();
        crate::arch_halt_with_interrupts();
    }
}

pub fn loopback_pump_passes() -> u64 {
    LOOPBACK_PUMP_PASSES.load(Ordering::Relaxed)
}

pub fn loopback_pump_rearms() -> u64 {
    LOOPBACK_PUMP_REARMS.load(Ordering::Relaxed)
}

pub fn loopback_pump_wakes() -> u64 {
    LOOPBACK_PUMP_WAKES.load(Ordering::Relaxed)
}

pub fn loopback_pump_wake_rejected() -> u64 {
    LOOPBACK_PUMP_WAKE_REJECTED.load(Ordering::Relaxed)
}

pub fn loopback_pump_tid() -> u64 {
    LOOPBACK_PUMP_TID.load(Ordering::Acquire)
}

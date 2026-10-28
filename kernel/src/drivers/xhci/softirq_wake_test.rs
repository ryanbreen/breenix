//! Force the first host-key press/release's command IRQs to interrupt PREEMPT.
//!
//! The device-IRQ CPU runs short lock-free preempt-disabled sections, with
//! safe release points spaced a millisecond apart. IRQs are masked over each
//! release/reacquire pair: irq_exit cannot accidentally observe PREEMPT == 0.
//! Thus the old irq_exit leaves IrqPoll pending throughout the forcing window;
//! a repair must make progress at the safe releases, without running a handler
//! over the disabled section. This uses real xHCI commands and HID dispatch.
//! No QEMU-only stimulus or synthetic completion is added by this test.
//! Publishing the worker's waiter before the command doorbell and requiring
//! its deferred callback closes the early-completion race in the forcing test.

use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use super::{host_keys, now, SECOND};
use crate::{
    per_cpu,
    task::{kthread, scheduler, thread::CpuPin},
};

const LIMIT_NS: u64 = 100_000_000;
const WINDOW_NS: u64 = 200_000_000;
static STARTED: AtomicBool = AtomicBool::new(false);
static READY: AtomicBool = AtomicBool::new(false);
static ACTIVE: AtomicBool = AtomicBool::new(false);
static CPU: AtomicUsize = AtomicUsize::new(usize::MAX);
static IRQS: AtomicU64 = AtomicU64::new(0);
static WAKES: AtomicU64 = AtomicU64::new(0);
static EXPECT_WAKE: AtomicU64 = AtomicU64::new(0);
static UNSAFE_WAKES: AtomicU64 = AtomicU64::new(0);

/// Called only in a testing kernel's xHCI ISR; atomic evidence, no output.
pub(super) fn note_irq() {
    if ACTIVE.load(Ordering::Acquire)
        && per_cpu::cpu_id() == CPU.load(Ordering::Relaxed)
        && per_cpu::preempt_count() & 0xff != 0
    {
        IRQS.fetch_add(1, Ordering::Relaxed);
    }
}

pub(super) fn note_wake() {
    if ACTIVE.load(Ordering::Acquire) && per_cpu::cpu_id() == CPU.load(Ordering::Relaxed) {
        if per_cpu::preempt_count() & 0xff != 0 {
            UNSAFE_WAKES.fetch_add(1, Ordering::Relaxed);
        }
        WAKES.fetch_add(1, Ordering::Release);
    }
}

pub(super) fn publish_command_waiter() {
    if ACTIVE.load(Ordering::Acquire) {
        EXPECT_WAKE.store(WAKES.load(Ordering::Acquire) + 1, Ordering::Release);
        super::WORK.publish(scheduler::current_thread_id().expect("command waiter"));
    }
}

pub(super) fn wait_for_command_wake() {
    let expected = EXPECT_WAKE.load(Ordering::Acquire);
    while ACTIVE.load(Ordering::Acquire) && WAKES.load(Ordering::Acquire) < expected {
        core::hint::spin_loop();
    }
}

/// First serial host-key packet, before its first synchronous command.
pub(super) fn begin(reports: usize) {
    if STARTED.swap(true, Ordering::AcqRel) {
        return;
    }
    assert_eq!(
        reports, 2,
        "softirq wake test needs one press/release packet"
    );
    let online = crate::arch_impl::x86_64::smp::cpus_online() as usize;
    assert!(
        online >= 2,
        "softirq wake test needs a separate device-IRQ CPU"
    );
    // ap_start routes device interrupts to the last online CPU. note_irq
    // verifies that the actual command interrupts take this forced path.
    let cpu = online - 1;
    CPU.store(cpu, Ordering::Relaxed);
    let tid = scheduler::current_thread_id().expect("xHCI worker identity");
    let old_pin =
        scheduler::with_thread_mut(tid, |thread| thread.cpu_affinity).expect("xHCI worker row");
    assert!(scheduler::set_user_affinity(
        tid,
        Some(CpuPin::user_affinity(0, 1))
    ));
    // Keep the command producer off the CPU occupied by the forcing thread.
    let deadline = now() + 5 * SECOND;
    while per_cpu::cpu_id() != 0 {
        assert!(
            now() < deadline,
            "xHCI worker did not move for forcing test"
        );
        scheduler::yield_current();
    }
    let replayed = host_keys::REPLAYED.load(Ordering::Acquire);
    let dispatched = host_keys::DISPATCHED.load(Ordering::Acquire);
    let changed = host_keys::CHANGED.load(Ordering::Acquire);
    kthread::kthread_run_on_cpu(move || {
        assert_eq!(per_cpu::preempt_count(), 0, "forcing thread entry context");
        per_cpu::preempt_disable();
        unsafe { crate::arch_enable_interrupts() };
        ACTIVE.store(true, Ordering::Release);
        let start = now();
        READY.store(true, Ordering::Release);
        let mut release_at = start + 1_000_000;
        while host_keys::DISPATCHED.load(Ordering::Acquire) < dispatched + 2
            && now().saturating_sub(start) < WINDOW_NS
        {
            if now() >= release_at {
                // No lock is held. The callback may run only after this
                // outermost enable; neither the ISR nor a timer gets a cleared
                // PREEMPT window in which to rescue the old implementation.
                crate::arch_without_interrupts(|| {
                    per_cpu::preempt_enable();
                    per_cpu::preempt_disable();
                });
                release_at = now() + 1_000_000;
            }
            core::hint::spin_loop();
        }
        // Snapshot before releasing the forcing window, so a cleanup IRQ
        // cannot manufacture successful replay evidence.
        let elapsed = now().saturating_sub(start);
        let replay_delta = host_keys::REPLAYED.load(Ordering::Acquire) - replayed;
        let dispatch_delta = host_keys::DISPATCHED.load(Ordering::Acquire) - dispatched;
        let changed_delta = host_keys::CHANGED.load(Ordering::Acquire) - changed;
        let max_ns = host_keys::REPLAY_MAX_NS.load(Ordering::Acquire);
        let irqs = IRQS.load(Ordering::Acquire);
        let wakes = WAKES.load(Ordering::Acquire);
        let unsafe_wakes = UNSAFE_WAKES.load(Ordering::Acquire);
        ACTIVE.store(false, Ordering::Release);
        per_cpu::preempt_enable();
        assert!(scheduler::set_user_affinity(tid, old_pin));
        crate::serial_println!(
            "SOFTIRQ_HOSTKEY cpu={} irqs={} wakes={} replayed={} dispatched={} changed={} elapsed_ns={} HOSTKEY_REPLAY_MAX_NS={} unsafe_wakes={}",
            cpu, irqs, wakes, replay_delta, dispatch_delta, changed_delta, elapsed, max_ns, unsafe_wakes
        );
        assert!(irqs >= 4, "command IRQs did not cross the forced PREEMPT sections");
        assert!(wakes >= 4, "command softirq wakes were stranded");
        assert_eq!(unsafe_wakes, 0, "softirq callback ran over disabled preemption");
        assert_eq!((replay_delta, dispatch_delta, changed_delta), (2, 2, 0),
            "forced host-key press/release did not dispatch unchanged");
        assert!(max_ns > 0 && max_ns < LIMIT_NS && elapsed < LIMIT_NS,
            "forced host-key replay exceeded 100 ms");
        crate::serial_println!("SOFTIRQ_HOSTKEY PASS");
    }, "ksoftirqcheck", cpu).expect("spawn softirq host-key forcing thread");
    let deadline = now() + 5 * SECOND;
    while !READY.load(Ordering::Acquire) {
        assert!(now() < deadline, "softirq forcing thread did not start");
        scheduler::yield_current();
    }
}

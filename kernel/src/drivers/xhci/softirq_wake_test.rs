//! Force the first host-key press/release's real command IRQs across PREEMPT.
//!
//! The xHCI worker moves to the device-IRQ CPU for this packet. Each command
//! holds PREEMPT from before its doorbell until its deferred callback runs.
//! The command wait releases/reacquires PREEMPT with IRQs masked at 1 ms
//! intervals, at a lock-free boundary. The old irq_exit therefore cannot
//! observe a cleared PREEMPT count and rescue its stranded IrqPoll callback.
//! This needs no extra thread to start during the measured replay.

use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use super::{host_keys, now, SECOND};
use crate::{per_cpu, task::{scheduler, thread::CpuPin}};

const LIMIT_NS: u64 = 100_000_000;
const WINDOW_NS: u64 = 200_000_000;
static STARTED: AtomicBool = AtomicBool::new(false);
static ACTIVE: AtomicBool = AtomicBool::new(false);
static HELD: AtomicBool = AtomicBool::new(false);
static CPU: AtomicUsize = AtomicUsize::new(usize::MAX);
static START_NS: AtomicU64 = AtomicU64::new(0);
static REPLAYED_AT: AtomicU64 = AtomicU64::new(0);
static DISPATCHED_AT: AtomicU64 = AtomicU64::new(0);
static CHANGED_AT: AtomicU64 = AtomicU64::new(0);
static OLD_PIN: spin::Once<Option<CpuPin>> = spin::Once::new();
static IRQS: AtomicU64 = AtomicU64::new(0);
static WAKES: AtomicU64 = AtomicU64::new(0);
static EXPECT_WAKE: AtomicU64 = AtomicU64::new(0);
static UNSAFE_WAKES: AtomicU64 = AtomicU64::new(0);

/// ISR evidence only: no output, allocation or lock.
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

/// Enter PREEMPT before ringing the actual command doorbell.
pub(super) fn publish_command_waiter() {
    if ACTIVE.load(Ordering::Acquire) {
        assert_eq!(per_cpu::cpu_id(), CPU.load(Ordering::Relaxed), "forcing command CPU");
        assert_eq!(per_cpu::preempt_count(), 0, "forcing command entry context");
        crate::arch_without_interrupts(|| {
            EXPECT_WAKE.store(WAKES.load(Ordering::Acquire) + 1, Ordering::Release);
            super::WORK.publish(scheduler::current_thread_id().expect("command waiter"));
            per_cpu::preempt_disable();
            assert!(!HELD.swap(true, Ordering::Release), "nested forcing command");
        });
    }
}

pub(super) fn wait_for_command_wake() {
    if !HELD.load(Ordering::Acquire) {
        return;
    }
    let expected = EXPECT_WAKE.load(Ordering::Acquire);
    let mut release_at = now() + 1_000_000;
    while WAKES.load(Ordering::Acquire) < expected {
        if now().saturating_sub(START_NS.load(Ordering::Relaxed)) >= WINDOW_NS {
            finish();
        }
        if now() >= release_at {
            // The command wait owns no lock. IRQs cannot observe the gap;
            // only preempt_enable's dispatch can consume the pending work.
            crate::arch_without_interrupts(|| {
                per_cpu::preempt_enable();
                per_cpu::preempt_disable();
            });
            release_at = now() + 1_000_000;
        }
        core::hint::spin_loop();
    }
    HELD.store(false, Ordering::Release);
    per_cpu::preempt_enable();
}

/// First serial packet, before its first synchronous command.
pub(super) fn begin(reports: usize) {
    if STARTED.swap(true, Ordering::AcqRel) {
        return;
    }
    assert_eq!(reports, 2, "softirq wake test needs one press/release packet");
    let cpu = crate::arch_impl::x86_64::smp::cpus_online() as usize - 1;
    CPU.store(cpu, Ordering::Relaxed);
    let tid = scheduler::current_thread_id().expect("xHCI worker identity");
    let old_pin = scheduler::with_thread_mut(tid, |thread| thread.cpu_affinity)
        .expect("xHCI worker row");
    OLD_PIN.call_once(|| old_pin);
    assert!(scheduler::set_user_affinity(tid, Some(CpuPin::per_cpu_worker(cpu))));
    let deadline = now() + 5 * SECOND;
    while per_cpu::cpu_id() != cpu {
        assert!(now() < deadline, "xHCI worker did not move for forcing test");
        scheduler::yield_current();
    }
    assert_eq!(per_cpu::preempt_count(), 0, "forcing replay entry context");
    REPLAYED_AT.store(host_keys::REPLAYED.load(Ordering::Acquire), Ordering::Relaxed);
    DISPATCHED_AT.store(host_keys::DISPATCHED.load(Ordering::Acquire), Ordering::Relaxed);
    CHANGED_AT.store(host_keys::CHANGED.load(Ordering::Acquire), Ordering::Relaxed);
    START_NS.store(now(), Ordering::Relaxed);
    ACTIVE.store(true, Ordering::Release);
}

/// Called after ordinary HID dispatch, outside its locks and after rearming.
pub(super) fn after_dispatch() {
    if ACTIVE.load(Ordering::Acquire)
        && host_keys::DISPATCHED.load(Ordering::Acquire) >= DISPATCHED_AT.load(Ordering::Relaxed) + 2
    {
        finish();
    }
}

fn finish() {
    // Capture before cleanup can unmask an IRQ and rescue a failed replay.
    let elapsed = now().saturating_sub(START_NS.load(Ordering::Relaxed));
    let replayed = host_keys::REPLAYED.load(Ordering::Acquire) - REPLAYED_AT.load(Ordering::Relaxed);
    let dispatched = host_keys::DISPATCHED.load(Ordering::Acquire) - DISPATCHED_AT.load(Ordering::Relaxed);
    let changed = host_keys::CHANGED.load(Ordering::Acquire) - CHANGED_AT.load(Ordering::Relaxed);
    let max_ns = host_keys::REPLAY_MAX_NS.load(Ordering::Acquire);
    let irqs = IRQS.load(Ordering::Acquire);
    let wakes = WAKES.load(Ordering::Acquire);
    let unsafe_wakes = UNSAFE_WAKES.load(Ordering::Acquire);
    ACTIVE.store(false, Ordering::Release);
    if HELD.swap(false, Ordering::AcqRel) {
        per_cpu::preempt_enable();
    }
    let tid = scheduler::current_thread_id().expect("forcing worker identity");
    assert!(scheduler::set_user_affinity(tid, *OLD_PIN.get().expect("saved worker pin")));
    crate::serial_println!(
        "SOFTIRQ_HOSTKEY cpu={} irqs={} wakes={} replayed={} dispatched={} changed={} elapsed_ns={} HOSTKEY_REPLAY_MAX_NS={} unsafe_wakes={}",
        CPU.load(Ordering::Relaxed), irqs, wakes, replayed, dispatched, changed, elapsed, max_ns, unsafe_wakes
    );
    assert!(wakes >= 4, "command softirq wakes were stranded");
    assert!(irqs >= 4, "command IRQs did not cross the forced PREEMPT sections");
    assert_eq!(unsafe_wakes, 0, "softirq callback ran over disabled preemption");
    assert_eq!((replayed, dispatched, changed), (2, 2, 0),
        "forced host-key press/release did not dispatch unchanged");
    assert!(max_ns > 0 && max_ns < LIMIT_NS && elapsed < LIMIT_NS,
        "forced host-key replay exceeded 100 ms");
    crate::serial_println!("SOFTIRQ_HOSTKEY PASS");
}

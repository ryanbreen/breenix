//! Two pinned CPUs drive the UART ownership path; the host scores the bytes.
use crate::test_framework::registry::TestResult;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
static ROUND: [AtomicUsize; 2] = [const { AtomicUsize::new(0) }; 2];
static FAILED: AtomicBool = AtomicBool::new(false);
static DONE: AtomicBool = AtomicBool::new(false);

fn now() -> u64 {
    let (s, ns) = crate::time::get_monotonic_time_ns();
    s.saturating_mul(1_000_000_000).saturating_add(ns)
}
fn writer(index: usize) {
    let cpu = crate::arch_impl::aarch64::percpu::Aarch64PerCpu::cpu_id() as usize;
    for seq in 0..200 {
        ROUND[index].store(seq + 1, Ordering::Release);
        let deadline = now().saturating_add(2_000_000_000);
        while ROUND[1 - index].load(Ordering::Acquire) < seq + 1 {
            if now() >= deadline {
                FAILED.store(true, Ordering::Release);
                return;
            }
            core::hint::spin_loop();
        }
        let mut line = crate::serial_line::Line::new();
        line.text("[SERIAL_INTERLEAVE:cpu=");
        line.dec(cpu as u64);
        line.text(":seq=");
        line.dec(seq as u64);
        line.text(":payload=");
        for _ in 0..768 {
            line.char(b'A' + cpu as u8);
        }
        line.text("]\n");
        // Bound outstanding work to one line per CPU. Otherwise a producer
        // that stages faster than the UART drains would test ring capacity.
        let deadline = now().saturating_add(2_000_000_000);
        while crate::serial_line::ORACLE_COMPLETED[cpu].load(Ordering::Acquire) < seq + 1 {
            if now() >= deadline {
                FAILED.store(true, Ordering::Release);
                return;
            }
            crate::serial_line::drain();
            core::hint::spin_loop();
        }
    }
}

pub fn run() -> TestResult {
    use crate::task::{kthread, scheduler};
    crate::arch_without_interrupts(crate::per_cpu::preempt_disable);
    let Some(peer) = scheduler::live_peer_cpu_for_test() else {
        crate::per_cpu::preempt_enable();
        return TestResult::Fail("serial interleave oracle needs a peer CPU");
    };
    let drops = crate::serial_line::DROPPED.load(Ordering::Acquire);
    let handle = kthread::kthread_run_on_cpu_for_test(
        || {
            crate::arch_without_interrupts(crate::per_cpu::preempt_disable);
            writer(1);
            crate::per_cpu::preempt_enable();
            if let Some(tid) = scheduler::current_thread_id() {
                scheduler::release_cpu_affine_thread_for_test(tid);
            }
            DONE.store(true, Ordering::Release);
        },
        "uart-oracle",
        peer,
    );
    let Ok(handle) = handle else {
        crate::per_cpu::preempt_enable();
        return TestResult::Fail("serial interleave peer creation failed");
    };
    writer(0);
    crate::per_cpu::preempt_enable();
    let deadline = now().saturating_add(3_000_000_000);
    while !DONE.load(Ordering::Acquire) && now() < deadline {
        core::hint::spin_loop();
    }
    if !DONE.load(Ordering::Acquire) {
        return TestResult::Fail("serial interleave peer timeout");
    }
    if kthread::kthread_join(&handle).is_err() {
        return TestResult::Fail("serial interleave peer join failed");
    }
    crate::serial_line::drain();
    if FAILED.load(Ordering::Acquire)
        || crate::serial_line::DROPPED.load(Ordering::Acquire) != drops
    {
        return TestResult::Fail("serial interleave rendezvous or staging overflow");
    }
    TestResult::Pass
}

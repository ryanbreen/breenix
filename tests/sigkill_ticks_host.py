#!/usr/bin/env python3
"""Exercise the production kick publication with simulated per-CPU timer delivery."""
from pathlib import Path
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
source = (ROOT / "kernel/src/tracing/providers/teardown.rs").read_text()
slot = source[source.index("pub(crate) struct KickSlot"):source.index("pub(crate) static EXIT_KICK_SLOTS")]
constants = "\n".join(line for line in source.splitlines() if line.startswith((
    "pub(crate) const EXIT_KICK_LOCK:", "pub(crate) const EXIT_KICK_OBSERVED_BIT:", "pub(crate) const KICK_RESERVE_ATTEMPTS:")))
user_source = (ROOT / "userspace/programs/src/sigkill_teardown_test.rs").read_text()
assertion = user_source[user_source.index("fn assert_kill_evidence"):user_source.index("fn wait_killed")]
harness = r'''
use core::sync::atomic::{AtomicU64, Ordering};
mod task { pub mod scheduler {
    pub const MAX_CPUS: usize = 8;
    std::thread_local! { pub static CPU: core::cell::Cell<usize> = const { core::cell::Cell::new(0) }; }
    pub fn current_cpu() -> usize { CPU.with(|cpu| cpu.get()) }
} }
mod tracing { pub mod providers { pub mod counters {
    use core::sync::atomic::{AtomicU64, Ordering};
    pub struct Counter([AtomicU64; 8]);
    impl Counter {
        pub fn get_cpu(&self, cpu: usize) -> u64 { self.0[cpu].load(Ordering::Relaxed) }
        pub fn set(&self, cpu: usize, ticks: u64) { self.0[cpu].store(ticks, Ordering::Relaxed); }
    }
    pub static TIMER_TICK_TOTAL: Counter = Counter([const { AtomicU64::new(0) }; 8]);
} } }
fn cpu(cpu: usize) { task::scheduler::CPU.with(|id| id.set(cpu)); }
#[test]
fn guest_ticks_follow_observer_and_publication_generation() {
    use tracing::providers::counters::TIMER_TICK_TOTAL as timer;
    let slot = KickSlot::new();
    cpu(0);
    timer.set(0, 100);
    timer.set(1, 200);
    timer.set(2, 300);
    assert!(matches!(slot.publish(7, 10), KickPublishResult::Published { generation: 1, displaced: false }));
    assert_eq!(slot.generation(), 1);
    assert!(slot.observe_from_peer(7).is_none(), "sender cannot supply peer evidence");
    cpu(1);
    assert!(slot.observe_from_peer(71).is_none(), "another PID cannot claim this generation");
    // Other CPUs' timer delivery and arbitrarily large CNTVCT/TSC advance
    // cannot spend the observer's budget.
    timer.set(0, 10_000);
    timer.set(2, 20_000);
    let counter_now = 14_000_010u64;
    let (observation, ticks) = slot.observe_from_peer(7).unwrap();
    assert_eq!(observation, KickObservation { generation: 1, pid: 7, at: 10 });
    assert_eq!(counter_now - observation.at, 14_000_000);
    assert_eq!(ticks, 0);
    assert!(slot.is_observed_for(7));
    assert!(slot.observe_from_peer(7).is_none(), "observation remains exactly once");

    cpu(0);
    assert!(matches!(slot.publish(71, counter_now), KickPublishResult::Published { generation: 2, displaced: false }));
    assert!(slot.superseded_for(7, 1));
    cpu(1);
    timer.set(1, 201);
    let (observation, ticks) = slot.observe_from_peer(71).unwrap();
    assert_eq!(observation.generation, 2);
    assert_eq!(ticks, 1, "one delivered observer tick must reach the strict failure boundary");

    // A reserved generation is invisible and cannot be overwritten by a
    // losing publisher. Its eventual commit must replace the tick baselines.
    cpu(0);
    let reservation = slot.reserve().unwrap();
    assert_eq!(slot.publish(135, 123), KickPublishResult::ReservationLost);
    cpu(2);
    assert!(slot.observe_from_peer(135).is_none());
    timer.set(2, u64::MAX);
    cpu(0);
    slot.commit(reservation, 135, 456);
    cpu(2);
    timer.set(2, 0);
    let (_, ticks) = slot.observe_from_peer(135).unwrap();
    assert_eq!(ticks, 1, "delivered tick arithmetic must wrap");
    assert!(!slot.is_observed_for(71));

    cpu(0);
    slot.publish(199, 789);
    cpu(1);
    timer.set(1, 204);
    assert_eq!(slot.observe_from_peer(199).unwrap().1, 3);
}
#[derive(Clone, Copy)]
struct Evidence {
    online_cpus: u64, defer: u64, reclaim: u64, quarantine: u64,
    sgi_sent: u64, kick_observed: u64, kick_ticks: u64, kick_cpu: u64,
    kick_interval: u64, counter_ticks_per_ms: u64, masked_frames_walked: u64, report: u64,
}
fn fail(message: &str) -> ! { panic!("{}", message) }
fn evidence() -> Evidence {
    Evidence { online_cpus: 4, defer: 1, reclaim: 1, quarantine: 1,
        sgi_sent: 1, kick_observed: 1, kick_ticks: 0, kick_cpu: 1,
        kick_interval: 14_000_000, counter_ticks_per_ms: 1_000_000,
        masked_frames_walked: 0, report: 1 }
}
#[test]
fn zero_guest_ticks_pass_even_when_counter_exceeds_one_ms() {
    assert_kill_evidence(7, evidence());
}
#[test]
fn one_or_more_guest_ticks_fail_the_unchanged_strict_bound() {
    for ticks in [1, 2, u64::MAX] {
        let current = Evidence { kick_ticks: ticks, ..evidence() };
        assert!(std::panic::catch_unwind(|| assert_kill_evidence(7, current)).is_err());
    }
}
#[test]
fn zero_ticks_without_a_paired_observation_cannot_pass() {
    let current = Evidence { kick_observed: 0, ..evidence() };
    assert!(std::panic::catch_unwind(|| assert_kill_evidence(7, current)).is_err());
}
'''
with tempfile.TemporaryDirectory(prefix="breenix-sigkill-ticks-") as tmp:
    test = Path(tmp) / "tests.rs"
    test.write_text(harness + constants + "\n" + slot + assertion)
    binary = Path(tmp) / "tests"
    subprocess.run((["taskpolicy", "-b"] if sys.platform == "darwin" else []) + ["rustc", "--edition=2021", "--test", "-Dwarnings",
                    str(test), "-o", str(binary)], cwd=ROOT, check=True)
    subprocess.run([str(binary)], check=True)

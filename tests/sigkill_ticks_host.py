#!/usr/bin/env python3
"""Exercise the kick window publication, the userspace verdict and its scorer.

The kernel's KickSlot and the userspace assertion are compiled from their
production source; the scorer is imported and fed the assertion's own lines.
"""
from pathlib import Path
import importlib.util
import re
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
source = (ROOT / "kernel/src/tracing/providers/teardown.rs").read_text()
slot = source[source.index("pub(crate) struct KickSlot"):source.index("pub(crate) static EXIT_KICK_SLOTS")]
constants = "\n".join(line for line in source.splitlines() if line.startswith((
    "pub(crate) const EXIT_KICK_LOCK:", "pub(crate) const EXIT_KICK_OBSERVED_BIT:", "pub(crate) const KICK_RESERVE_ATTEMPTS:")))
user_source = (ROOT / "userspace/programs/src/sigkill_teardown_test.rs").read_text()
assertion = user_source[user_source.index("/// Counter time per delivered timer tick above which"):user_source.index("fn wait_killed")]

# The userspace threshold and the scorer's are SOFTIRQ_DEFERRAL_ORACLE's.
softirq = (ROOT / "kernel/src/task/softirq_tests.rs").read_text()
wait_ticks = int(re.search(r"const WAIT_TICKS: u64 = ([\d_]+);", softirq).group(1).replace("_", ""))
wall_limit = int(re.search(r"const WALL_LIMIT_NS: u64 = ([\d_]+);", softirq).group(1).replace("_", ""))
assert "const STARVED_NS_PER_TICK: u128 = 15_000_000_000 / 250;" in assertion
assert (wall_limit, wait_ticks) == (15_000_000_000, 250)
spec = importlib.util.spec_from_file_location("score", ROOT / "scripts/score-sigkill-teardown.py")
scorer = importlib.util.module_from_spec(spec)
spec.loader.exec_module(scorer)
assert scorer.STARVED_NS_PER_TICK == wall_limit // wait_ticks

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
fn window_counts_observer_and_all_cpu_ticks_per_generation() {
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
    timer.set(0, 103);
    timer.set(2, 305);
    let (observation, window) = slot.observe_from_peer(7).unwrap();
    assert_eq!(observation, KickObservation { generation: 1, pid: 7, at: 10 });
    assert_eq!(window, KickWindow { observer_ticks: 0, all_ticks: 8, sender_cpu: 0 });
    assert!(slot.is_observed_for(7));
    assert!(slot.observe_from_peer(7).is_none(), "observation remains exactly once");

    cpu(2);
    assert!(matches!(slot.publish(71, 20), KickPublishResult::Published { generation: 2, displaced: false }));
    assert!(slot.superseded_for(7, 1));
    cpu(1);
    timer.set(1, 201);
    let (observation, window) = slot.observe_from_peer(71).unwrap();
    assert_eq!(observation.generation, 2);
    assert_eq!(window, KickWindow { observer_ticks: 1, all_ticks: 1, sender_cpu: 2 });

    // A reserved generation is invisible; its commit carries the baseline
    // taken before the reservation, and delivered-tick arithmetic wraps.
    cpu(0);
    timer.set(3, u64::MAX);
    let baseline = tick_baseline();
    let reservation = slot.reserve().unwrap();
    assert_eq!(slot.publish(135, 123), KickPublishResult::ReservationLost);
    cpu(3);
    assert!(slot.observe_from_peer(135).is_none());
    timer.set(0, 1_000);
    cpu(0);
    slot.commit_with_ticks(reservation, 135, 456, baseline);
    cpu(3);
    timer.set(3, 0);
    let (_, window) = slot.observe_from_peer(135).unwrap();
    assert_eq!(window.observer_ticks, 1, "delivered tick arithmetic must wrap");
    assert_eq!(window.all_ticks, 1 + 897, "ticks after the baseline count even when taken before commit");
    assert!(!slot.is_observed_for(71));
}
#[derive(Clone, Copy, Debug)]
struct Evidence {
    online_cpus: u64, defer: u64, reclaim: u64, quarantine: u64,
    sgi_sent: u64, kick_observed: u64, kick_interval: u64, kick_ticks: u64,
    kick_all_ticks: u64, kick_cpu: u64, kick_sender_cpu: u64,
    counter_ticks_per_ms: u64, tick_period: u64, masked_frames_walked: u64, report: u64,
}
fn fail(message: &str) -> ! { panic!("{}", message) }
// ARM64 at 24 MHz: one 1 ms tick is 24,000 counter ticks.
fn evidence(kick_interval: u64, kick_ticks: u64, kick_all_ticks: u64) -> Evidence {
    Evidence { online_cpus: 4, defer: 1, reclaim: 1, quarantine: 1,
        sgi_sent: 1, kick_observed: 1, kick_interval, kick_ticks, kick_all_ticks,
        kick_cpu: 1, kick_sender_cpu: 0, counter_ticks_per_ms: 24_000, tick_period: 24_000,
        masked_frames_walked: 0, report: 1 }
}
fn fails(current: Evidence) -> bool {
    std::panic::catch_unwind(|| assert_kill_evidence(7, current)).is_err()
}
fn starved_after(current: Evidence) -> bool {
    let before = STARVED_WINDOWS.load(Ordering::Relaxed);
    assert_kill_evidence(7, current);
    STARVED_WINDOWS.load(Ordering::Relaxed) == before + 1
}
#[test]
fn under_one_tick_passes_even_across_a_tick_boundary() {
    for (interval, observer, all) in [(426, 0, 0), (7_200, 1, 4), (23_999, 1, 4)] {
        let current = evidence(interval, observer, all);
        assert_eq!(kick_verdict(&current), KickVerdict::Pass, "{current:?}");
        assert!(!fails(current));
    }
}
#[test]
fn one_tick_or_more_unstarved_fails() {
    for (interval, observer, all) in [(24_000, 1, 4), (28_800, 1, 4), (24_000 * 50, 1, 200), (24_000 * 59, 1, 4)] {
        let current = evidence(interval, observer, all);
        assert_eq!(kick_verdict(&current), KickVerdict::Fail, "{current:?}");
        assert!(fails(current));
    }
}
#[test]
fn starved_window_is_its_own_verdict() {
    for (interval, observer, all) in [(24_000 * 14, 0, 0), (24_000 * 70, 1, 4), (24_000 * 61, 1, 4)] {
        let current = evidence(interval, observer, all);
        assert_eq!(kick_verdict(&current), KickVerdict::Starved, "{current:?}");
        assert!(starved_after(current));
    }
}
#[test]
fn masked_or_dead_observer_timer_while_others_tick_fails() {
    // 5 ms with nothing delivered on the observer, 15 ticks on the other three CPUs.
    for (interval, all) in [(24_000 * 5, 15), (24_000 * 14, 3)] {
        let current = evidence(interval, 0, all);
        assert_eq!(kick_verdict(&current), KickVerdict::Fail, "{current:?}");
        assert!(fails(current));
    }
}
#[test]
fn measurement_and_attribution_preconditions_fail() {
    assert!(fails(evidence(0, 0, 0)), "a peer kick with a zero interval");
    assert!(fails(Evidence { counter_ticks_per_ms: 0, ..evidence(426, 0, 0) }));
    assert!(fails(Evidence { tick_period: 0, ..evidence(426, 0, 0) }));
    assert!(fails(Evidence { kick_cpu: 0, ..evidence(426, 0, 0) }), "observed on the sending CPU");
    assert!(fails(Evidence { kick_observed: 0, ..evidence(426, 0, 0) }));
    let single = Evidence { online_cpus: 1, sgi_sent: 0, kick_observed: 0, ..evidence(0, 0, 0) };
    assert!(!fails(single));
    assert!(fails(Evidence { kick_interval: 1, ..single }));
}
#[test]
fn print_windows_for_the_scorer() {
    assert_kill_evidence(7, evidence(426, 0, 0));
    assert_kill_evidence(8, evidence(24_000 * 14, 0, 0));
}
'''

with tempfile.TemporaryDirectory(prefix="breenix-sigkill-ticks-") as tmp:
    test = Path(tmp) / "tests.rs"
    test.write_text(harness + constants + "\n" + slot + assertion)
    binary = Path(tmp) / "tests"
    subprocess.run((["taskpolicy", "-b"] if sys.platform == "darwin" else []) + [
        "rustc", "--edition=2021", "--test", "-Dwarnings", "--cfg", 'feature="testing"',
        str(test), "-o", str(binary)], cwd=ROOT, check=True)
    subprocess.run([str(binary), "--test-threads=1"], check=True)
    printed = subprocess.run([str(binary), "--nocapture", "--exact", "print_windows_for_the_scorer"],
                             check=True, capture_output=True, text=True).stdout

windows = [line for line in printed.splitlines() if line.startswith("SIGKILL_TEARDOWN_TEST: pid=")]
assert len(windows) == 2 and windows[0].endswith("verdict=pass") and windows[1].endswith("verdict=STARVED"), printed
assert " observer_ms_per_tick=unbounded " in windows[1] and " interval_us=14000 " in windows[1], windows[1]
run_format = "SIGKILL_TEARDOWN_TEST: STARVED windows={starved}: host-starved, not evidence; rerun the boot"
assert run_format in user_source
run_starved = run_format.replace("{starved}", "1")
assert scorer.score(windows[0] + "\nSIGKILL_TEARDOWN_TEST_PASSED")[0] == 0
assert scorer.score("\n".join(windows) + "\n" + run_starved)[0] == 2
assert scorer.score("\n".join(windows) + "\nSIGKILL_TEARDOWN_TEST_PASSED")[0] == 1, "starved window without run verdict"
assert scorer.score("\n".join(windows) + "\n" + run_starved + "\nSIGKILL_TEARDOWN_TEST_PASSED")[0] == 1
assert scorer.score(windows[0] + "\n" + run_starved)[0] == 1, "run verdict without a starved window"
assert scorer.score(windows[1].replace("verdict=STARVED", "verdict=pass"))[0] == 1, "forged pass"
assert scorer.score(windows[0].replace("verdict=pass", "verdict=STARVED") + "\n" + run_starved)[0] == 1, "forged starved"
masked = windows[1].replace(" all_ticks=0 ", " all_ticks=42 ").replace("verdict=STARVED", "verdict=fail")
assert scorer.score(masked)[0] == 1
assert scorer.score(windows[1] + "\n" + run_starved + "\nSIGKILL_TEARDOWN_TEST: FAIL: x")[0] == 1
assert scorer.score(windows[0].replace("sender_cpu=0", "sender_cpu=1"))[0] == 1, "observer is sender"
assert scorer.score(windows[0].replace("starved_ms_per_tick=60", "starved_ms_per_tick=10"))[0] == 1
assert scorer.score("")[0] == 0
# Kernel output on the shared ARM64 serial can split a window line.
cut = windows[1].index(" all_ms_per_tick=")
split = windows[1][:cut] + "\ncreate_user_process: ENTRY - Creating 'next' (ARM64)\n" + windows[1][cut:]
assert scorer.score(split + "\n" + run_starved)[0] == 2, "a rejoined window is read"
assert scorer.score(split.replace(" all_ticks=0 ", " all_ticks=42 ") + "\n" + run_starved)[0] == 1
lost = windows[1][:cut] + "\n" + "\n".join(["kernel line"] * 6) + "\n" + windows[1][cut:]
assert scorer.score(lost + "\n" + run_starved)[0] == 1, "a starved run needs every window read"
assert scorer.score(windows[0][:cut] + "\n" + "\n".join(["kernel line"] * 6) + "\nSIGKILL_TEARDOWN_TEST_PASSED")[0] == 0
print("sigkill_ticks_host: ok")

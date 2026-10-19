#!/usr/bin/env python3
"""Exercise the kick window publication, the userspace verdict and its scorer.

The kernel's KickSlot and the userspace assertion are compiled from their
production source; the scorer is imported and fed the assertion's own lines.
"""
from pathlib import Path
import importlib.util
import json
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
} }
    pub static NOW: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
    pub fn trace_timestamp() -> u64 { NOW.load(core::sync::atomic::Ordering::Relaxed) }
}
#[cfg(target_arch = "aarch64")]
mod arch_impl { pub mod aarch64 {
    pub mod timer_interrupt {
        pub static TICKS_PER_INTERRUPT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(24_000);
    }
    pub mod irq_mask_account {
        use core::sync::atomic::{AtomicU64, Ordering};
        pub static SINCE: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];
        pub fn masked_since(cpu: usize) -> u64 { SINCE[cpu].load(Ordering::Relaxed) }
        pub static HELD: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];
        pub static RANGE: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];
        pub fn held_within(cpu: usize, from: u64, to: u64) -> u64 {
            RANGE[0].store(from, Ordering::Relaxed);
            RANGE[1].store(to, Ordering::Relaxed);
            HELD[cpu].load(Ordering::Relaxed)
        }
    }
} }
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
    assert_eq!(window, KickWindow { observer_ticks: 0, all_ticks: 8, masked_cpus: 0, held: 0, held_cpu: 0, sender_cpu: 0 });
    assert!(slot.is_observed_for(7));
    assert!(slot.observe_from_peer(7).is_none(), "observation remains exactly once");

    cpu(2);
    assert!(matches!(slot.publish(71, 20), KickPublishResult::Published { generation: 2, displaced: false }));
    assert!(slot.superseded_for(7, 1));
    cpu(1);
    timer.set(1, 201);
    let (observation, window) = slot.observe_from_peer(71).unwrap();
    assert_eq!(observation.generation, 2);
    assert_eq!(window, KickWindow { observer_ticks: 1, all_ticks: 1, masked_cpus: 0, held: 0, held_cpu: 0, sender_cpu: 2 });

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
#[cfg(target_arch = "aarch64")]
#[test]
fn window_names_cpus_masked_from_its_first_tick_on() {
    use arch_impl::aarch64::irq_mask_account::SINCE;
    let slot = KickSlot::new();
    cpu(0);
    assert!(matches!(slot.publish(9, 100_000), KickPublishResult::Published { .. }));
    // CPU 1 masked before the window, CPU 2 exactly one tick into it: neither
    // can have taken a tick. CPU 3 masked after its first tick was due, and
    // CPU 4 has interrupts enabled.
    SINCE[1].store(90_000, Ordering::Relaxed);
    SINCE[2].store(124_000, Ordering::Relaxed);
    SINCE[3].store(124_001, Ordering::Relaxed);
    cpu(5);
    let (_, window) = slot.observe_from_peer(9).unwrap();
    assert_eq!(window.masked_cpus, 0b110);
    for since in &SINCE {
        since.store(0, Ordering::Relaxed);
    }
}
#[cfg(target_arch = "aarch64")]
#[test]
fn window_names_the_cpu_held_longest_over_the_observed_window() {
    use arch_impl::aarch64::irq_mask_account::{HELD, RANGE};
    let slot = KickSlot::new();
    cpu(0);
    assert!(matches!(slot.publish(11, 100_000), KickPublishResult::Published { .. }));
    HELD[1].store(5_000, Ordering::Relaxed);
    HELD[3].store(13_000, Ordering::Relaxed);
    HELD[6].store(12_999, Ordering::Relaxed);
    tracing::NOW.store(137_000, Ordering::Relaxed);
    cpu(2);
    let (_, window) = slot.observe_from_peer(11).unwrap();
    assert_eq!((window.held, window.held_cpu), (13_000, 3));
    assert_eq!([RANGE[0].load(Ordering::Relaxed), RANGE[1].load(Ordering::Relaxed)], [100_000, 137_000],
               "held time is read over the window from its publication to its observation");
    for held in &HELD {
        held.store(0, Ordering::Relaxed);
    }
}
#[derive(Clone, Copy, Debug)]
struct Evidence {
    online_cpus: u64, defer: u64, reclaim: u64, quarantine: u64,
    sgi_sent: u64, kick_observed: u64, kick_interval: u64, kick_at: u64, kick_ticks: u64,
    kick_all_ticks: u64, masked_accounting: u64, kick_masked_cpus: u64, kick_held: u64, kick_held_cpu: u64,
    kick_cpu: u64, kick_sender_cpu: u64, kick_vcpu: u64, kick_sender_vcpu: u64,
    counter_ticks_per_ms: u64, tick_period: u64, masked_frames_walked: u64, report: u64,
}
fn fail(message: &str) -> ! { panic!("{}", message) }
// ARM64 at 24 MHz: one 1 ms tick is 24,000 counter ticks. The kernel
// accounts masking, so the guest itself judges a starved window.
fn evidence(kick_interval: u64, kick_ticks: u64, kick_all_ticks: u64) -> Evidence {
    Evidence { online_cpus: 4, defer: 1, reclaim: 1, quarantine: 1,
        sgi_sent: 1, kick_observed: 1, kick_interval, kick_at: 48_000_000, kick_ticks, kick_all_ticks,
        masked_accounting: 1, kick_masked_cpus: 0, kick_held: 0, kick_held_cpu: 0,
        kick_cpu: 1, kick_sender_cpu: 0, kick_vcpu: 1, kick_sender_vcpu: 0,
        counter_ticks_per_ms: 24_000, tick_period: 24_000,
        masked_frames_walked: 0, report: 1 }
}
// x86-64 at 2.4 GHz with 5 ms ticks and no masking accounting: the host's
// record judges a starved window.
fn host_evidence(kick_interval: u64, kick_ticks: u64, kick_all_ticks: u64) -> Evidence {
    Evidence { masked_accounting: 0, kick_at: 24_000_000_000, kick_vcpu: 3, kick_sender_vcpu: 2,
        counter_ticks_per_ms: 2_400_000, tick_period: 12_000_000,
        ..evidence(kick_interval, kick_ticks, kick_all_ticks) }
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
fn guest_starved_window_needs_every_cpu_tickless_and_none_masked() {
    let current = evidence(24_000 * 14, 0, 0);
    assert_eq!(kick_verdict(&current), KickVerdict::Starved(Starvation::Ticks), "{current:?}");
    assert!(starved_after(current));
    // Ticks elsewhere, or a CPU masked across the window, mean the guest
    // itself may have held the observer off.
    for current in [evidence(24_000 * 70, 1, 4), evidence(24_000 * 61, 0, 4),
                    Evidence { kick_masked_cpus: 0b100, ..evidence(24_000 * 14, 0, 0) }] {
        assert_eq!(kick_verdict(&current), KickVerdict::Fail, "{current:?}");
        assert!(fails(current));
    }
}
#[test]
fn host_starved_window_is_left_to_the_host_record() {
    for (interval, observer, all) in [(12_000_000 * 14, 0, 0), (12_000_000 * 70, 1, 4)] {
        let current = host_evidence(interval, observer, all);
        assert_eq!(kick_verdict(&current), KickVerdict::Starved(Starvation::Ticks), "{current:?}");
        assert!(starved_after(current));
    }
    let current = host_evidence(12_000_000 * 50, 1, 200);
    assert_eq!(kick_verdict(&current), KickVerdict::Fail, "{current:?}");
}
// #1737's window: 1,526 us with three ticks delivered over four CPUs and CPU 2
// masked across it, which the tick rule cannot call starved.
fn held_evidence(kick_held: u64) -> Evidence {
    Evidence { kick_held, kick_held_cpu: 3, kick_masked_cpus: 0b100, ..evidence(36_641, 2, 3) }
}
#[test]
fn held_window_is_starved_only_when_the_held_time_covers_its_excess() {
    assert_eq!(held_needed(&held_evidence(0)), 36_641 - 24_000 + 1);
    let current = held_evidence(12_642);
    assert_eq!(kick_verdict(&current), KickVerdict::Starved(Starvation::Held), "{current:?}");
    assert!(starved_after(current));
    // A CPU held off for less than the window's excess, or not at all, cannot
    // account for it: the guest's own masking or a slow kill path remains.
    for held in [0, 12_641] {
        let current = held_evidence(held);
        assert_eq!(kick_verdict(&current), KickVerdict::Fail, "{current:?}");
        assert!(fails(current));
    }
}
#[test]
fn held_window_needs_half_a_tick_however_small_its_excess() {
    let short = |kick_held| Evidence { kick_held, ..evidence(24_100, 1, 4) };
    assert_eq!(held_needed(&short(0)), 12_000);
    assert_eq!(kick_verdict(&short(11_999)), KickVerdict::Fail);
    assert!(fails(short(11_999)));
    assert_eq!(kick_verdict(&short(12_000)), KickVerdict::Starved(Starvation::Held));
    // A window under one tick passes on its own and needs no held time.
    assert_eq!(held_needed(&evidence(23_999, 1, 4)), 0);
}
#[test]
fn held_time_is_not_evidence_without_masking_accounting() {
    let current = Evidence { kick_held: 12_000_000 * 50, ..host_evidence(12_000_000 * 50, 1, 200) };
    assert_eq!(kick_verdict(&current), KickVerdict::Fail, "{current:?}");
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
    assert_kill_evidence(9, host_evidence(12_000_000 * 20, 0, 0));
    assert_kill_evidence(10, held_evidence(12_642));
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
assert len(windows) == 4 and windows[0].endswith("verdict=pass") and windows[1].endswith("verdict=STARVED"), printed
assert windows[2].endswith("starvation_check=host verdict=STARVED"), windows[2]
assert " observer_ms_per_tick=unbounded " in windows[1] and " interval_us=14000 " in windows[1], windows[1]
assert " starvation_check=guest " in windows[1] and " masked_cpus=0 " in windows[1], windows[1]
assert " starved_by=ticks " in windows[1] and " starved_by=none " in windows[0], windows[:2]
held_window = windows[3]
assert held_window.endswith(
    " masked_cpus=4 held=12642 held_cpu=3 held_needed=12642 starved_by=held starvation_check=guest verdict=STARVED"), held_window
run_format = "SIGKILL_TEARDOWN_TEST: STARVED windows={starved}: host-starved, not evidence; rerun the boot"
assert run_format in user_source
run_starved = run_format.replace("{starved}", "1")
assert scorer.score(windows[0] + "\nSIGKILL_TEARDOWN_TEST_PASSED")[0] == 0
assert scorer.score("\n".join(windows[:2]) + "\n" + run_starved)[0] == 2
assert scorer.score("\n".join(windows[:2]) + "\nSIGKILL_TEARDOWN_TEST_PASSED")[0] == 1, "starved window without run verdict"
assert scorer.score("\n".join(windows[:2]) + "\n" + run_starved + "\nSIGKILL_TEARDOWN_TEST_PASSED")[0] == 1
assert scorer.score(windows[0] + "\n" + run_starved)[0] == 1, "run verdict without a starved window"
assert scorer.score(windows[1].replace("verdict=STARVED", "verdict=pass"))[0] == 1, "forged pass"
assert scorer.score(windows[0].replace("verdict=pass", "verdict=STARVED") + "\n" + run_starved)[0] == 1, "forged starved"
masked = windows[1].replace(" all_ticks=0 ", " all_ticks=42 ").replace("verdict=STARVED", "verdict=fail")
assert scorer.score(masked)[0] == 1
for forged in (windows[1].replace(" all_ticks=0 ", " all_ticks=3 "), windows[1].replace(" masked_cpus=0 ", " masked_cpus=4 ")):
    assert scorer.score(forged + "\n" + run_starved)[0] == 1, "a guest window with ticks or a masked CPU is not starved"
assert scorer.score(windows[1] + "\n" + run_starved + "\nSIGKILL_TEARDOWN_TEST: FAIL: x")[0] == 1
assert scorer.score(windows[0].replace("sender_cpu=0", "sender_cpu=1"))[0] == 1, "observer is sender"
assert scorer.score(windows[0].replace("starved_ms_per_tick=60", "starved_ms_per_tick=10"))[0] == 1
assert scorer.score("")[0] == 0
assert scorer.score(held_window + "\n" + run_starved)[0] == 2, "a guest-held window is starved without a host record"
for forged in (re.sub(r" held_cpu=\d+ ", " held_cpu=999999 ", held_window),
               held_window.replace(" held=12642 ", " held=12641 "),
               held_window.replace(" starved_by=held ", " starved_by=ticks "),
               held_window.replace(" held_needed=12642 ", " held_needed=12000 "),
               held_window.replace(" starvation_check=guest ", " starvation_check=host ")):
    assert scorer.score(forged + "\n" + run_starved)[0] == 1, forged
assert scorer.score(held_window.replace(" held=12642 ", " held=12641 ").replace("verdict=STARVED", "verdict=fail")
                    .replace("starved_by=held", "starved_by=none"))[0] == 1
# Kernel output on the shared ARM64 serial can split a window line.
cut = windows[1].index(" all_ms_per_tick=")
split = windows[1][:cut] + "\ncreate_user_process: ENTRY - Creating 'next' (ARM64)\n" + windows[1][cut:]
assert scorer.score(split + "\n" + run_starved)[0] == 2, "a rejoined window is read"
assert scorer.score(split.replace(" all_ticks=0 ", " all_ticks=42 ") + "\n" + run_starved)[0] == 1
lost = windows[1][:cut] + "\n" + "\n".join(["kernel line"] * 6) + "\n" + windows[1][cut:]
assert scorer.score(lost + "\n" + run_starved)[0] == 1, "a starved run needs every window read"
assert scorer.score(windows[0][:cut] + "\n" + "\n".join(["kernel line"] * 6) + "\nSIGKILL_TEARDOWN_TEST_PASSED")[0] == 0

# The host-checked window (x86-64: 2.4 GHz, 5 ms ticks) opened at guest TSC
# 24e9 and lasted 100 ms. It is starved only if the host's record shows vCPUs
# 3 (observer) and 2 (sender) kept from running for at least 95 ms around it.
host_window = windows[2]
assert " kick_at=24000000000 observer_vcpu=3 sender_vcpu=2 " in host_window, host_window
start, end, step = 24_000_000_000, 24_240_000_000, 48_000_000


def record(delay_ms, steal_ticks=0, first=start - step, last=end + step, offsets=None):
    """Samples every 20 ms from guest TSC `first` to `last`, with vCPU 3's
    run delay growing by `delay_ms` and its host CPU's steal by
    `steal_ticks`, spread evenly over the samples inside the window."""
    lines = [{"kind": "start", "clk_tck": 100, "period": 0.02},
             {"kind": "guest", "pid": 77, "vcpus": {str(v): str(100 + v) for v in range(4)},
              "tsc_offsets": offsets or {str(v): -5 for v in range(4)}, "tsc_scaling": {}}]
    inside = [tsc for tsc in range(first, last + 1, step) if start < tsc <= end]
    delay = run = steal = 0
    for tsc in range(first, last + 1, step):
        if start < tsc <= end:
            delay += delay_ms * 1_000_000 // len(inside)
            steal += steal_ticks // len(inside)
        run += 1_000_000
        lines.append({"t": tsc / 2.4e9, "tsc": tsc, "steal": [0, steal, 0, 0],
                      "vcpu": {"2": [0, run, 0, "R"], "3": [delay, run, 1, "R"], "0": [0, run, 2, "S"], "1": [0, run, 3, "S"]}})
    path = Path(tmp_record.name)
    path.write_text("\n".join(json.dumps(line) for line in lines) + "\n")
    return scorer.load_host_record(str(path))


with tempfile.NamedTemporaryFile(prefix="breenix-host-stall-", suffix=".jsonl") as tmp_record:
    host_run = host_window + "\n" + run_starved
    assert scorer.score(host_run, record(150))[0] == 2, "the host held the observer off for the window"
    assert scorer.score(host_run, record(40, steal_ticks=10))[0] == 2, "steal on the observer's CPU counts"
    status, message = scorer.score(host_run, record(20))
    assert status == 1 and "needs 95000 us" in message, message
    assert scorer.score(host_run, record(150, first=start + step))[0] == 1, "no sample before the window"
    assert scorer.score(host_run, record(150, last=end - step))[0] == 1, "no sample after the window"
    assert scorer.score(host_run, record(150, offsets={"0": -5, "1": -5, "2": -5, "3": 9}))[0] == 1
    status, message = scorer.score(host_run, None)
    assert status == 1 and "no host stall record" in message, message
    assert scorer.score(host_window.replace("observer_vcpu=3", "observer_vcpu=7") + "\n" + run_starved,
                        record(150))[0] == 1, "a vCPU the record does not name"
print("sigkill_ticks_host: ok")

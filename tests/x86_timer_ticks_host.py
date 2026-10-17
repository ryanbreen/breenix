#!/usr/bin/env python3
"""Run the real x86 tick handler and timer-vector tick decision with explicit
timer, retry self-IPI and reschedule IPI deliveries."""
from pathlib import Path
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
source = (ROOT / 'kernel/src/interrupts/timer.rs').read_text()
handler = source[source.index('#[no_mangle]'):source.index('/// Reset this CPU')]
quantum = next(line for line in source.splitlines() if line.startswith('const TIME_QUANTUM:'))
scheduler_source = (ROOT / 'kernel/src/task/scheduler.rs').read_text()
decision = scheduler_source[scheduler_source.index('/// x86_64: a `RETRY_VECTOR` self-IPI this CPU sent'):
                            scheduler_source.index('/// x86_64: called first on every entry to `check_need_resched_and_switch`')]
retry = scheduler_source[scheduler_source.index('pub fn retry_after_interrupts_x86()'):
                         scheduler_source.index('/// x86_64: arrange for this CPU to pick again')]
decision = decision.replace('#[cfg(target_arch = "x86_64")]\n', '')
harness = r'''
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
static TICKS: AtomicU64 = AtomicU64::new(0);
static NOW: AtomicU64 = AtomicU64::new(1);
static LAST: AtomicU64 = AtomicU64::new(0);
static QUANTUM: AtomicU32 = AtomicU32::new(10);
static IRQ_DEPTH: AtomicU32 = AtomicU32::new(0);
static TSC: AtomicU64 = AtomicU64::new(0);
static SENT: AtomicU64 = AtomicU64::new(0);
mod time {
    pub fn timer_interrupt() {}
    pub fn get_ticks() -> u64 { super::NOW.load(super::Ordering::Relaxed) }
    pub mod tsc { pub fn read_tsc() -> u64 { crate::TSC.load(crate::Ordering::Relaxed) } }
    pub mod timer { pub fn tsc_cycles_per_tick() -> u64 { 100 } }
}
mod interrupts { pub const RETRY_VECTOR: u8 = 0x20; }
mod per_cpu {
    use super::*;
    pub fn irq_enter() { IRQ_DEPTH.fetch_add(1, Ordering::Relaxed); }
    pub fn irq_exit() { IRQ_DEPTH.fetch_sub(1, Ordering::Relaxed); }
    pub fn take_quantum_ticks(now: u64) -> u64 { now - LAST.swap(now, Ordering::Relaxed) }
    pub fn quantum() -> u32 { QUANTUM.load(Ordering::Relaxed) }
    pub fn set_quantum(value: u32) { QUANTUM.store(value, Ordering::Relaxed); }
    pub fn running_idle_thread() -> bool { false }
}
mod scheduler {
    use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    const MAX_CPUS: usize = 8;
    fn current_cpu_id_raw() -> usize { 0 }
    static RESCHED_REQUESTED: [AtomicBool; MAX_CPUS] = [const { AtomicBool::new(false) }; MAX_CPUS];
    fn send_vector_x86(cpu: usize, vector: u8) {
        assert_eq!((cpu, vector), (0, 0x20));
        crate::SENT.fetch_add(1, Ordering::Relaxed);
    }
    pub fn timer_deadline_due() -> bool { false }
    pub fn set_need_resched() { panic!("unexpected quantum expiry"); }
    pub fn resched_requested() -> bool { RESCHED_REQUESTED[0].load(Ordering::Relaxed) }
    @@DECISION@@
    @@RETRY@@
}
fn trace_timer_tick(_: u64) { TICKS.fetch_add(1, Ordering::Relaxed); }
fn at(tsc: u64) { TSC.store(tsc, Ordering::Relaxed); }
fn ticks() -> u64 { TICKS.load(Ordering::Relaxed) }
#[test]
fn only_timer_expiries_count_as_delivered_ticks() {
    // A timer entry counts, and charges the quantum once.
    at(1_000);
    timer_interrupt_handler(0, 0);
    assert_eq!(ticks(), 1);
    assert_eq!(QUANTUM.load(Ordering::Relaxed), 9);
    // Reschedule IPIs never count and do not charge elapsed time twice.
    for _ in 0..10 {
        timer_interrupt_handler(0, 1);
    }
    assert_eq!(ticks(), 1, "IPIs are not delivered timer ticks");
    assert_eq!(QUANTUM.load(Ordering::Relaxed), 9);
    // A retry self-IPI arrives on the timer's own vector and is not a tick.
    scheduler::retry_after_interrupts_x86();
    assert_eq!(SENT.load(Ordering::Relaxed), 1);
    assert!(scheduler::resched_requested());
    at(1_030);
    timer_interrupt_handler(0, 0);
    assert_eq!(ticks(), 1, "a retry self-IPI is not a delivered timer tick");
    // The next timer expiry still counts.
    at(1_100);
    NOW.store(2, Ordering::Relaxed);
    timer_interrupt_handler(1, 0);
    assert_eq!(ticks(), 2, "real timer delivery must still count");
    assert_eq!(QUANTUM.load(Ordering::Relaxed), 8);
    // A retry storm: the retries are not ticks, the expiry merged into one of
    // them a full period after the last counted tick is.
    for (tsc, expected) in [(1_150, 2), (1_199, 2), (1_205, 3), (1_250, 3), (1_304, 3), (1_305, 4)] {
        scheduler::retry_after_interrupts_x86();
        at(tsc);
        timer_interrupt_handler(0, 0);
        assert_eq!(ticks(), expected, "retry entry at {tsc}");
    }
    // A reschedule IPI does not consume a pending retry.
    scheduler::retry_after_interrupts_x86();
    at(1_320);
    timer_interrupt_handler(0, 1);
    timer_interrupt_handler(0, 0);
    assert_eq!(ticks(), 4);
    at(1_405);
    timer_interrupt_handler(0, 0);
    assert_eq!(ticks(), 5, "with no retry pending every timer-vector entry is a tick");
    assert_eq!(IRQ_DEPTH.load(Ordering::Relaxed), 0);
}
'''.replace('@@DECISION@@', decision).replace('@@RETRY@@', retry)
with tempfile.TemporaryDirectory(prefix='breenix-x86-timer-') as tmp:
    path = Path(tmp)
    (path / 'test.rs').write_text(harness + quantum + '\n' + handler)
    subprocess.run((['taskpolicy', '-b'] if sys.platform == 'darwin' else []) +
                   ['rustc', '--edition=2021', '--test', '-Dwarnings', str(path / 'test.rs'),
                    '-o', str(path / 'test')], cwd=ROOT, check=True)
    subprocess.run([str(path / 'test')], check=True)

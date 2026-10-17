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
retry = scheduler_source[scheduler_source.index('pub fn retry_after_interrupts_x86()'):
                         scheduler_source.index('/// x86_64: arrange for this CPU to pick again')]
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
    use core::sync::atomic::{AtomicBool, Ordering};
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
    timer_interrupt_handler(0, 1);
    assert_eq!(ticks(), 1, "a retry self-IPI is not a delivered timer tick");
    // The next timer expiry still counts.
    at(1_100);
    NOW.store(2, Ordering::Relaxed);
    timer_interrupt_handler(1, 0);
    assert_eq!(ticks(), 2, "real timer delivery must still count");
    assert_eq!(QUANTUM.load(Ordering::Relaxed), 8);
    // Arbitrarily delayed retry-only entries never invent timer deliveries.
    for tsc in [1_150, 1_199, 1_205, 1_250, 1_304, 100_000] {
        scheduler::retry_after_interrupts_x86();
        at(tsc);
        timer_interrupt_handler(0, 1);
        assert_eq!(ticks(), 2, "retry entry at {tsc}");
    }
    // A real expiry immediately after a delayed delivery still counts.
    timer_interrupt_handler(0, 0);
    timer_interrupt_handler(0, 0);
    assert_eq!(ticks(), 4);
    assert_eq!(IRQ_DEPTH.load(Ordering::Relaxed), 0);
}
'''.replace('@@RETRY@@', retry)
with tempfile.TemporaryDirectory(prefix='breenix-x86-timer-') as tmp:
    path = Path(tmp)
    (path / 'test.rs').write_text(harness + quantum + '\n' + handler)
    subprocess.run((['taskpolicy', '-b'] if sys.platform == 'darwin' else []) +
                   ['rustc', '--edition=2021', '--test', '-Dwarnings', str(path / 'test.rs'),
                    '-o', str(path / 'test')], cwd=ROOT, check=True)
    subprocess.run([str(path / 'test')], check=True)

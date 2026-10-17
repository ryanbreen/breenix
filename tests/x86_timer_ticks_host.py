#!/usr/bin/env python3
"""Run the real x86 tick handler with explicit timer and reschedule deliveries."""
from pathlib import Path
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
source = (ROOT / 'kernel/src/interrupts/timer.rs').read_text()
handler = source[source.index('#[no_mangle]'):source.index('/// Reset this CPU')]
quantum = next(line for line in source.splitlines() if line.startswith('const TIME_QUANTUM:'))
harness = r'''
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
static TICKS: AtomicU64 = AtomicU64::new(0);
static NOW: AtomicU64 = AtomicU64::new(1);
static LAST: AtomicU64 = AtomicU64::new(0);
static QUANTUM: AtomicU32 = AtomicU32::new(10);
static IRQ_DEPTH: AtomicU32 = AtomicU32::new(0);
mod time {
    pub fn timer_interrupt() {}
    pub fn get_ticks() -> u64 { super::NOW.load(super::Ordering::Relaxed) }
}
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
    pub fn timer_deadline_due() -> bool { false }
    pub fn set_need_resched() { panic!("unexpected quantum expiry"); }
}
fn trace_timer_tick(_: u64) { TICKS.fetch_add(1, Ordering::Relaxed); }
#[test]
fn reschedule_delivery_does_not_spend_a_timer_tick() {
    timer_interrupt_handler(0, 0);
    assert_eq!(TICKS.load(Ordering::Relaxed), 1);
    assert_eq!(QUANTUM.load(Ordering::Relaxed), 9);
    for _ in 0..10 {
        timer_interrupt_handler(0, 1);
    }
    assert_eq!(TICKS.load(Ordering::Relaxed), 1, "IPIs are not delivered timer ticks");
    assert_eq!(QUANTUM.load(Ordering::Relaxed), 9, "an IPI does not charge elapsed time twice");
    NOW.store(2, Ordering::Relaxed);
    timer_interrupt_handler(1, 0);
    assert_eq!(TICKS.load(Ordering::Relaxed), 2, "real timer delivery must still count");
    assert_eq!(QUANTUM.load(Ordering::Relaxed), 8);
    assert_eq!(IRQ_DEPTH.load(Ordering::Relaxed), 0);
}
'''
with tempfile.TemporaryDirectory(prefix='breenix-x86-timer-') as tmp:
    path = Path(tmp)
    (path / 'test.rs').write_text(harness + quantum + '\n' + handler)
    subprocess.run((['taskpolicy', '-b'] if sys.platform == 'darwin' else []) +
                   ['rustc', '--edition=2021', '--test', '-Dwarnings', str(path / 'test.rs'),
                    '-o', str(path / 'test')], cwd=ROOT, check=True)
    subprocess.run([str(path / 'test')], check=True)

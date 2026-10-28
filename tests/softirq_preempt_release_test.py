#!/usr/bin/env python3
"""Execute the kernel's IRQ-exit/preemption wrappers with a host per-CPU HAL.

Only GS storage and interrupt masking are substituted. The extracted functions
are the implementation under test, not a second copy of its dispatch policy.
"""
from pathlib import Path
import platform
import re
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


def function(source, name):
    match = re.search(rf'^(?:pub )?fn {name}\(', source, re.M)
    if match is None:
        return ""
    opening = source.index('{', match.start())
    depth = 1
    end = opening + 1
    while depth:
        depth += (source[end] == '{') - (source[end] == '}')
        end += 1
    return source[match.start():end]


class SoftirqPreemptRelease(unittest.TestCase):
    def test_real_irq_exit_and_release(self):
        source = (ROOT / 'kernel/src/per_cpu.rs').read_text()
        functions = '\n'.join(function(source, name) for name in (
            'irq_enter', 'irq_exit', 'preempt_disable', 'preempt_enable',
            'run_pending_softirqs',
        ))
        harness = r'''
use core::sync::atomic::{compiler_fence, AtomicBool, AtomicU32, AtomicU64, Ordering};
const PREEMPT_MASK: u32 = 0xff;
const SOFTIRQ_MASK: u32 = 0xff00;
const SOFTIRQ_OFFSET: u32 = 0x100;
const HARDIRQ_MASK: u32 = 0x3ff0000;
const NMI_MASK: u32 = 0x4000000;
static PER_CPU_INITIALIZED: AtomicBool = AtomicBool::new(true);
static IRQ_ENTER_COUNT: AtomicU64 = AtomicU64::new(0);
static IRQ_EXIT_COUNT: AtomicU64 = AtomicU64::new(0);
static MAX_PREEMPT_IMBALANCE: AtomicU64 = AtomicU64::new(0);
static PREEMPT_UNDERFLOW_COUNT: AtomicU64 = AtomicU64::new(0);
static COUNT: AtomicU32 = AtomicU32::new(0);
static PENDING: AtomicU32 = AtomicU32::new(0);
static LOCK_HELD: AtomicBool = AtomicBool::new(false);
static MASKED: AtomicBool = AtomicBool::new(false);
static CALLS: AtomicU32 = AtomicU32::new(0);
mod hal_percpu {
    use super::*;
    pub struct X86PerCpu;
    impl X86PerCpu {
        pub fn preempt_count() -> u32 { COUNT.load(Ordering::Relaxed) }
        pub fn preempt_disable() { COUNT.fetch_add(1, Ordering::Relaxed); }
        pub fn preempt_enable() { COUNT.fetch_sub(1, Ordering::Relaxed); }
        pub unsafe fn irq_enter() { COUNT.fetch_add(1 << 16, Ordering::Relaxed); }
        pub unsafe fn irq_exit() { COUNT.fetch_sub(1 << 16, Ordering::Relaxed); }
    }
}
fn softirq_pending() -> u32 { PENDING.load(Ordering::Relaxed) }
fn arch_without_interrupts(f: impl FnOnce()) {
    let old = MASKED.swap(true, Ordering::Relaxed);
    f();
    MASKED.store(old, Ordering::Relaxed);
}
fn do_softirq() {
    assert_eq!(COUNT.load(Ordering::Relaxed), 0, "unsafe callback context");
    assert!(!LOCK_HELD.load(Ordering::Relaxed), "callback deadlocked on interrupted lock");
    assert!(MASKED.load(Ordering::Relaxed), "local dispatch must exclude IRQ reentry");
    COUNT.fetch_add(SOFTIRQ_OFFSET, Ordering::Relaxed);
    PENDING.store(0, Ordering::Relaxed);
    CALLS.fetch_add(1, Ordering::Relaxed);
    // Handler lock release must not recursively dispatch the same softirq.
    preempt_disable();
    preempt_enable();
    COUNT.fetch_sub(SOFTIRQ_OFFSET, Ordering::Relaxed);
}
__FUNCTIONS__
fn reset(count: u32) {
    COUNT.store(count, Ordering::Relaxed);
    PENDING.store(1 << 5, Ordering::Relaxed); // xHCI IrqPoll
    CALLS.store(0, Ordering::Relaxed);
    LOCK_HELD.store(false, Ordering::Relaxed);
    MASKED.store(false, Ordering::Relaxed);
}
#[test]
fn interrupted_lock_defers_until_outermost_release_without_another_irq() {
    reset(0);
    preempt_disable();
    LOCK_HELD.store(true, Ordering::Relaxed);
    preempt_disable();
    arch_without_interrupts(|| {
        irq_enter();
        irq_exit();
    });
    assert_eq!(CALLS.load(Ordering::Relaxed), 0);
    assert_ne!(softirq_pending(), 0);
    preempt_enable();
    assert_eq!(CALLS.load(Ordering::Relaxed), 0, "nested release ran callback");
    LOCK_HELD.store(false, Ordering::Relaxed);
    preempt_enable();
    assert_eq!(CALLS.load(Ordering::Relaxed), 1, "IrqPoll stranded until another irq_exit");
    assert_eq!(softirq_pending(), 0);
    assert_eq!(COUNT.load(Ordering::Relaxed), 0);
}
#[test]
fn release_and_irq_exit_preserve_interrupt_context_exclusion() {
    for context in [SOFTIRQ_OFFSET, 2 * SOFTIRQ_OFFSET, 1 << 16, NMI_MASK] {
        assert_ne!(context & (SOFTIRQ_MASK | HARDIRQ_MASK | NMI_MASK), 0);
        reset(context);
        preempt_disable();
        preempt_enable();
        MASKED.store(true, Ordering::Relaxed);
        irq_enter();
        irq_exit();
        assert_eq!(CALLS.load(Ordering::Relaxed), 0, "nested context {context:#x}");
        assert_ne!(softirq_pending(), 0);
    }
    reset(0);
    MASKED.store(true, Ordering::Relaxed);
    irq_enter();
    irq_exit();
    assert_eq!(CALLS.load(Ordering::Relaxed), 1, "ordinary irq_exit dispatch regressed");
}
'''.replace('__FUNCTIONS__', functions)
        target = 'x86_64-apple-darwin' if platform.system() == 'Darwin' else 'x86_64-unknown-linux-gnu'
        with tempfile.TemporaryDirectory(prefix='softirq-release-') as directory:
            path = Path(directory)
            (path / 'check.rs').write_text(harness)
            command = ['rustc', '--edition=2021', '--test', '--target', target,
                       '-Dwarnings', str(path / 'check.rs'), '-o', str(path / 'check')]
            if platform.system() == 'Darwin':
                command = ['taskpolicy', '-b', *command]
            subprocess.run(command, check=True)
            subprocess.run([str(path / 'check'), '--test-threads=1'], check=True)


if __name__ == '__main__':
    unittest.main()

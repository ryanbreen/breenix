#!/usr/bin/env python3
"""Run ARM64 accounting with simulated counter registers and IRQ context."""
from pathlib import Path
import re
import subprocess
import tempfile

root = Path(__file__).resolve().parents[1]
source = (root / 'kernel/src/arch_impl/aarch64/irq_mask_account.rs').read_text()
source = '\n'.join(line for line in source.splitlines() if not line.startswith('//!'))
source = re.sub(r'    fn cpu\(\) -> Option<usize> \{.*?\n    \}',
                '    fn cpu() -> Option<usize> { Some(0) }', source, flags=re.S)

def register(match):
    reg, dest = match.groups()
    if dest == 'counter':
        return 'counter = crate::NOW.load(Ordering::Relaxed)'
    value = 'crate::LATER' if dest == 'count' else 'crate::DUE' if dest == 'cval' else 'crate::CTL'
    advance = ' crate::NOW.store(count, Ordering::Relaxed);' if dest == 'count' else ''
    return f'{dest} = {value}.load(Ordering::Relaxed){";" + advance.rstrip(";") if advance else ""}'

source = re.sub(r'core::arch::asm!\("mrs \{\}, (\w+)", out\(reg\) (\w+), options\(nomem, nostack, preserves_flags\)\)', register, source)
# Privileged reads were substituted; the remaining unsafe frame access
# belongs to ExceptionStretch and is exercised through the real returning hook.
source = source.replace('unsafe { counter =', '{ counter =').replace('        unsafe {\n            if crate::platform_config', '        {\n            if crate::platform_config')
harness = r'''
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
static NOW: AtomicU64 = AtomicU64::new(1);
static LATER: AtomicU64 = AtomicU64::new(1);
static DUE: AtomicU64 = AtomicU64::new(1001);
static CTL: AtomicU64 = AtomicU64::new(1);
static IRQ: AtomicBool = AtomicBool::new(false);
static PHYSICAL: AtomicBool = AtomicBool::new(false);
mod task { pub mod scheduler { pub const MAX_CPUS: usize = 1; } }
mod per_cpu_aarch64 { pub fn in_interrupt() -> bool { crate::IRQ.load(crate::Ordering::Relaxed) } }
mod platform_config { pub fn use_physical_timer() -> bool { crate::PHYSICAL.load(crate::Ordering::Relaxed) } }
mod arch_impl { pub mod aarch64 {
    pub mod timer_interrupt {
        pub static TICKS_PER_INTERRUPT: crate::AtomicU64 = crate::AtomicU64::new(1000);
    }
    pub mod exception_frame { pub struct Aarch64ExceptionFrame { pub spsr: u64 } }
    pub mod irq_mask_account { ACCOUNTING }
} }
#[test]
fn nested_returns_and_late_register_reads_cannot_forge_held_time() {
    use arch_impl::aarch64::{irq_mask_account as a, timer_interrupt::TICKS_PER_INTERRUPT};
    let frame = arch_impl::aarch64::exception_frame::Aarch64ExceptionFrame { spsr: 0 };
    drop(a::ExceptionStretch::begin(&frame));
    assert_eq!(a::masked_since(0), 0);
    a::armed(1000);
    a::closing();
    NOW.store(1000, Ordering::Relaxed);
    LATER.store(1000, Ordering::Relaxed);
    a::taken_irq();
    IRQ.store(true, Ordering::Relaxed);
    a::closing_nested();
    NOW.store(1100, Ordering::Relaxed);
    a::taken_irq();
    a::returning(0);
    assert_eq!(a::held_within(0, 1000, 2600), 0);
    a::restoring(0);
    assert_eq!(a::held_within(0, 1000, 2600), 0);
    IRQ.store(false, Ordering::Relaxed);
    // The outer return may publish enabled time again.
    NOW.store(2600, Ordering::Relaxed);
    a::returning(0);
    assert_eq!(a::held_within(0, 2600, 3200), 600);
    a::opened();
    for physical in [false, true] {
        let base = if physical { 10000 } else { 0 };
        DUE.store(base + 1001, Ordering::Relaxed);
        PHYSICAL.store(physical, Ordering::Relaxed);
        NOW.store(base + 1, Ordering::Relaxed);
        a::closing();
        NOW.store(base + 1101, Ordering::Relaxed);
        LATER.store(base + 1801, Ordering::Relaxed);
        a::taken_irq();
        assert_eq!(a::held_within(0, base + 1, base + 1401), 0, "100 is below the half-tick floor");
        // A smaller period exposes the exact recorded interval too.
        TICKS_PER_INTERRUPT.store(100, Ordering::Relaxed);
        NOW.store(base + 1, Ordering::Relaxed);
        a::closing();
        NOW.store(base + 1101, Ordering::Relaxed);
        a::taken_irq();
        assert_eq!(a::held_within(0, base + 1, base + 1401), 100);
        // Discard the completed interval before testing the other timer.
        NOW.store(base + 4000, Ordering::Relaxed);
        a::closing();
        LATER.store(base + 4000, Ordering::Relaxed);
        a::taken_irq();
        TICKS_PER_INTERRUPT.store(1000, Ordering::Relaxed);
    }
}
'''.replace('ACCOUNTING', source)
with tempfile.TemporaryDirectory(prefix='irq-account-host-') as tmp:
    path = Path(tmp)
    (path / 'test.rs').write_text(harness)
    subprocess.run(['rustc', '--edition=2021', '--test', '--cfg', 'feature="testing"', '-Dwarnings', str(path / 'test.rs'), '-o', str(path / 'test')], check=True)
    subprocess.run([str(path / 'test')], check=True)

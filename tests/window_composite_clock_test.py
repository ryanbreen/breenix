"""Exercise G33's composite clock publication and procfs pair parsing."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


class CompositeClock(unittest.TestCase):
    def run_rust(self, source):
        with tempfile.TemporaryDirectory(prefix='composite-clock-') as tmp:
            path = Path(tmp) / 'check.rs'
            path.write_text(source)
            binary = Path(tmp) / 'check'
            command = ['rustc', '--edition=2021', '-Dwarnings', str(path), '-o', str(binary)]
            if sys.platform == 'darwin':
                command = ['taskpolicy', '-b', *command]
            build = subprocess.run(command, capture_output=True, text=True)
            self.assertEqual(build.returncode, 0, build.stdout + build.stderr)
            run = subprocess.run([str(binary)], capture_output=True, text=True, timeout=30)
            self.assertEqual(run.returncode, 0, run.stdout + run.stderr)

    def test_concurrent_readers_get_one_completed_event(self):
        provider = (ROOT / 'kernel/src/tracing/providers/window.rs').read_text()
        self.assertTrue('pub fn composite_clock(' in provider, 'composite event timestamp is not published')
        fixture = r'''
use std::sync::atomic::{AtomicU64, Ordering};
const PERIOD: u64 = 16_661_700;
static CLOCK: AtomicU64 = AtomicU64::new(0);
mod signal {
    pub fn monotonic_nanos() -> u64 {
        super::CLOCK.fetch_add(super::PERIOD, super::Ordering::Relaxed) + super::PERIOD
    }
}
mod per_cpu { pub fn preempt_disable() {} pub fn preempt_enable() {} }
mod tracing { pub mod counter {
    use std::sync::atomic::{AtomicU64, Ordering};
    pub struct TraceCounter(AtomicU64);
    impl TraceCounter {
        pub const fn new(_name: &str, _description: &str) -> Self { Self(AtomicU64::new(0)) }
        pub fn increment(&self) { std::thread::yield_now(); self.add(1); }
        pub fn add(&self, n: u64) { self.0.fetch_add(n, Ordering::Relaxed); }
        pub fn aggregate(&self) -> u64 { self.0.load(Ordering::Relaxed) }
        pub fn get_cpu(&self, _: usize) -> u64 { self.aggregate() }
        pub fn add_cpu(&self, _: usize, n: u64) { self.add(n); }
        pub fn reset(&self) { self.0.store(0, Ordering::Relaxed); }
    }
    pub fn register_counter(c: &TraceCounter) -> Option<usize> { Some(c.aggregate() as usize) }
} }
mod window { PROVIDER }
fn main() {
    window::init();
    per_cpu::preempt_disable(); per_cpu::preempt_enable();
    window::count_damage_rects(3); window::count_buffer_alloc();
    window::record_irq_masked(2); window::reset_irq_masked();
    assert_eq!(window::composite_clock(), (0, 0));
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
    std::thread::scope(|s| {
        let start = barrier.clone();
        s.spawn(move || { start.wait(); for _ in 0..20_000 { window::count_composite(7, signal::monotonic_nanos()); } });
        for _ in 0..2 {
            let start = barrier.clone();
            s.spawn(move || {
                start.wait();
                for _ in 0..100_000 {
                    let (count, at) = window::composite_clock();
                    assert_eq!(at, count * PERIOD, "mixed composite count/time");
                }
            });
        }
    });
    assert_eq!(window::composite_clock(), (20_000, 20_000 * PERIOD));
    assert_eq!(window::WINDOW_COMPOSITES.aggregate(), 20_000);
    assert_eq!(window::WINDOW_COMPOSITE_PIXELS.aggregate(), 140_000);
}
'''.replace('PROVIDER', provider.replace('//!', '//'))
        self.run_rust(fixture)

    def test_split_procfs_reads_cannot_mix_events(self):
        kit = (ROOT / 'userspace/programs/src/suite_breengel/window_cases/kit.rs').read_text()
        self.assertTrue('fn parse_composite_clock(' in kit, 'composite event pair reader is not implemented')
        parser = item(kit, 'fn parse_composite_clock(')
        self.run_rust(parser + r'''
fn main() {
    assert_eq!(parse_composite_clock("composites: 9 (cpu0=9) last_ns=150000000 snapshot_count=9\n"), Some((9,150000000)));
    assert_eq!(parse_composite_clock("composites: 0 last_ns=0 snapshot_count=0\n"), Some((0,0)));
    for bad in [
        "composites: 9 last_ns=150000000 snapshot_count=10",
        "composites: 9 last_ns=150000000",
        "composites: 9 snapshot_count=9",
        "composites: 9 last_ns=no snapshot_count=9",
        "composites: 9 last_ns=150000000 last_ns=160000000 snapshot_count=9",
        "composites: 9 last_ns=150000000 snapshot_count=9 snapshot_count=10",
    ] { assert_eq!(parse_composite_clock(bad), None, "{bad}"); }
}
''')


if __name__ == '__main__':
    unittest.main()

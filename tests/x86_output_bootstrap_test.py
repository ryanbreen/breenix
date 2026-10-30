"""Early x86 output accounting must not read an uninitialized GS CPU ID."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tests'))
from x86_output_accounting_test import item


class OutputBootstrap(unittest.TestCase):
    def test_early_output_uses_an_explicit_slot_then_the_current_cpu(self):
        source = (ROOT / 'kernel/src/tracing/providers/output.rs').read_text()
        # Exercise the x86 branch even when this harness runs on an ARM host.
        count = item(source, 'pub fn count_unattributed(').replace(
            '#[cfg(target_arch = "x86_64")]', '')
        harness = r'''
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
mod per_cpu {
    pub static READY: super::AtomicBool = super::AtomicBool::new(false);
    pub fn is_initialized() -> bool { READY.load(super::Ordering::Relaxed) }
}
struct Counter { bootstrap: AtomicU64, current: AtomicU64 }
impl Counter {
    fn add(&self, n: u64) {
        assert!(per_cpu::is_initialized(), "GS-relative CPU ID read before per-CPU init");
        self.current.fetch_add(n, Ordering::Relaxed);
    }
    fn add_cpu(&self, cpu: usize, n: u64) {
        assert_eq!(cpu, 0);
        self.bootstrap.fetch_add(n, Ordering::Relaxed);
    }
}
static KERNEL_OUTPUT_UNATTRIBUTED: Counter = Counter {
    bootstrap: AtomicU64::new(0), current: AtomicU64::new(0),
};
// COUNT
fn main() {
    let _explicit_slot: fn(&Counter,usize,u64) = Counter::add_cpu;
    count_unattributed(33);
    assert_eq!(KERNEL_OUTPUT_UNATTRIBUTED.bootstrap.load(Ordering::Relaxed), 33);
    assert_eq!(KERNEL_OUTPUT_UNATTRIBUTED.current.load(Ordering::Relaxed), 0);
    per_cpu::READY.store(true, Ordering::Relaxed);
    count_unattributed(7);
    assert_eq!(KERNEL_OUTPUT_UNATTRIBUTED.bootstrap.load(Ordering::Relaxed), 33);
    assert_eq!(KERNEL_OUTPUT_UNATTRIBUTED.current.load(Ordering::Relaxed), 7);
}
'''.replace('// COUNT', count)
        prefix = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            (path / 'bootstrap.rs').write_text(harness)
            subprocess.run(prefix + ['rustc', '--edition=2021', '-Dwarnings',
                           str(path / 'bootstrap.rs'), '-o', str(path / 'bootstrap')], check=True)
            subprocess.run([str(path / 'bootstrap')], check=True)


if __name__ == '__main__':
    unittest.main()

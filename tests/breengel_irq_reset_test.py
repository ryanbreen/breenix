"""Exercise G31's reset/read race with the production gauge and case check.

Only the clock, counter storage and suite error wrapper are host substitutes.
The failed-reset control removes the reset store, leaving the hold in place.
"""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


class BreenGLIRQReset(unittest.TestCase):
    def test_fresh_interval_passes_but_surviving_hold_fails(self):
        provider = (ROOT / 'kernel/src/tracing/providers/gpu.rs').read_text()
        reset = item(provider, 'pub fn reset_irq_masked(')
        failed_reset = reset.replace('reset_irq_masked', 'failed_reset_irq_masked').replace(
            'GPU_IRQ_MASKED_MAX_US.per_cpu[0].value.store(0, Ordering::Relaxed);', '')
        self.assertNotEqual(reset, failed_reset)
        production = '\n'.join(item(provider, name) for name in [
            'fn counter_now(', 'fn record_since(', 'pub fn masked_end('])
        case = item((ROOT / 'userspace/programs/src/suite_breengel/window_cases/composition.rs').read_text(),
                    'fn composition_irq_bound(')
        assertion = next(line.strip() for line in case.splitlines()
                         if line.strip().startswith('ensure(reset '))
        fixture = r'''
use std::sync::atomic::{AtomicU64, Ordering};
struct Slot { value: AtomicU64 }
struct Counter { per_cpu: [Slot; 1] }
static GPU_IRQ_MASKED_MAX_US: Counter = Counter {
    per_cpu: [Slot { value: AtomicU64::new(0) }],
};
static CLOCK: AtomicU64 = AtomicU64::new(1);
fn arch_read_timestamp() -> u64 { CLOCK.load(Ordering::Relaxed) }
fn arch_ticks_to_nanos(ticks: u64) -> u64 { ticks }
PRODUCTION
fn maximum() -> u64 { GPU_IRQ_MASKED_MAX_US.per_cpu[0].value.load(Ordering::Relaxed) }
fn interval(us: u64) {
    let start = counter_now();
    CLOCK.fetch_add(us * 1_000, Ordering::Relaxed);
    masked_end(start);
}
fn ensure(ok: bool, _: &str, message: String) -> Result<(), String> {
    if ok { Ok(()) } else { Err(message) }
}
fn case_reset_check(reset: u64) -> Result<(), String> {
    ASSERTION
    Ok(())
}
#[test]
fn reset_then_fresh_interval_and_failed_reset_control() {
    for hold in [2_000, 2_500] {
        reset_irq_masked();
        interval(hold);
        assert_eq!(maximum(), hold, "calibrated hold reaches the production gauge");
        reset_irq_masked();
        assert_eq!(maximum(), 0, "reset itself clears the hold");
        interval(1); // kgpu may run between DISARM_ALL and the procfs read.
        let later_read = maximum();
        assert_eq!(later_read, 1, "fresh interval makes the later read nonzero");
        assert!(case_reset_check(later_read).is_ok(),
                "G31: a fresh 1 us interval must not fail the reset check");

        interval(hold);
        failed_reset_irq_masked(); // Planted defect: DISARM_ALL did not clear the gauge.
        interval(1);
        assert_eq!(maximum(), hold, "fresh intervals cannot hide the surviving hold");
        assert!(case_reset_check(maximum()).is_err(),
                "a failed reset must still be rejected");
    }
}
'''.replace('PRODUCTION', reset + '\n' + failed_reset + '\n' + production).replace('ASSERTION', assertion)
        with tempfile.TemporaryDirectory(prefix='breengel-irq-reset-') as tmp:
            source = Path(tmp) / 'check.rs'
            source.write_text(fixture)
            binary = Path(tmp) / 'check'
            target = 'x86_64-apple-darwin' if sys.platform == 'darwin' else 'x86_64-unknown-linux-gnu'
            args = ['rustc', '--edition=2021', '--target', target, '--test', '-Dwarnings',
                    str(source), '-o', str(binary)]
            if sys.platform == 'darwin':
                args = ['taskpolicy', '-b', *args]
            build = subprocess.run(args, capture_output=True, text=True)
            self.assertEqual(build.returncode, 0, build.stdout + build.stderr)
            run = subprocess.run([str(binary), '--nocapture'], capture_output=True, text=True)
            self.assertEqual(run.returncode, 0, run.stdout + run.stderr)


if __name__ == '__main__':
    unittest.main()

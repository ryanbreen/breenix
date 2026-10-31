"""Stop the authorized production capture on clock loss or thermal trip."""
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location('watch', Path(__file__).resolve().parents[1] / 'scripts/metal/gt710_c7_watch.py')
watch = importlib.util.module_from_spec(spec)
spec.loader.exec_module(watch)

class Watch(unittest.TestCase):
    def test_startup_requires_full_clocks_and_counter(self):
        good = 'GT710_CLOCK_STARTUP full=true gpc_khz=1306451 gpc_counter_khz=81650 memory_khz=900001 temperature_c=34 status=ok'
        self.assertIsNone(watch.stop_reason(good))
        for old, new in [('full=true','full=false'), ('1306451','810000'), ('81650','50625'), ('900001','405000')]:
            self.assertIsNotNone(watch.stop_reason(good.replace(old, new)))
        self.assertIsNotNone(watch.stop_reason('GT710_CLOCK_FALLBACK reason=EXEC-timeout'))

    def test_guard_display_fault_and_suite_completion(self):
        self.assertIsNone(watch.stop_reason('GT710_CLOCK_TEMPERATURE monotonic_ns=1000000000 celsius=84 tripped=false'))
        for line in ['GT710_CLOCK_TEMPERATURE monotonic_ns=2000000000 celsius=85 tripped=true',
                     'GT710_CLOCK_GUARD trip_c=85',
                     'GT710_NATIVE_FAULT reason=native present lost display health',
                     'GT710_C5F_ERROR C5f fresh display underflow or reporting state failure',
                     'PANIC: test']:
            self.assertIsNotNone(watch.stop_reason(line))
        # An attributed MMU fault is a required suite stimulus, not clock loss.
        self.assertIsNone(watch.stop_reason('GT710_NATIVE_MMU_FAULT code=0x60010002'))
        self.assertIsNone(watch.stop_reason('SUITE: gpu CASE 1 FAIL performance/coverage-throughput'))

if __name__ == '__main__': unittest.main()

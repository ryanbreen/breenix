"""C5e observation errors must never become power-off requests."""
import importlib.util
import io
from pathlib import Path
import unittest
from unittest.mock import Mock, patch

SPEC = importlib.util.spec_from_file_location(
    'c5e_watch', Path(__file__).resolve().parents[1] / 'scripts/metal/gt710_c5e_watch.py')
watch = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(watch)


class Watcher(unittest.TestCase):
    def test_startup_and_unacted_records_are_not_json_parsed(self):
        logs = []
        for line in (
            'GT710_TEMPERATURE reading={"value":null,"status":"needs-setup"} restored=true',
            'GT710_RAM_INIT_WRITE not-json',
            'GT710_CLOCK_MEASURED not-json',
        ):
            self.assertIsNone(watch.stop_reason(line, logs.append))
        self.assertEqual(logs, [])

    def test_bad_acted_records_log_and_continue_to_done(self):
        logs = []
        for line in ('GT710_TEMPERATURE {broken', 'GT710_BENCH {}',
                     'GT710_MEMORY_READ {"readback_khz":"bad","post_khz":405000}'):
            self.assertIsNone(watch.stop_reason(line, logs.append))
        self.assertEqual(len(logs), 3)
        self.assertTrue(all('WATCH_PARSE_ERROR' in line for line in logs))
        self.assertEqual(watch.stop_reason('GT710_C5E_DONE', logs.append), 'C5e-complete')

    def test_only_step_stop_conditions(self):
        cases = {
            'GT710_C5E_ERROR GPU or PMU fault': 'gpu-fault-or-failed-step',
            'GT710_PMU_WAIT name=recv raw=0 matched=false': 'PMU-ring-timeout',
            'GT710_BENCH {"completion":false}': 'completion-failed',
            'GT710_TEMPERATURE {"celsius":85}': 'temperature-85C',
            'GT710_MEMORY_READ {"readback_khz":900000,"post_khz":405000}': 'memory-clock-outside-1-percent',
            'GT710_C5E_DONE': 'C5e-complete',
            'GT710_MEMX_EXEC_REPLY status=-1 reason=no-reply': 'EXEC-failed',
            'GT710_MEMX_EXEC_REPLY status=0 pause_ns=1234 pmu_input=0x10000': None,
            'GT710_MEMORY_READ {"readback_khz":409050,"post_khz":405000}': None,
            'GT710_MEMORY_READ {"readback_khz":400950,"post_khz":405000}': None,
            'GT710_MEMORY_READ {"readback_khz":409051,"post_khz":405000}': 'memory-clock-outside-1-percent',
            'GT710_C5E_CONSOLE_OK fresh=true identical=true': None,
            'GT710_TEMPERATURE {"celsius":84}': None,
            'GT710_BENCH {"completion":true,"elapsed_ns":800000000}': None,
            'GT710_MEMORY_READ {"readback_khz":405000,"post_khz":405000}': None,
        }
        for line, expected in cases.items():
            with self.subTest(line=line):
                self.assertEqual(watch.stop_reason(line, self.fail), expected)

    def test_target_frequency_uses_stage_limit_and_never_bench_time(self):
        for khz in (891000, 900000, 909000):
            self.assertIsNone(watch.stop_reason('GT710_MEMORY_READ '+str({"readback_khz":khz,"post_khz":405000}).replace("'", '"'), self.fail, target=900000))
        self.assertEqual(watch.stop_reason('GT710_MEMORY_READ {"readback_khz":405000,"post_khz":405000}', self.fail, target=900000), 'memory-clock-outside-1-percent')
        self.assertEqual(watch.stop_reason('GT710_MEMORY_READ {"readback_khz":909001,"post_khz":405000}', self.fail, target=900000), 'memory-clock-outside-1-percent')
        self.assertIsNone(watch.stop_reason('GT710_BENCH {"completion":true,"elapsed_ns":9000000000}', self.fail, target=900000))

    def test_child_failure_or_normal_end_never_stops_boot(self):
        for code in (1, 0, -15, watch.STOP_REQUESTED):
            child = Mock(stdout=io.StringIO('WATCH_PARSE_ERROR example\n'))
            child.wait.return_value = code
            with patch.object(watch.subprocess, 'Popen', return_value=child), \
                 patch.object(watch.subprocess, 'run') as stop, \
                 patch.dict(watch.os.environ, {'METAL_INSTANCE': 'synthetic-instance'}):
                watch.run_on_host('20990101T000000Z-metal', 'a' * 40)
            self.assertEqual(stop.call_count, int(code == watch.STOP_REQUESTED))
            if stop.called:
                self.assertEqual(stop.call_args.args[0][-2:], ['stop', '20990101T000000Z-metal'])


if __name__ == '__main__':
    unittest.main()

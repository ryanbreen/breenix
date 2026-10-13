"""Exercise launcher outcomes without booting a VM."""
import importlib.util
import json
import os
from pathlib import Path
import tempfile
import time
import unittest
from unittest.mock import Mock, patch

SPEC = importlib.util.spec_from_file_location('gate_boot', Path(__file__).resolve().parents[1] / 'scripts/x86-gate-boot.py')
gate = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(gate)
COMPLETE = '[ INFO] kernel::syscall::handlers: 🎯 USERSPACE TEST COMPLETE - All processes finished\n'
DONE = '[ INFO] kernel::syscall::handlers: USERSPACE TEST REPORT DONE\n'


class BootOutcomes(unittest.TestCase):
    def run_gate(self, serial, poll, times, deadline='10', offset=0, report_clock=True, backstop='20', user_output=''):
        with tempfile.TemporaryDirectory() as directory:
            kernel = Path(directory) / 'kernel'
            user = Path(directory) / 'user'
            host_epoch = time.time()
            kernel.write_text(serial)
            user.write_text((f'[clock] UTC epoch={int(host_epoch) + offset} source=CMOS\n' if report_clock else '') + user_output)
            process = Mock(returncode=7)
            if isinstance(poll, list):
                process.poll.side_effect = poll
            else:
                process.poll.return_value = poll
            readings = iter(times)
            last = times[-1]
            def monotonic():
                return next(readings, last)
            with patch.object(gate.sys, 'argv', ['gate', str(kernel), str(user), 'launcher']), \
                 patch.object(gate.subprocess, 'Popen', return_value=process), \
                 patch.object(gate.subprocess, 'run'), \
                 patch.object(gate, 'stop') as stop, \
                 patch.object(gate.time, 'monotonic', side_effect=monotonic), \
                 patch.object(gate.time, 'time', return_value=host_epoch), \
                 patch.dict(os.environ, {'BREENIX_GATE_TIMEOUT': deadline, 'BREENIX_FULL_BACKSTOP': backstop}):
                status = gate.main()
                stop.assert_called_once_with(process)
                if status == 124 and user_output:
                    record = json.loads(Path(str(user) + '.stall.json').read_text())
                    self.assertEqual(record['status'], 'stalled')
                    self.assertEqual(record['last_progress_line'], user_output.rstrip())
                    self.assertGreaterEqual(record['inactivity_seconds'], 120)
                return status

    def test_completion_with_interleaved_prefix(self):
        self.assertEqual(self.run_gate('noise' + COMPLETE + DONE, None, [0, 2, 2]), 0)

    def test_completed_boot_with_local_clock_fails(self):
        self.assertEqual(self.run_gate(COMPLETE + DONE, None, [0, 2, 2], offset=3600), 3)

    def test_completed_boot_without_epoch_fails(self):
        self.assertEqual(self.run_gate(COMPLETE + DONE, None, [0, 2, 2], report_clock=False), 3)

    def test_late_completion_is_scored_after_report(self):
        self.assertEqual(self.run_gate(COMPLETE + DONE, None, [0, 11, 11]), 2)

    def test_launcher_exits_early(self):
        self.assertEqual(self.run_gate('', 7, [0]), 1)

    def test_backstop_waits_for_report_done(self):
        self.assertEqual(self.run_gate(COMPLETE, None, [0, 21]), 1)

    def test_completion_announcement_is_not_completion(self):
        self.assertEqual(self.run_gate('kernel::test_exec: -> Userspace will emit USERSPACE TEST COMPLETE marker\n', None, [0, 21]), 1)

    def test_silent_userspace_fails_before_the_backstop(self):
        self.assertEqual(self.run_gate('', None, [0, 0, 121, 121], backstop='1800',
                                      user_output='SIGKILL_TEARDOWN_TEST: FAIL\n'), 124)

    def test_quiet_userspace_under_the_limit_keeps_running(self):
        self.assertEqual(self.run_gate('', [None, 7], [0, 1, 120.5, 120.5], backstop='1800',
                                      user_output='SIGKILL_TEARDOWN_TEST: FAIL\n'), 1)


if __name__ == '__main__':
    unittest.main()

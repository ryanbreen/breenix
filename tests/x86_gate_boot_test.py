"""Exercise launcher outcomes without booting a VM."""
import importlib.util
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
    def run_gate(self, serial, poll, times, deadline='10', offset=0, report_clock=True, backstop='20'):
        with tempfile.TemporaryDirectory() as directory:
            kernel = Path(directory) / 'kernel'
            user = Path(directory) / 'user'
            kernel.write_text(serial)
            user.write_text(f'[clock] UTC epoch={int(time.time()) + offset} source=CMOS\n' if report_clock else '')
            process = Mock(returncode=7)
            process.poll.return_value = poll
            with patch.object(gate.sys, 'argv', ['gate', str(kernel), str(user), 'launcher']), \
                 patch.object(gate.subprocess, 'Popen', return_value=process), \
                 patch.object(gate.subprocess, 'run'), \
                 patch.object(gate, 'stop') as stop, \
                 patch.object(gate.time, 'monotonic', side_effect=times), \
                 patch.dict(os.environ, {'BREENIX_GATE_TIMEOUT': deadline, 'BREENIX_FULL_BACKSTOP': backstop}):
                status = gate.main()
                stop.assert_called_once_with(process)
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
        self.assertEqual(self.run_gate(COMPLETE, None, [0, 1, 50, 121], backstop='1800'), 1)

    def test_quiet_userspace_under_the_limit_keeps_running(self):
        with self.assertRaises(StopIteration):
            self.run_gate(COMPLETE, None, [0, 1, 50, 120.5], backstop='1800')

    def test_census_records_are_not_userspace_output(self):
        census = '[TOMBSTONE_CENSUS:resident=0:removed=165]\n[PT_ROOT_CUSTODY:no_proof=0:retired=306]\r\n'
        self.assertEqual(gate.userspace_output(census + 'SIGKILL_TEARDOWN_TEST: FAIL\n[CHILD] x\n'),
                         '\n\nSIGKILL_TEARDOWN_TEST: FAIL\n[CHILD] x\n')


if __name__ == '__main__':
    unittest.main()

"""Bound synthetic S21 probes and verify slot transitions follow stopped VMs."""
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import vm_upgrade_driver_test as fixture


class UpgradeProbeTest(unittest.TestCase):
    setUp = fixture.DriverTest.setUp

    def test_lease_and_strict_pin_are_required_before_ssh(self):
        probe = fixture.Probe(self.ops, Path(self.scratch.name))
        probe.run_case('host-key', '')
        for fault in ('lease', 'pin', 'strict'):
            with self.subTest(fault=fault):
                self.ops.stopped = fault == 'lease'
                probe.common = [] if fault == 'strict' else ['StrictHostKeyChecking=yes']
                if fault == 'pin': probe.known.unlink()
                result = self.module['probe_upgrade'](self.ops.machine, probe, fixture.SITE,
                                                      self.driver.host, 'flat', fixture.PIN)
                self.assertFalse(result['passed'])
                self.assertEqual(result['sshAttempts'], 0)
                self.assertFalse(any(c[0] == 'ssh' for c in self.ops.calls))
                probe.run_case('host-key', '')

    def test_probe_cannot_finish_after_launch_deadline(self):
        probe = fixture.Probe(self.ops, Path(self.scratch.name))
        probe.run_case('host-key', '')
        ssh = probe.ssh
        def slow_ssh(command, **kwargs):
            result = ssh(command, **kwargs)
            self.assertLessEqual(kwargs['timeout'], 600 - (self.ops.clock[0] - 1000))
            self.ops.clock[0] = 1601
            return result
        with patch.object(probe, 'ssh', side_effect=slow_ssh):
            result = self.module['probe_upgrade'](self.ops.machine, probe, fixture.SITE,
                                                  self.driver.host, 'flat', fixture.PIN)
        self.assertFalse(result['passed'])
        self.assertEqual(result['powerOn'], 1000)
        self.assertEqual(result['sshAttempts'], 1)

    def test_good_and_failed_candidates_stop_before_slot_transition(self):
        self.driver.prepare()
        self.driver.run_case('good')
        self.driver.run_case('broken')
        calls = self.ops.calls
        for index, call in enumerate(calls):
            if call[0] not in ('promote', 'rollback'): continue
            self.assertEqual(calls[index - 1][0], 'stop')
            if call[0] == 'rollback':
                self.assertEqual(calls[index + 1], ('boot', 'current'))
        failed = self.driver.results['broken']
        self.assertNotEqual(failed['verification']['boot'], failed['rollback']['boot'])
        self.assertTrue(failed['rollback']['passed'])


if __name__ == '__main__':
    unittest.main()

#!/usr/bin/env python3
"""Exercise the x86 host reservation's boot-phase tracking and sharing without root or a VM."""
import importlib.util
from pathlib import Path
import sys
import tempfile
import unittest

HELPER = Path(__file__).resolve().parents[1] / 'scripts' / 'x86-host-reserve.py'
spec = importlib.util.spec_from_file_location('x86_host_reserve', HELPER)
reserve = importlib.util.module_from_spec(spec)
spec.loader.exec_module(reserve)


class ReservationTest(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.events = []
        self.saved = (reserve.reserve, reserve.release)
        reserve.reserve = lambda instance: self.events.append(('reserve', instance))
        reserve.release = lambda instance: self.events.append(('release', instance))

    def tearDown(self):
        reserve.reserve, reserve.release = self.saved
        self.temporary.cleanup()

    def reservation(self):
        return reserve.Reservation('vm', self.temporary.name)

    def test_boot_phase_markers_take_and_leave_the_reservation(self):
        script = ('echo "[gate-phase] phase=boot-slot-wait ended=1"; '
                  'echo "[gate-phase] phase=boot started=2"; echo inside; '
                  'echo "[gate-phase] phase=boot ended=3"; echo after; exit 7')
        status = reserve.relay(['bash', '-c', script], self.reservation())
        self.assertEqual(status, 7)
        self.assertEqual(self.events, [('reserve', 'vm'), ('release', 'vm')])

    def test_command_that_exits_inside_a_boot_releases(self):
        reserve.relay(['bash', '-c', 'echo "[gate-phase] phase=boot started=2"; exit 1'], self.reservation())
        self.assertEqual(self.events, [('reserve', 'vm'), ('release', 'vm')])

    def test_concurrent_holders_share_one_reservation(self):
        first, second = self.reservation(), self.reservation()
        first.take()
        second.take()
        first.leave()
        self.assertEqual(self.events, [('reserve', 'vm'), ('reserve', 'vm')])
        second.leave()
        self.assertEqual(self.events[-1], ('release', 'vm'))

    def test_reservation_without_a_holder_is_cleared_before_starting(self):
        self.reservation().clear_stale()
        self.assertEqual(self.events, [('release', 'vm')])
        holder = self.reservation()
        holder.take()
        self.events.clear()
        self.reservation().clear_stale()
        self.assertEqual(self.events, [])
        holder.leave()

    def test_output_is_copied_unchanged(self):
        with tempfile.TemporaryFile() as output:
            saved = sys.stdout
            sys.stdout = output
            try:
                reserve.relay(['printf', 'a\\nb'], self.reservation())
            finally:
                sys.stdout = saved
            output.seek(0)
            self.assertEqual(output.read(), b'a\nb')


if __name__ == '__main__':
    unittest.main()

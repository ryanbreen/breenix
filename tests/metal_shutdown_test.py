"""Requested reboot retains the persistent session through autonomous return."""
from pathlib import Path
import sys
import unittest
from unittest.mock import Mock

sys.path.insert(0, str(Path(__file__).resolve().parent))
import metal_session_test as sessions
from types import SimpleNamespace
from unittest.mock import patch

boot, m = sessions.boot, sessions.m


class ShutdownTest(sessions.existing.StateTest):
    def setUp(self):
        super().setUp()
        args = SimpleNamespace(id='20261017T000009Z-metal', mode='console', payload='image',
                               session=True, idle_minutes=2, input=None, disk=None,
                               target='branch:fixture', task='', commit='a' * 40, inject=None, nmi=False)
        with patch.object(m, 'load_site', return_value={}):
            self.driver = boot.Driver(args)
        self.driver.event, self.driver.phase = Mock(), Mock()
        self.driver.dir.mkdir(parents=True)
        self.driver.t0 = 100
        self.driver.kernel_started = 101
        self.driver.on_line("[init] log boot-id=" + "b" * 64, observed_at=102)

    def test_clean_request_waits_for_a_real_reset(self):
        d = self.driver
        d.args.mode = 'persistent'
        d.args.payload = 'persistent'
        d.kernel_started = 110
        d.on_line('[shutdown] home clean read-only', observed_at=111)
        d.on_line('[shutdown] root clean read-only', observed_at=112)
        d.on_line('[shutdown] requested reboot token=' + d.shutdown_token, observed_at=113)
        self.assertEqual(d.shutdown_action, 'reboot')
        d.on_reset(120)
        self.assertEqual(d.requested_reset_at, 120)
        self.assertIsNone(d.shutdown_action)
        d.event.assert_any_call('requested-reset-observed', observed_at=120)
        d.kernel_started = 130
        d.on_line('bsshd: listening on 0.0.0.0:22', observed_at=131)
        self.assertEqual(d.requested_reset_at, 120)
        d.seen.add('manager-slot')
        d.on_line('bsshd: listening on 0.0.0.0:22', observed_at=132)
        self.assertIsNone(d.requested_reset_at)
        d.event.assert_any_call('requested-reboot-ready', observed_at=132, commit=d.args.commit)

    def test_receipt_is_scoped_to_the_kernel_boot(self):
        d = self.driver
        first = d.shutdown_token
        self.assertNotEqual(first, d.shutdown_seed)
        d.on_line('[init] log boot-id=' + 'c' * 64, observed_at=120)
        self.assertNotEqual(first, d.shutdown_token)
        for mount in ('home', 'root'):
            d.on_line('[shutdown] ' + mount + ' clean read-only', observed_at=121)
        d.on_line('[shutdown] requested poweroff token=' + first, observed_at=122)
        self.assertIsNone(d.shutdown_action)

    def test_poweroff_needs_clean_mounts_and_no_fatal(self):
        d = self.driver
        d.kernel_started = 110
        d.on_line('[shutdown] requested poweroff token=' + d.shutdown_token, observed_at=111)
        self.assertIsNone(d.shutdown_action)
        for mount in ('home', 'root'):
            d.on_line('[shutdown] ' + mount + ' clean read-only', observed_at=112)
        d.on_line('[shutdown] requested poweroff token=' + '0' * 64, observed_at=112)
        self.assertIsNone(d.shutdown_action)
        d.fatal_at = 113
        d.on_line('[shutdown] requested poweroff token=' + d.shutdown_token, observed_at=114)
        self.assertIsNone(d.shutdown_action)
        d.fatal_at = None
        with self.assertRaises(boot.Finish) as end:
            d.on_line('[shutdown] requested poweroff token=' + d.shutdown_token, observed_at=115)
        self.assertEqual(end.exception.status, m.EXIT_DONE)

    def test_unrequested_reset_and_post_request_panic_still_fail(self):
        d = self.driver
        d.kernel_started = 110
        d.event = Mock()
        d.on_reset(111)
        d.event.assert_not_called()
        d.shutdown_action, d.fatal_at = 'reboot', 112
        d.on_reset(113)
        d.event.assert_not_called()

    def test_fault_panic_is_fatal_without_the_old_rust_location_format(self):
        d = self.driver
        d.kernel_started = 110
        d.args.inject = 'panic-reboot'
        d.on_line('KERNEL PANIC: reboot fault test', observed_at=111)
        self.assertEqual(d.fatal_at, 111)
        with self.assertRaises(boot.Finish) as end:
            d.on_reset(120)
        self.assertEqual(end.exception.status, m.EXIT_SHORT)
        d.event.assert_any_call('panic-reset-observed', observed_at=120)


if __name__ == '__main__':
    unittest.main()

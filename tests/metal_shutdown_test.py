"""Requested transitions end a boot without masking a fatal or an unexpected reset."""
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

    def test_clean_request_waits_for_a_real_reset(self):
        d = self.driver
        d.kernel_started = 110
        d.on_line('[shutdown] home clean read-only', observed_at=111)
        d.on_line('[shutdown] root clean read-only', observed_at=112)
        d.on_line('[shutdown] requested reboot', observed_at=113)
        self.assertEqual(d.shutdown_action, 'reboot')
        with self.assertRaises(boot.Finish) as end:
            d.on_reset(120)
        self.assertEqual(end.exception.status, m.EXIT_DONE)
        d.event.assert_any_call('requested-reset-observed', observed_at=120)

    def test_poweroff_needs_clean_mounts_and_no_fatal(self):
        d = self.driver
        d.kernel_started = 110
        d.on_line('[shutdown] requested poweroff', observed_at=111)
        self.assertIsNone(d.shutdown_action)
        for mount in ('home', 'root'):
            d.on_line('[shutdown] ' + mount + ' clean read-only', observed_at=112)
        d.fatal_at = 113
        d.on_line('[shutdown] requested poweroff', observed_at=114)
        self.assertIsNone(d.shutdown_action)
        d.fatal_at = None
        with self.assertRaises(boot.Finish) as end:
            d.on_line('[shutdown] requested poweroff', observed_at=115)
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


if __name__ == '__main__':
    unittest.main()

"""S26: distinguish an unknown management state from confirmed POST evidence."""
from types import SimpleNamespace
from unittest.mock import Mock, patch
import unittest
import metal_tools_test as existing

boot, m = existing.boot, existing.m


class BootProgressTest(existing.StateTest):
    def setUp(self):
        super().setUp()
        self.make_driver()

    def make_driver(self):
        args = SimpleNamespace(id='20261017T000009Z-metal', mode='default', payload='image',
                               inject=None, serve_as=None, content_type=None, no_wait=False)
        with patch.object(m, 'load_site', return_value={}):
            self.d = boot.Driver(args)
        self.d.dir.mkdir(parents=True, exist_ok=True)
        self.d.event, self.d.phase = Mock(), Mock()
        self.d.t0 = 1793095631.6
        self.d.kernel_started = 1793095717.595
        self.d.progress_key = ('OSRunning', '', None)
        self.d.post_code = 126

    def reading(self, at, state, code=0):
        reading = dict(t=at, power='On', progress={'LastState': state}, postCode=code)
        self.d.bmc_events = [reading]
        self.d.pump_bmc()
        return reading

    def test_100345_unknown_without_post_keeps_the_kernel(self):
        at = 1793095756.458
        reading = self.reading(at, 'None')
        self.d.event.assert_any_call('boot-progress-unknown', observed_at=at, reading=reading)
        self.reading(at + 33, 'None')
        self.reading(at + 66, 'None')
        self.assertEqual(self.d.kernel_started, 1793095717.595)

    def test_unknown_then_post_preserves_the_first_reset_time(self):
        # Retained 20261027T093515Z-metal management sequence.
        at = 1793093977.65
        self.d.kernel_started = at - 1
        self.d.t0 = at - 47.1
        self.reading(at, 'None')
        self.d.on_reset = Mock()
        with self.assertRaises(boot.Finish) as raised:
            self.reading(1793093990.947, 'SystemHardwareInitializationComplete')
        self.assertEqual(raised.exception.status, m.EXIT_SHORT)
        self.d.on_reset.assert_called_once_with(at)

    def test_pre_handoff_post_state_is_not_new_reset_evidence(self):
        self.d.progress_key = ('SystemHardwareInitializationComplete', '', None)
        self.d.post_code = 88
        self.reading(1793095756.458, 'SystemHardwareInitializationComplete', 88)
        self.assertIsNotNone(self.d.kernel_started)

    def test_post_code_and_banner_confirm_only_within_the_window(self):
        for evidence in ('code', 'banner', 'image'):
            with self.subTest(evidence=evidence):
                self.make_driver()
                at = 1793095756.458
                self.reading(at, '')
                self.d.on_reset = Mock()
                with self.assertRaises(boot.Finish):
                    if evidence == 'code': self.reading(at + 10, '', 65)
                    elif evidence == 'banner': self.d.on_line(boot.BOOTLOADER_LINE, observed_at=at + 10)
                    else:
                        self.d.request_at, self.d.complete_at = at - 60, at - 50
                        self.d.on_http(dict(t=at + 10, event='404', route=True))
                self.d.on_reset.assert_called_once_with(at)
        self.make_driver()
        self.reading(at, None)
        self.reading(at + 61, None, 65)
        self.assertIsNotNone(self.d.kernel_started)


if __name__ == '__main__': unittest.main()

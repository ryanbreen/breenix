import argparse
from pathlib import Path
import runpy
import subprocess
import threading
import unittest
from unittest.mock import Mock


class PairedFlashTest(unittest.TestCase):
    def setUp(self):
        self.flash = runpy.run_path(str(Path(__file__).resolve().parents[1] /
                                       'scripts/metal/metal_live_flash.py'))
        self.current = '20261127T230258Z-metal'
        self.previous = '20261127T224742Z-metal'
        self.trial = '20261127T235000Z-metal'
        self.staged = '20261127T235900Z-metal'
        self.token = '1' * 32
        self.commit = '2' * 40

    def test_reclaim_through_each_host_publication_cut(self):
        slots = {'current': {'userland': self.current},
                 'previous': {'userland': self.previous}}
        trial = dict(userland=self.trial)
        staged = dict(userland=self.staged)
        names = lambda *ids: {'gen-' + value for value in ids}
        # An acknowledged trial can be left unpromoted, for example
        # when NEXT differs from the trial. Its receipt keeps three roots.
        for attempts, booted, expected in (
            ([], self.current, names(self.current, self.previous)),
            ([staged], self.current, names(self.current, self.previous, self.staged)),
            ([trial], self.trial, names(self.current, self.previous, self.trial)),
            ([trial, staged], self.trial, names(self.current, self.previous, self.trial, self.staged)),
        ):
            with self.subTest(booted=booted, attempts=attempts):
                self.assertEqual(self.flash['retained_roots'](slots, attempts, 'gen-' + booted),
                                 expected)
        promoted = {'current': trial, 'previous': slots['current']}
        self.assertEqual(self.flash['retained_roots'](promoted, [], 'gen-' + self.trial),
                         names(self.current, self.trial))

    def test_manifest_capacity_and_capability(self):
        entry = dict(commit=self.commit, sha256='3' * 64, baseNoteSha256='4' * 64,
                     userland=self.trial)
        for size, length in [(99_999_999, 256), (536_870_912, 257)]:
            with self.subTest(size=size):
                entry['size'] = size
                legacy = self.flash['manifest'](self.token, entry, 1)
                paired = self.flash['manifest'](self.token, entry, 2)
                self.assertEqual(len(legacy.splitlines()), 6)
                self.assertLessEqual(len(legacy), 255)
                self.assertEqual(paired.splitlines()[0], b'BREENIX-FLASH 2')
                self.assertEqual(paired.splitlines()[-1], ('gen-' + self.trial).encode())
                self.assertEqual(len(paired), length)

    def test_paired_install_uses_live_build_and_rejects_network_recovery(self):
        runner = runpy.run_path(str(Path(__file__).resolve().parents[1] / 'scripts/metal/metal-run'))
        scope = runner['install_persistent'].__globals__
        live = Mock(return_value=0)
        network = Mock(side_effect=AssertionError('paired deploy used the network installer'))
        scope.update(live_flash=live, boot=network)
        parser = runner['argument_parser']()
        args = parser.parse_args(['install-persistent', '--paired', '--inject', 'paired-no-commit'])
        self.assertEqual(runner['install_persistent'](args), 0)
        live.assert_called_once_with(args)
        args.recovery = True
        with self.assertRaises(SystemExit):
            runner['install_persistent'](args)


class FlashPathTest(unittest.TestCase):
    def test_kernel_flash_uses_ssh_unless_explicitly_in_recovery(self):
        runner = runpy.run_path(str(Path(__file__).resolve().parents[1] / 'scripts/metal/metal-run'))
        scope = runner['install_persistent'].__globals__
        for recovery, live_result, expected in [
                (False, 0, 'live'), (False, 1, 'live'), (True, 1, 'network')]:
            with self.subTest(recovery=recovery, live_result=live_result):
                live = Mock(return_value=live_result)
                network = Mock(return_value=17)
                scope.update(live_flash=live, boot=network)
                args = argparse.Namespace(sha=None, branch=None, rollback=False, inject=None,
                                          kernel_only=True, recovery=recovery, replace_manager=False)
                result = runner['install_persistent'](args)
                self.assertEqual(result, live_result if expected == 'live' else 17)
                self.assertEqual(live.call_count, int(expected == 'live'))
                self.assertEqual(network.call_count, int(expected == 'network'))


    def test_ssh_health_uses_privileged_pid1_inspection(self):
        module = runpy.run_path(str(Path(__file__).resolve().parents[1] / 'scripts/metal/metal_live_flash.py'))
        shell = r"""
            readlink() { return 1; }
            sudo() {
                test "$*" = '-n /bin/readlink /proc/1/exe' || return 1
                printf '%s\n' "$PID1"
            }
            cat() { printf '%s\n' '12-34-56'; }
        """
        for executable in ('/sbin/init', '/bin/sh'):
            with self.subTest(executable=executable):
                result = subprocess.run(['sh', '-c', shell + module['HEALTH']],
                                        env={'PID1': executable}, capture_output=True, text=True)
                self.assertEqual(result.returncode, int(executable != '/sbin/init'))
                self.assertEqual(result.stdout, '12-34-56\n' if executable == '/sbin/init' else '')


    def test_firmware_cold_reset_keeps_the_persistent_lease_through_off(self):
        path = Path(__file__).resolve().parents[1] / 'scripts/metal/metal-boot'
        boot = runpy.run_path(str(path))
        driver = boot['Driver'].__new__(boot['Driver'])
        driver.args = argparse.Namespace(payload='persistent')
        driver.kernel_started = None
        driver.migration_phase = None
        driver.requested_reset_at = None
        driver.seen = {'live-flash-trial', 'manager-slot'}
        driver.event = Mock()
        driver.on_slot_line = Mock()
        driver.on_slot_boot = Mock()
        driver.t0 = 1
        driver.power_seen_on = True
        driver.bmc_lock = threading.Lock()
        driver.on_manager_line(
            'breenix-manager: live flash: candidate returned (0x8000000000000003); restarting',
            observed_at=100, seconds=99)
        driver.bmc_events = [{'t': 105, 'power': 'Off'}]
        driver.pump_bmc()
        self.assertEqual(driver.requested_reset_at, 100)
        self.assertNotIn('manager-slot', driver.seen)
        driver.seen.discard('live-flash-reset')
        driver.bmc_events = [{'t': 200, 'power': 'Off'}]
        with self.assertRaises(boot['Finish']):
            driver.pump_bmc()


if __name__ == '__main__':
    unittest.main()

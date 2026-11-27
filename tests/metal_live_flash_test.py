import argparse
from pathlib import Path
import runpy
import subprocess
import threading
import unittest
from unittest.mock import Mock


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

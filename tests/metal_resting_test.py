"""Idle admission and one-shot lease regressions; no hardware is contacted."""
import os
import json
import runpy
from pathlib import Path
import sys
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import metal_tools_test as existing
import metal_persistent_test as persistent
import metal_persist

m = existing.m


class IdleTest(existing.StateTest):
    def test_resting_requires_explicit_site_opt_in(self):
        self.ready()
        metal_persist.update_disk(layout=metal_persist.new_layout())
        with patch.object(m, 'power_state', return_value='Off'), \
             patch.object(existing.machine.subprocess, 'run', return_value=SimpleNamespace(returncode=0)) as start:
            existing.machine.Machine({}).rest()
            start.assert_not_called()
            existing.machine.Machine({'BREENIX_METAL_RESTING': 'on'}).rest()
            self.assertEqual(start.call_count, 1)

    def test_restoring_other_tools_disables_the_loaded_branch_policy(self):
        self.ready()
        metal_persist.update_disk(layout=metal_persist.new_layout())
        service = existing.machine.Machine({})
        with patch.object(existing.machine.Path, 'read_bytes', return_value=b'restored tools'), \
             patch.object(existing.machine.subprocess, 'run') as start:
            service.rest()
        start.assert_not_called()

    def test_idle_starts_current_through_the_ordinary_driver_and_does_not_duplicate_it(self):
        self.ready()
        metal_persist.update_disk(layout=metal_persist.new_layout())
        service = existing.machine.Machine({})
        with patch.object(m, 'power_state', return_value='Off'), \
             patch.object(existing.machine.subprocess, 'run', return_value=SimpleNamespace(returncode=0)) as start:
            service.rest()
            service.rest()
        self.assertEqual(start.call_count, 1)
        command = start.call_args.args[0]
        self.assertIn(str(m.TOOLS / 'metal-boot'), command)
        boot_id = command[command.index('--id') + 1]
        self.assertEqual(command[command.index('run'):], ['run', '--id', boot_id, '--payload', 'persistent',
                                                        '--mode', 'persistent', '--resting', '--wait', '240'])

    def test_a_live_holder_or_queued_request_prevents_idle_admission(self):
        self.ready()
        metal_persist.update_disk(layout=metal_persist.new_layout())
        service = existing.machine.Machine({})
        with patch.object(existing.machine.subprocess, 'run') as start:
            m.claim_lease(dict(id='holder', pid=os.getpid()), wait=False)
            service.rest()
            m.release_lease('holder')
            m.write_queue([dict(id='waiter', pid=os.getpid(), deadline=m.now() + 60)])
            service.rest()
        start.assert_not_called()

    def test_operator_power_on_is_left_for_manager_adoption(self):
        self.ready()
        metal_persist.update_disk(layout=metal_persist.new_layout())
        service = existing.machine.Machine({})
        with patch.object(m, 'power_state', return_value='On'), \
             patch.object(existing.machine.subprocess, 'run') as start:
            service.rest()
        start.assert_not_called()
        self.assertIsNone(m.read_lease())

    def test_three_failed_idle_boots_disable_admission(self):
        self.ready()
        metal_persist.update_disk(layout=metal_persist.new_layout())
        service = existing.machine.Machine({})
        with patch.object(m, 'power_state', return_value='Off'), \
             patch.object(existing.machine.subprocess, 'run', return_value=SimpleNamespace(returncode=0)) as start:
            for attempt in range(3):
                service.last_resting_attempt = 0
                service.rest()
                directory = m.boot_dir(service.resting_boot)
                directory.mkdir(parents=True)
                (directory / 'ended').write_text('1')
                (directory / 'exit-status').write_text('124')
                # Distinct boot names without waiting for wall time.
                service.resting_boot = directory.name
                service.last_resting_attempt = 0
                if attempt < 2:
                    with patch.object(m, 'now', return_value=0):
                        service.rest()
                    import shutil
                    shutil.rmtree(directory)
            service.rest()
            service.rest()
        self.assertEqual(start.call_count, 3)
        self.assertEqual(service.resting_failures, 3)


class PreemptionTest(unittest.TestCase):
    def test_only_lab_resting_sessions_are_preemptible(self):
        predicate = runpy.run_path(str(existing.TOOLS / 'resting_host.py'))['preemptible']
        self.assertFalse(predicate(dict(session=True)))
        self.assertTrue(predicate(dict(session=True, resting=True)))
        self.assertFalse(predicate(dict(session=True, resting=True, upgradeParent='install')))
        self.assertFalse(predicate(dict(session=True, resting=True, run=['work'])))


class OneShotTest(unittest.TestCase):
    setUp = persistent.DriverPersistentTest.setUp
    tearDown = persistent.DriverPersistentTest.tearDown

    def test_operator_power_on_racing_idle_admission_is_not_powered_off(self):
        driver = self.driver
        driver.t0 = None
        driver.args.resting = True
        with patch.object(m, 'machine_checks', return_value=([], dict(power='On'))), \
             patch.object(m, 'force_off_and_wait') as off:
            with self.assertRaises(existing.boot.Finish) as ended:
                driver.preflight()
        self.assertEqual(ended.exception.status, m.EXIT_CANCELLED)
        self.assertIsNone(driver.t0)
        off.assert_not_called()

    def test_previous_is_consumed_before_a_reboot_in_the_same_lease(self):
        driver = self.driver
        driver.args.payload = 'persistent'
        current, previous = 'a' * 40, 'b' * 40
        driver.args.commit = previous
        metal_persist.update_disk(slots={'current': {'commit': current}, 'previous': {'commit': previous}})
        (m.STATE / 'persistent' / previous / 'definition').mkdir(parents=True)
        m.write_json(m.STATE / m.LEASE, dict(id=driver.id, state='powered', kind='persistent', entry='previous'))
        self.assertTrue(persistent.http.manager_answer().endswith(' previous'))
        driver.on_slot_boot('breenix-manager: booting previous ' + previous, 1)
        self.assertTrue(persistent.http.manager_answer().endswith(' current'))
        self.assertEqual(metal_persist.read_disk()['slots']['current']['commit'], current)



class InitialSSHTest(unittest.TestCase):
    def lab(self, replies):
        cls = runpy.run_path(str(Path(__file__).resolve().parents[1] / 'scripts/metal/server_resting.py'))['Lab']
        lab = cls.__new__(cls)
        def require(result, output=None):
            if result.returncode:
                raise ValueError('SSH failed')
            if output is not None and result.stdout.strip() != output:
                raise ValueError('unexpected SSH output')
        lab.probe = SimpleNamespace(ssh=Mock(side_effect=replies), require=require)
        return lab

    def test_initial_transport_waits_for_network_then_requires_authenticated_command(self):
        lab = self.lab([SimpleNamespace(returncode=255, stderr='Connection refused'),
                        SimpleNamespace(returncode=0, stdout='abc-def-012\n')])
        with patch('time.sleep'):
            self.assertEqual(lab.boot_id(), 'abc-def-012')
        self.assertEqual(lab.probe.ssh.call_count, 2)

    def test_idle_return_waits_for_network_before_the_operator_identity_check(self):
        lab = self.lab([SimpleNamespace(returncode=255, stderr='Connection refused'),
                        SimpleNamespace(returncode=0, stdout='abc-def-012\n'),
                        SimpleNamespace(returncode=0, stdout='1000\n')])
        lab.site = {'OPERATOR_UID': '1000'}
        lab.ended = lambda boot: True
        lab.lease = lambda: dict(id='returned', kind='persistent')
        def read(boot, name):
            return {'serial.txt': 'bsshd: listening on port\nbreenix-manager: booting current ' + 'a' * 40,
                    'ended': '1', 'run.json': json.dumps(dict(startedAt=2))}[name]
        lab.read = read
        with patch('time.sleep'):
            receipt = lab.idle('test')
        self.assertEqual(receipt['boot'], 'returned')
        self.assertTrue(receipt['authenticated'])
        self.assertEqual(lab.probe.ssh.call_count, 3)

    def test_authentication_failure_is_not_treated_as_network_readiness(self):
        lab = self.lab([SimpleNamespace(returncode=255, stderr='Permission denied (publickey).')])
        with self.assertRaises(ValueError):
            lab.boot_id()
        self.assertEqual(lab.probe.ssh.call_count, 1)

if __name__ == '__main__':
    unittest.main()

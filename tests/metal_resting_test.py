"""Idle admission and one-shot lease regressions; no hardware is contacted."""
import os
from pathlib import Path
import sys
from types import SimpleNamespace
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import metal_tools_test as existing
import metal_persistent_test as persistent
import metal_persist

m = existing.m


class IdleTest(existing.StateTest):
    def test_idle_starts_current_through_the_ordinary_driver_and_does_not_duplicate_it(self):
        self.ready()
        metal_persist.update_disk(layout=metal_persist.new_layout())
        service = existing.machine.Machine({})
        with patch.object(existing.machine.subprocess, 'run', return_value=SimpleNamespace(returncode=0)) as start:
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


class OneShotTest(unittest.TestCase):
    setUp = persistent.DriverPersistentTest.setUp
    tearDown = persistent.DriverPersistentTest.tearDown

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


if __name__ == '__main__':
    unittest.main()

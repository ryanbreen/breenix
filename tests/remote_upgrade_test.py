"""Exercise the actual install driver's S21 verification and rollback control flow."""
from pathlib import Path
import sys
from types import SimpleNamespace
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import metal_tools_test as existing


class UpgradeDriver(existing.StateTest):
    def driver(self):
        args = SimpleNamespace(id='20261023T000000Z-metal', mode='persistent', payload='persistent',
                               session=False, target='main', task='', commit='a' * 40, inject=None,
                               nmi=False, verify=True, run=None, upgrade_parent='20261022T000000Z-metal')
        with patch.object(existing.m, 'load_site', return_value={}):
            driver = existing.boot.Driver(args)
        driver.dir.mkdir(parents=True, exist_ok=True)
        return driver

    def test_login_prompt_does_not_finish_verification_without_ssh(self):
        driver = self.driver()
        driver.final_marker = 'Breenix login: '
        driver.final_seen = True
        driver.required_stages = []
        driver.seen.add('manager-slot')
        # The prompt and required stages are insufficient without the host SSH probe.
        driver.maybe_complete(existing.m.now(), 50)
        self.assertFalse(driver.completion_reported)

    def test_receipt_requires_authentication_strict_key_and_services(self):
        driver = self.driver()
        driver.t0 = 1000
        driver.args.expected_generation = 'new'
        receipt = dict(boot=driver.id, powerOn=1000, completedAt=1100, passed=True, generation='new',
                       authenticated=True, strictHostKey=True, servicesStarted=True)
        existing.m.write_json(driver.dir / 'ssh-verification.json', receipt)
        self.assertTrue(driver.ssh_verified())
        for field in ('authenticated', 'strictHostKey', 'servicesStarted'):
            existing.m.write_json(driver.dir / 'ssh-verification.json', dict(receipt, **{field: False}))
            with self.subTest(field=field), self.assertRaises(existing.boot.Finish):
                driver.ssh_verified()

    def test_failed_probe_boots_previous_under_inherited_lease(self):
        driver = self.driver()
        existing.m.write_json(existing.m.STATE / 'disk.json', {'slots': {
            'current': {'commit': 'a' * 40, 'installedBy': 'old'},
            'next': {'commit': 'a' * 40, 'installedBy': 'new'}}})
        attempts = []
        def run(child, inherited=None):
            child.dir.mkdir(parents=True, exist_ok=True)
            child.on_slot_line('breenix-manager: slot ' + child.args.slot_action + ': current ' + 'a' * 40 + ', previous ' + 'a' * 40, existing.m.now())
            attempts.append((child.args.slot_action, inherited))
            if len(attempts) == 1:
                existing.m.write_json(child.dir / 'ssh-verification-progress.json', dict(sshAttempts=9))
            return 1 if len(attempts) == 1 else 0
        with patch.object(existing.boot.Driver, 'run', run), patch.object(existing.m, 'load_site', return_value={}):
            self.assertEqual(driver.continue_verification(), 0)
        self.assertEqual([action for action, _ in attempts], ['commit', 'rollback'])
        self.assertEqual(attempts[0][1], driver.id)
        self.assertEqual(existing.m.read_json(driver.dir / 'upgrade-result.json')['verification']['sshAttempts'], 9)
        self.assertTrue(attempts[1][1])
        self.assertEqual(existing.m.read_json(existing.m.STATE / 'disk.json')['slots']['current']['installedBy'], 'old')


if __name__ == '__main__':
    unittest.main()

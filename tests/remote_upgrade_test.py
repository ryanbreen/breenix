"""Exercise the actual install driver's S21 verification and rollback control flow."""
from pathlib import Path
import runpy
import sys
from types import SimpleNamespace
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import metal_tools_test as existing


class UpgradeSuiteStop(unittest.TestCase):
    def test_requested_stop_waits_for_finalization_and_other_failures_remain_errors(self):
        module = runpy.run_path(str(Path(__file__).resolve().parents[1] / 'scripts/metal/upgrade_suite.py'))
        suite = module['UpgradeSuite'].__new__(module['UpgradeSuite'])
        suite.active = 'own-boot'
        suite.console = None
        suite.host = {'command': lambda _: SimpleNamespace(returncode=0), '__file__': '/tmp/server-suite'}
        reads = iter(['', '143', 'finished'])
        suite.read = lambda *args, **kwargs: next(reads)
        with patch.object(module['time'], 'sleep'):
            suite.stop()
        self.assertIsNone(suite.active)
        suite.read = lambda boot, name, **kwargs: '124' if name == 'exit-status' else ''
        with self.assertRaisesRegex(ValueError, '124'):
            suite.wait('own-boot', 'ended', allowed_status=('0', '143'))


class UpgradeWorker(unittest.TestCase):
    def test_install_ended_does_not_end_child_probe_and_worker_failure_retries(self):
        import json
        module = runpy.run_path(str(Path(__file__).resolve().parents[1] / 'scripts/metal/upgrade_probe.py'))
        calls = []
        def remote(env, *cmd, **kw):
            boot, name = Path(cmd[-1]).parts[-2:]
            value = ''
            if name == 'events.jsonl':
                value = json.dumps(dict(event='persistent-verification-next', verification='child') if boot == 'install'
                                   else dict(event='power-on', t=module['time'].time()))
            elif name == 'ended' and boot == 'install': value = 'phase ended'
            elif name in ('ssh-verification.json', 'upgrade-result.json') and len(calls) == 2: value = '{}'
            return SimpleNamespace(returncode=0, stdout=value)
        runner = dict(host_env=lambda: {}, in_metal=remote, STATE='/state')
        def run(*args, **kwargs): calls.append(args); return SimpleNamespace(returncode=1 if len(calls)==1 else 0)
        with patch.object(module['subprocess'], 'run', side_effect=run), patch.object(module['time'], 'sleep'):
            self.assertEqual(module['watch'](runner, 'install'), 0)
        self.assertEqual(len(calls), 2)


class UpgradeAssertions(unittest.TestCase):
    def test_bootable_fault_requires_booted_console_and_actual_ssh_failure(self):
        cases = runpy.run_path(str(Path(__file__).resolve().parents[1] / 'scripts/metal/upgrade_cases.py'))
        baseline = dict(commit='a'*40, fingerprint='pin', rootSentinel='root', homeSentinel='home')
        result = dict(outcome='rolled-back', toolsCommit=baseline['commit'], candidateGeneration='new',
                      previousGeneration='old', candidateBooted=True,
                      verification=dict(sshAttempts=3, authenticated=False),
                      rollback=dict(generation='old', fingerprint='pin', authenticated=True, strictHostKey=True,
                                    servicesStarted=True, powerOn=1000, completedAt=1100),
                      observations=[baseline, baseline])
        cases['upgrade'](result, True, baseline, bootable=True)
        for changes, message in ((dict(candidateBooted=False), 'never booted'),
                                 (dict(verification=dict(sshAttempts=0)), 'no actual SSH')):
            with self.assertRaisesRegex(ValueError, message):
                cases['upgrade'](dict(result, **changes), True, baseline, bootable=True)


class UpgradePinRecovery(unittest.TestCase):
    def test_stale_pin_recovers_only_through_console_and_strict_authenticated_probe(self):
        import hashlib
        import json
        import tempfile
        module = runpy.run_path(str(Path(__file__).resolve().parents[1] / 'scripts/metal/server-suite'))
        fn = module['probe_upgrade_attempt']
        with tempfile.TemporaryDirectory() as scratch:
            home = Path(scratch)
            (home / '.ssh').mkdir()
            (home / '.ssh/operator.pub').write_text('ssh-ed25519 public')
            (home / '.ssh/operator').touch()
            state = home / '.local/state/breenix-metal'
            state.mkdir(parents=True)
            cache = state / ('pinned-hostkey-' + hashlib.sha256(b'system:host').hexdigest() + '.json')
            cache.write_text(json.dumps(dict(known='old', fingerprint='old')))
            calls = []
            now = module['time'].time()
            boot_id = '20261023T000000Z-metal'
            def remote(env, *cmd, **kw):
                text = ' '.join(cmd)
                if cmd[0] == 'python3':
                    value = (dict(partitions=dict(system=dict(guid='system'))) if 'read_disk' in text else
                             dict(SERVER_SSH_HOST='host', OPERATOR_USER='operator', OPERATOR_UID='1000',
                                  OPERATOR_SSH_PUBLIC_KEY='ssh-ed25519 public', CONSOLE_USER='root', CONSOLE_PASSWORD='secret'))
                    return SimpleNamespace(returncode=0, stdout=json.dumps(value))
                name = Path(cmd[-1]).name
                values = {'run.json': json.dumps(dict(upgradeParent='parent')),
                          'lease.json': json.dumps(dict(id=boot_id)),
                          'events.jsonl': json.dumps(dict(event='power-on', t=now-1)),
                          'serial.txt': 'Jumping to kernel entry point at\nBreenix login: '}
                return SimpleNamespace(returncode=0, stdout=values[name])
            class FakeProbe:
                def __init__(self, host, user, identity, directory):
                    self.known = directory / 'known_hosts'
                def ssh(self, command, **kw):
                    calls.append((command, self.known.read_text()))
                    return SimpleNamespace(returncode=int(self.known.read_text() != 'new'),
                                           stdout={'id -u':'1000', 'cat /usr/share/factory/generation':'generation',
                                                   'cat /run/breenix/services':'services'}[command])
                def run_case(self, name, log):
                    if log != 'trusted': raise ValueError('untrusted console')
                    self.known.write_text('new')
                def require(self, result, expected=None):
                    if result.returncode: raise ValueError('strict SSH failed')
            class FakeConsole:
                def __init__(self, *args): pass
                def service_log(self, name): return 'trusted'
                def logout(self): pass
            runner = dict(host_env=lambda: {}, in_metal=remote, STATE='/state')
            with patch.dict(fn.__globals__, Probe=FakeProbe, Console=FakeConsole,
                            parse_state=lambda _: ({}, {name: dict(starts=1, state='running', pid=1)
                                                       for name in module['Services'].REQUIRED})), \
                 patch.object(module['runpy'], 'run_path', return_value=runner), \
                 patch.object(Path, 'home', return_value=home), \
                 patch.object(module['re'], 'search', return_value=['', 'SHA256:new']):
                result = fn(SimpleNamespace(boot=boot_id))
            self.assertTrue(result['passed'], result)
            self.assertEqual(calls[:2], [('id -u', 'old'), ('id -u', 'new')])
            self.assertEqual(json.loads(cache.read_text())['known'], 'new')


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

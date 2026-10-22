"""The host server scorer refuses incomplete or mismatched machine evidence."""
import copy
import json
from pathlib import Path
import runpy
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch
from types import SimpleNamespace

sys.path.insert(0, str(Path(__file__).resolve().parent))
import metal_tools_test as existing

ROOT = Path(__file__).resolve().parents[1]
host = runpy.run_path(str(ROOT / 'scripts/metal/server-suite'))


class PersistentServerRecord(existing.StateTest):
    def test_persistent_session_record_can_supply_server_evidence(self):
        sha = 'a' * 40
        args = SimpleNamespace(id='20261021T000000Z-metal', mode='persistent', payload='persistent',
                               session=False, target='main', task='', commit=sha, inject=None, nmi=False)
        with patch.object(existing.m, 'load_site', return_value={}):
            driver = existing.boot.Driver(args)
        driver.create_record()
        record = existing.m.read_json(driver.dir / 'run.json')
        self.assertTrue(record['session'])
        self.assertTrue(record['evidenceEligible'])
        self.assertEqual(record['kind'], 'persistent')
        record.update(bootedCommit=sha, toolsCommit=sha)
        manifest = json.loads((ROOT / 'docs/suites/server.json').read_text())
        path = json.loads((ROOT / 'docs/efforts/path.json').read_text())
        records = dict(boot=record['id'], commit=sha, milestone='server/ssh-keys', cleanup=True,
                       cases=[dict(id='ssh-keys/' + name, result='PASS') for name, _ in host['CATEGORIES']['ssh-keys']])
        serial = "[stdout] bsshd: host key ED25519 SHA256:example (loaded)\nbsshd: refused method 'password'\n"
        self.assertTrue(host['score'](manifest, path, records, serial, record, sha, []).startswith('PASS: server/ssh-keys'))


class HostServerScore(unittest.TestCase):
    def setUp(self):
        self.manifest = json.loads((ROOT / 'docs/suites/server.json').read_text())
        self.path = json.loads((ROOT / 'docs/efforts/path.json').read_text())
        self.sha = 'a' * 40
        self.boot = dict(id='20261021T000000Z-metal', mode='persistent', bootedCommit=self.sha, toolsCommit=self.sha, evidenceEligible=True)
        self.records = dict(boot=self.boot['id'], commit=self.sha, milestone='server/ssh-keys', cleanup=True,
                            cases=[dict(id='ssh-keys/' + name, result='PASS') for name, _ in host['CATEGORIES']['ssh-keys']])
        self.serial = "[stdout] bsshd: host key ED25519 SHA256:example (loaded)\nbsshd: refused method 'password'\n"

    def test_complete_host_evidence_and_rejections(self):
        def score():
            return host['score'](self.manifest, self.path, self.records, self.serial, self.boot, self.sha, ['KERNEL PANIC:'])
        self.assertTrue(score().startswith('PASS: server/ssh-keys'))
        good = copy.deepcopy(self.records)
        for bad in [[], good['cases'][:-1], good['cases'] + good['cases'][:1], list(reversed(good['cases']))]:
            self.records['cases'] = bad
            with self.assertRaises(ValueError): score()
        self.records = copy.deepcopy(good)
        self.records['cases'][0]['result'] = 'FAIL'
        with self.assertRaises(ValueError): score()
        self.records = copy.deepcopy(good)
        self.records['cleanup'] = False
        with self.assertRaises(ValueError): score()
        self.records = good
        self.boot['bootedCommit'] = 'b' * 40
        with self.assertRaises(ValueError): score()
        self.boot['bootedCommit'] = self.sha
        self.boot['toolsCommit'] = 'b' * 40
        with self.assertRaises(ValueError): score()
        self.boot['toolsCommit'] = self.sha
        serial = self.serial
        self.serial = '[stdout] bsshd: host key ED25519 SHA256:example (loaded)\n'
        with self.assertRaises(ValueError): score()
        self.serial = serial + 'KERNEL PANIC: example\n'
        with self.assertRaises(ValueError): score()

    def test_host_key_rejects_embedded_fingerprint_and_requires_complete_record(self):
        with tempfile.TemporaryDirectory() as scratch:
            probe = object.__new__(host['Probe'])
            probe.host = 'fixture'
            probe.known = Path(scratch) / 'known_hosts'
            replies = [SimpleNamespace(returncode=0, stdout='fixture ssh-ed25519 key\n'),
                       SimpleNamespace(returncode=0, stdout='256 SHA256:forged fixture (ED25519)\n')]
            forged = "[stdout] bsshd: refused method 'password' for 'bsshd: host key ED25519 SHA256:forged'\n"
            with patch.dict(host['Probe'].run_case.__globals__, command=lambda *a, **kw: replies.pop(0)):
                with self.assertRaisesRegex(ValueError, 'fingerprint differs'):
                    probe.run_case('host-key', forged)
            self.assertFalse(probe.known.exists())
            replies[:] = [SimpleNamespace(returncode=0, stdout='fixture ssh-ed25519 key\n'),
                          SimpleNamespace(returncode=0, stdout='256 SHA256:trusted fixture (ED25519)\n')]
            with patch.dict(host['Probe'].run_case.__globals__, command=lambda *a, **kw: replies.pop(0)):
                probe.run_case('host-key', forged + '[stdout] bsshd: host key ED25519 SHA256:trusted (loaded)\n')
            self.assertEqual(probe.known.read_text(), 'fixture ssh-ed25519 key\n')

    def test_logs_require_all_cases_and_prior_boot_at_same_commit(self):
        records = dict(self.records, milestone='server/logs',
                       cases=[dict(id='logs/' + name, result='PASS') for name, _ in host['LOGS']])
        def score():
            return host['score'](self.manifest, self.path, records, self.serial, self.boot, self.sha, [])
        with self.assertRaises(ValueError): score()
        records['previous'] = dict(commit='b' * 40)
        with self.assertRaises(ValueError): score()
        records['previous'] = dict(commit=self.sha)
        self.assertTrue(score().startswith('PASS: server/logs'))
        records['cases'][-1]['result'] = 'FAIL'
        with self.assertRaises(ValueError): score()

    def test_cleanup_restores_exact_keys_without_guest_mktemp(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            ssh = root / '.ssh'
            ssh.mkdir()
            target = ssh / 'authorized_keys'
            original = '# retained comment\nprimary key\nsecond key without final newline'
            target.write_text(original + '\ntemporary probe key\n')
            binaries = root / 'bin'
            binaries.mkdir()
            for name in ('cat', 'mv'):
                (binaries / name).symlink_to(shutil.which(name))
            probe = object.__new__(host['Probe'])
            probe.original = original
            probe.ssh = lambda script: subprocess.run(
                ['/bin/sh', '-c', script], capture_output=True, text=True,
                env=dict(os.environ, HOME=str(root), PATH=str(binaries)))
            probe.cleanup()
            self.assertEqual(target.read_text(), original)
            self.assertEqual(target.stat().st_mode & 0o777, 0o600)
            self.assertEqual(list(ssh.iterdir()), [target])

    def test_malformed_records_fail_this_milestone_without_crashing_the_boot_scorer(self):
        sys.path.insert(0, str(ROOT / 'scripts/metal'))
        score_boot = runpy.run_path(str(ROOT / 'scripts/metal/metal_score.py'))['score']
        with tempfile.TemporaryDirectory() as scratch:
            directory = Path(scratch)
            def write(name, value):
                path = directory / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(json.dumps(value))
            write('run.json', self.boot | dict(disk='persistent'))
            write('definition/docs/metal/boot-path.json', dict(gateArch='x86_64', milestones=[]))
            write('definition/tools/breenix-runs/Sources/BreenixRuns/Resources/boot-stages-x86_64.json', dict(stages=[]))
            write('definition/docs/suites/server.json', self.manifest)
            write('definition/docs/efforts/path.json', self.path)
            write('definition/docs/boot-path.json', dict(fatal=[]))
            source = directory / 'definition/scripts/metal/server-suite'
            source.parent.mkdir(parents=True, exist_ok=True)
            source.write_text((ROOT / 'scripts/metal/server-suite').read_text())
            (directory / 'serial.txt').write_text(self.serial)
            (directory / 'events.jsonl').write_text('')
            write('server-ssh-keys.json', self.records | dict(cases=None))
            write('server-services.json', self.records | dict(milestone='server/services', cases=None))
            write('server-services-transcript.json', [])
            verdict = score_boot(directory)
            self.assertFalse(verdict['milestones']['server/ssh-keys']['passed'])
            self.assertFalse(verdict['milestones']['server/services']['passed'])
            self.assertNotIn('server/services-transcript', verdict['milestones'])
            self.assertTrue(verdict['captureComplete'])

    def test_services_evidence_needs_its_cases_cleanup_and_console_lines(self):
        records = dict(self.records, milestone='server/services',
                       cases=[dict(id='services/' + name, result='PASS') for name, _ in host['CATEGORIES']['services']])
        serial = ('[stdout] bsshd: host key ED25519 SHA256:example (loaded)\n[init] bsshd killed by signal 9\n'
                  '[init] svc-suite-fail is crash-looping: 5 failures in a row, each within 10 s of starting\n')
        def score(records=records, serial=serial):
            return host['score'](self.manifest, self.path, records, serial, self.boot, self.sha, ['KERNEL PANIC:'])
        self.assertTrue(score().startswith('PASS: server/services categories=services'))
        # ssh-keys results do not complete the services milestone, nor the reverse.
        with self.assertRaises(ValueError): score(dict(records, cases=self.records['cases']))
        with self.assertRaises(ValueError): score(dict(self.records, milestone='server/services'))
        with self.assertRaises(ValueError): score(dict(records, milestone='server/logs'))
        with self.assertRaises(ValueError): score(dict(records, cleanup=False))
        for line in serial.splitlines()[1:]:
            with self.assertRaises(ValueError): score(serial=serial.replace(line + '\n', ''))
        failed = copy.deepcopy(records)
        failed['cases'][3]['result'] = 'FAIL'
        with self.assertRaises(ValueError): score(failed)

    def test_fixtures_are_written_as_console_literals(self):
        with tempfile.TemporaryDirectory() as scratch:
            target = Path(scratch) / 'services'
            commands = []
            def console_run(command):
                commands.append(command)
                self.assertNotIn('\n', command)
                if command == 'svc reload': return 'svc: reloaded 6 added'
                subprocess.run(['/bin/sh', '-c', command], check=True)
                return ''
            probe = SimpleNamespace(user='service-user')
            console = SimpleNamespace(size=lambda: 0, run=console_run)
            services = host['Services'](probe, console, None)
            services.fixture_file = str(target)
            services.ensure_fixtures()
            table = target.read_text()
            self.assertEqual(table.count('user = service-user\n'), 5)
            self.assertIn('trap "" TERM; exec /bin/sleep 3600', table)
            self.assertIn('user = svc-suite-absent\n', table)
            self.assertEqual(len(commands), 2)

    def test_reconnect_rejects_success_after_deadline(self):
        services = object.__new__(host['Services'])
        services.state = lambda: ({}, {'bsshd': {'pid': 42}})
        services.console = SimpleNamespace(size=lambda: 0, login=lambda: None,
                                          type_line=lambda line: None, wait=lambda *args: None)
        clock = [0.0]
        timeouts = []
        def ssh(script, timeout):
            timeouts.append(timeout)
            clock[0] = 11.0
            return SimpleNamespace(returncode=0, stdout='1000\n')
        services.probe = SimpleNamespace(ssh=ssh, transcript=[])
        with patch.object(host['time'], 'monotonic', side_effect=lambda: clock[0]):
            with self.assertRaisesRegex(ValueError, 'within 10 s'):
                services.case_kill_restart()
        self.assertEqual(timeouts, [10.0])
        self.assertEqual(services.probe.transcript, [])

    def test_operator_category_is_scored_against_the_users_manifest(self):
        users = json.loads((ROOT / 'docs/suites/users.json').read_text())
        host['validate_manifest'](users)
        records = dict(self.records, milestone='users/operator-login',
                       cases=[dict(id='operator/' + name, result='PASS') for name, _ in host['OPERATOR']])
        serial = self.serial + ''.join(line + '\n' for line in host['OPERATOR_CONSOLE_LINES'])
        def score(manifest=users, serial=serial):
            return host['score'](manifest, self.path, records, serial, self.boot, self.sha, ['KERNEL PANIC:'])
        self.assertTrue(score().startswith('PASS: users/operator-login categories=operator cases=6'))
        with self.assertRaisesRegex(ValueError, 'users manifest'):
            score(self.manifest)
        for line in host['OPERATOR_CONSOLE_LINES']:
            with self.subTest(line=line), self.assertRaisesRegex(ValueError, 'no line'):
                score(serial=serial.replace(line + '\n', ''))
        records['cases'][2]['result'] = 'FAIL'
        with self.assertRaises(ValueError): score()
        # A host-run category whose cases drift from the runner's table is refused.
        drifted = copy.deepcopy(users)
        next(c for c in drifted['categories'] if c['id'] == 'operator')['cases'].pop()
        with self.assertRaises(ValueError): host['validate_manifest'](drifted)

    def test_boot_scorer_reads_operator_records_and_leaves_other_users_files(self):
        sys.path.insert(0, str(ROOT / 'scripts/metal'))
        score_boot = runpy.run_path(str(ROOT / 'scripts/metal/metal_score.py'))['score']
        with tempfile.TemporaryDirectory() as scratch:
            directory = Path(scratch)
            def write(name, value):
                path = directory / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(json.dumps(value))
            write('run.json', self.boot | dict(disk='persistent'))
            write('definition/docs/metal/boot-path.json', dict(gateArch='x86_64', milestones=[]))
            write('definition/tools/breenix-runs/Sources/BreenixRuns/Resources/boot-stages-x86_64.json', dict(stages=[]))
            write('definition/docs/suites/users.json', json.loads((ROOT / 'docs/suites/users.json').read_text()))
            write('definition/docs/efforts/path.json', self.path)
            write('definition/docs/boot-path.json', dict(fatal=[]))
            source = directory / 'definition/scripts/metal/server-suite'
            source.parent.mkdir(parents=True, exist_ok=True)
            source.write_text((ROOT / 'scripts/metal/server-suite').read_text())
            (directory / 'serial.txt').write_text(self.serial + ''.join(line + '\n' for line in host['OPERATOR_CONSOLE_LINES']))
            (directory / 'events.jsonl').write_text('')
            write('users-operator.json', self.records | dict(milestone='users/operator-login',
                  cases=[dict(id='operator/' + name, result='PASS') for name, _ in host['OPERATOR']]))
            write('users-operator-transcript.json', [])
            write('users-hostssh.json', dict(boot=self.boot['id'], requests=[]))
            verdict = score_boot(directory)
            self.assertEqual(list(verdict['milestones']), ['users/operator-login'])
            self.assertTrue(verdict['milestones']['users/operator-login']['passed'])

    def test_missing_category_and_manifest_drift_block_acceptance(self):
        self.manifest['categories'][0]['cases'] = []
        with self.assertRaises(ValueError):
            host['score'](self.manifest, self.path, self.records, self.serial, self.boot, self.sha, ['KERNEL PANIC:'])


if __name__ == '__main__':
    unittest.main()

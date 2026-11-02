"""Exercise retained A2 scoring and host admission refusals without a VM."""
import hashlib
import json
from pathlib import Path
import runpy
import tempfile
import subprocess
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
HOST = runpy.run_path(str(ROOT / 'scripts/metal/server-suite'))
SHA = 'a' * 40
BOOT = '20261101T120000Z-qemu'
OTHER = '20261101T110000Z-qemu'


class ArmA2Capture(unittest.TestCase):
    def setUp(self):
        self.host = runpy.run_path(str(ROOT / 'scripts/check-arm-a2-capture.py'))
        self.audit = self.host['audit']
        # Synthetic capture identities isolate scoring from Git history here.
        self.delta = dict(ancestor=True, changedPaths=[], runtimeUnchanged=True)
        provenance = patch.dict(self.audit.__globals__,
                                capture_delta=lambda *args: dict(self.delta))
        provenance.start()
        self.addCleanup(provenance.stop)
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.run = Path(self.tmp.name)
        for boot, sha in ((BOOT, SHA), (OTHER, 'b' * 40)):
            directory = self.run / 'boots' / boot
            directory.mkdir(parents=True)
            (directory / 'run.json').write_text(json.dumps(dict(id=boot, mode='persistent',
                bootedCommit=sha, toolsCommit=sha, evidenceEligible=True, target='qemu', artifacts={name: 'a' * 64
                    for name in ('kernel', 'system.img', 'home.img')})))
            (directory / 'ended').write_text('2026-11-01T12:01:00Z\n')
            (directory / 'logs-prepared.json').write_text(json.dumps(dict(boot=boot, commit=sha)))
            for name in ('docs/suites/server.json', 'docs/efforts/path.json', 'docs/boot-path.json'):
                path = directory / 'definition' / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes((ROOT / name).read_bytes())
        self.directory = self.run / 'boots' / BOOT
        (self.directory / 'serial.txt').write_text(
            '[stdout] bsshd: host key ED25519 SHA256:fixture (loaded)\n'
            "bsshd: refused method 'password'\n"
            '[init] svc-suite-fail is crash-looping: 5 failures in a row\n'
            '1000 [init] bsshd killed by signal 9\n')
        for category in ('ssh-keys', 'services', 'logs', 'time', 'console-input'):
            records = dict(boot=BOOT, commit=SHA, milestone='server/' + category,
                cases=[dict(id=category + '/' + case, result='PASS', ms=1)
                       for case, _ in HOST['CATEGORIES'][category]],
                cleanup=True, consoleGaps=[], previous=dict(commit=SHA))
            self.record(category).write_text(json.dumps(records))

    def record(self, category):
        return self.directory / ('server-' + category + '.json')

    def digest(self):
        return {str(p.relative_to(self.run)): hashlib.sha256(p.read_bytes()).hexdigest()
                for p in self.run.rglob('*') if p.is_file()}

    def test_scores_and_retains_refusal_receipts_without_mutating_capture(self):
        before = self.digest()
        result = self.audit(self.run, SHA, BOOT, OTHER)
        self.assertTrue(result['proven'], result)
        self.assertEqual(result['boots'], [BOOT])
        self.assertTrue(result['captureDelta']['runtimeUnchanged'])
        self.assertEqual({r['category']: r['passed'] for r in result['categories']},
                         {'ssh-keys': 14, 'services': 9, 'logs': 6, 'time': 6, 'console-input': 1})
        self.assertEqual(len(result['controls']), 2)
        for receipt, reason in zip(result['controls'], (
                'this boot does not hold the machine lease',
                'previous log preparation must be a completed persistent boot at this commit')):
            self.assertEqual(receipt['exitStatus'], 1)
            self.assertIn('ValueError: ' + reason, receipt['stderr'])
            self.assertTrue(receipt['passed'])
        self.assertEqual(result['controls'][1]['previousCommit'], 'b' * 40)
        self.assertEqual(self.digest(), before)
        for name, digest in before.items():
            self.assertEqual(result['captureSha256'][name], digest)

    def test_unhashed_legacy_head_label_is_not_accepted(self):
        path = self.directory / 'run.json'
        identity = json.loads(path.read_text())
        del identity['artifacts']
        path.write_text(json.dumps(identity))
        result = self.audit(self.run, SHA, BOOT, OTHER)
        self.assertFalse(result['proven'])
        self.assertIn('old HEAD labels are insufficient', ' '.join(result['reasons']))

    def test_changed_runtime_prevents_applying_retained_results_to_head(self):
        self.delta.update(runtimeUnchanged=False, changedPaths=['kernel/src/lib.rs'])
        result = self.audit(self.run, SHA, BOOT, OTHER)
        self.assertFalse(result['proven'])
        self.assertIn('runtime changed', ' '.join(result['reasons']))

    def test_missing_category_and_failed_case_are_reported(self):
        self.record('services').unlink()
        path = self.record('ssh-keys')
        records = json.loads(path.read_text())
        records['cases'][0]['result'] = 'FAIL'
        path.write_text(json.dumps(records))
        result = self.audit(self.run, SHA, BOOT, OTHER)
        self.assertFalse(result['proven'])
        self.assertIn('services', ' '.join(result['reasons']))
        self.assertIn('one or more ssh-keys cases failed', ' '.join(result['reasons']))

    def test_cross_commit_control_requires_genuine_consistent_preparation(self):
        path = self.run / 'boots' / OTHER / 'logs-prepared.json'
        path.write_text(json.dumps(dict(boot=OTHER, commit=SHA)))
        result = self.audit(self.run, SHA, BOOT, OTHER)
        self.assertFalse(result['proven'])
        self.assertIn('preparation identity', ' '.join(result['reasons']))

    def test_refuses_an_active_run_and_wrong_capture_commit(self):
        (self.run / 'boot.lock').mkdir()
        with self.assertRaisesRegex(ValueError, 'active'):
            self.audit(self.run, SHA, BOOT, OTHER)
        (self.run / 'boot.lock').rmdir()
        result = self.audit(self.run, 'c' * 40, BOOT, OTHER)
        self.assertFalse(result['proven'])
        self.assertIn('capture commit', ' '.join(result['reasons']))


class CaptureDelta(unittest.TestCase):
    def test_git_diff_accepts_only_the_host_auditor_and_its_check(self):
        host = runpy.run_path(str(ROOT / 'scripts/check-arm-a2-capture.py'))
        with tempfile.TemporaryDirectory() as name:
            repo = Path(name)
            def git(*args):
                return subprocess.check_output(['git', '-C', str(repo), *args],
                                               text=True, stderr=subprocess.PIPE).strip()
            git('init', '-q')
            git('config', 'user.name', 'Capture fixture')
            git('config', 'user.email', 'fixture@example.invalid')
            paths = ('kernel/src/lib.rs', 'scripts/check-arm-a2-capture.py',
                     'tests/arm_a2_capture_test.py')
            for path in paths:
                f = repo / path
                f.parent.mkdir(parents=True, exist_ok=True)
                f.write_text('capture\n')
            git('add', '.')
            git('commit', '-qm', 'Capture fixture')
            captured = git('rev-parse', 'HEAD')
            for path in paths[1:]:
                (repo / path).write_text('host scoring repair\n')
            git('add', '.')
            git('commit', '-qm', 'Change host auditor')
            delta = host['capture_delta'](captured, git('rev-parse', 'HEAD'), repo)
            self.assertTrue(delta['ancestor'])
            self.assertTrue(delta['runtimeUnchanged'])
            self.assertEqual(delta['changedPaths'], sorted(paths[1:]))
            (repo / paths[0]).write_text('changed guest\n')
            git('add', '.')
            git('commit', '-qm', 'Change guest')
            delta = host['capture_delta'](captured, git('rev-parse', 'HEAD'), repo)
            self.assertFalse(delta['runtimeUnchanged'])
            self.assertIn(paths[0], delta['changedPaths'])
            reverse = host['capture_delta'](git('rev-parse', 'HEAD'), captured, repo)
            self.assertFalse(reverse['ancestor'])
            self.assertFalse(reverse['runtimeUnchanged'])


if __name__ == '__main__':
    unittest.main()

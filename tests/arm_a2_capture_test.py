"""Exercise retained A2 scoring and host admission refusals without a VM."""
import hashlib
import json
from pathlib import Path
import runpy
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
HOST = runpy.run_path(str(ROOT / 'scripts/metal/server-suite'))
SHA = 'a' * 40
BOOT = '20261101T120000Z-qemu'
OTHER = '20261101T110000Z-qemu'


class ArmA2Capture(unittest.TestCase):
    def setUp(self):
        self.audit = runpy.run_path(str(ROOT / 'scripts/check-arm-a2-capture.py'))['audit']
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.run = Path(self.tmp.name)
        for boot, sha in ((BOOT, SHA), (OTHER, 'b' * 40)):
            directory = self.run / 'boots' / boot
            directory.mkdir(parents=True)
            (directory / 'run.json').write_text(json.dumps(dict(id=boot, mode='persistent',
                bootedCommit=sha, toolsCommit=sha, evidenceEligible=True, target='qemu')))
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


if __name__ == '__main__':
    unittest.main()

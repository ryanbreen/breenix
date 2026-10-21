"""The host server scorer refuses incomplete or mismatched machine evidence."""
import copy
import json
from pathlib import Path
import runpy
import unittest

ROOT = Path(__file__).resolve().parents[1]
host = runpy.run_path(str(ROOT / 'scripts/metal/server-suite'))


class HostServerScore(unittest.TestCase):
    def setUp(self):
        self.manifest = json.loads((ROOT / 'docs/suites/server.json').read_text())
        self.path = json.loads((ROOT / 'docs/efforts/path.json').read_text())
        self.sha = 'a' * 40
        self.boot = dict(id='20261021T000000Z-metal', mode='persistent', bootedCommit=self.sha, evidenceEligible=True)
        self.records = dict(boot=self.boot['id'], commit=self.sha, milestone='server/ssh-keys', cleanup=True,
                            cases=[dict(id='ssh-keys/' + name, result='PASS') for name, _ in host['CASES']],
                            fatalPatterns=['KERNEL PANIC:'])
        self.serial = 'bsshd: host key ED25519 SHA256:example\n'

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
        self.serial += 'KERNEL PANIC: example\n'
        with self.assertRaises(ValueError): score()

    def test_missing_category_and_manifest_drift_block_acceptance(self):
        self.manifest['categories'][0]['cases'] = []
        with self.assertRaises(ValueError):
            host['score'](self.manifest, self.path, self.records, self.serial, self.boot, self.sha, ['KERNEL PANIC:'])


if __name__ == '__main__':
    unittest.main()

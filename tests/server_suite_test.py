"""The host server scorer refuses incomplete or mismatched machine evidence."""
import copy
import json
from pathlib import Path
import runpy
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
host = runpy.run_path(str(ROOT / 'scripts/metal/server-suite'))


class HostServerScore(unittest.TestCase):
    def setUp(self):
        self.manifest = json.loads((ROOT / 'docs/suites/server.json').read_text())
        self.path = json.loads((ROOT / 'docs/efforts/path.json').read_text())
        self.sha = 'a' * 40
        self.boot = dict(id='20261021T000000Z-metal', mode='persistent', bootedCommit=self.sha, toolsCommit=self.sha, evidenceEligible=True)
        self.records = dict(boot=self.boot['id'], commit=self.sha, milestone='server/ssh-keys', cleanup=True,
                            cases=[dict(id='ssh-keys/' + name, result='PASS') for name, _ in host['CASES']])
        self.serial = "bsshd: host key ED25519 SHA256:example\nbsshd: refused method 'password'\n"

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
        self.serial = 'bsshd: host key ED25519 SHA256:example\n'
        with self.assertRaises(ValueError): score()
        self.serial = serial + 'KERNEL PANIC: example\n'
        with self.assertRaises(ValueError): score()

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
            verdict = score_boot(directory)
            self.assertFalse(verdict['milestones']['server/ssh-keys']['passed'])
            self.assertTrue(verdict['captureComplete'])

    def test_missing_category_and_manifest_drift_block_acceptance(self):
        self.manifest['categories'][0]['cases'] = []
        with self.assertRaises(ValueError):
            host['score'](self.manifest, self.path, self.records, self.serial, self.boot, self.sha, ['KERNEL PANIC:'])


if __name__ == '__main__':
    unittest.main()

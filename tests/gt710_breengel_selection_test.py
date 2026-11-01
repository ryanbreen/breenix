"""Step 9.1 selection and BreenGL capture scoring."""
import json
from pathlib import Path
import re
import runpy
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
API = runpy.run_path(str(ROOT / 'scripts/metal/gt710-suite'))
MANIFEST = json.loads((ROOT / 'docs/suites/breengel.json').read_text())
NAMES = [c['id'] + '/' + t['id'] for c in MANIFEST['categories'] for t in c['cases']]
WHOLE = {'font-parser', 'inflate', 'bmp', 'png-decode', 'png-encode', 'raster',
         'text', 'widgets', 'text-entry', 'icons'}
EXCLUDED = {'windows/every-target', 'windows/distinct-content', 'windows/commit-waits-render',
            'windows/release-order', 'isolation/compositor-authorized', 'isolation/efault-readback',
            'isolation/efault-mouse', 'isolation/retired-ops', 'isolation/test-hooks-absent'}
PANEL = {'stage-layout', 'tile-stamps', 'tile-thumbnail', 'spotlight-picture', 'spotlight-timing',
         'milestone-ribbon', 'finale', 'deterministic', 'stage-hidpi'}
SELECTED = [n for n in NAMES if n.split('/')[0] in WHOLE or
            (n.split('/')[0] in {'windows', 'isolation'} and n not in EXCLUDED) or
            (n.startswith('panel/') and n.split('/')[1] in PANEL)]


def pictured_cases():
    """Read the coordinator's ARM judged-set data independently of the scorer."""
    return set(json.loads((ROOT / 'docs/gui/gt710-breengel-pictured.json').read_text())['cases'])


def capture():
    rows = ['SUITE breengel START cases=401',
            'SUITE breengel MEMORY before free=1000 heap_free_kb=500 user_pages=100']
    rows += [f'SUITE breengel CASE {n} PASS ms=1' + (' sha256=' + 'a' * 64 if n in pictured_cases() else '') for n in NAMES]
    rows += ['SUITE breengel DONE passed=401 failed=0 skipped=0 total=401',
             'SUITE breengel MEMORY after free=1000 heap_free_kb=500 user_pages=100',
             'SUITE breengel MEMORY reclaimed=PASS free_before=1000 free_after=1000 allowance=64',
             'SUITE breengel ELAPSED ms=1000', 'SUITE_SEQUENCE READY breengel']
    return '\n'.join(rows) + '\n'


def boot():
    sha = 'a' * 40
    return dict(id='20261101T000000Z-metal', bootedCommit=sha, toolsCommit=sha,
                kind='boot', mode='suite', suite='breengel', platform='metal', arch='x86_64',
                evidenceEligible=True, build=dict(commit=sha, profile='production', features=[]))


class BreengelSelection(unittest.TestCase):
    def test_inventory_tool_requires_successful_arm_and_rejects_judged_set_drift(self):
        tool = ROOT / 'scripts/gt710-picture-inventory.py'
        self.assertTrue(tool.exists(), 'generate inventory from an accepted ARM capture')
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp)
            (p/'run.json').write_text(json.dumps(dict(id='20261101T000000Z-qemu-1', arch='aarch64', platform='qemu', mode='suite', suite='breengel')))
            for name, value in [('dirty','0'), ('commit','a'*40), ('exit-status','0')]:
                (p/name).write_text(value+'\n')
            original = capture()
            (p/'serial.txt').write_text(original)
            command = [sys.executable,str(tool),str(p),'--check',str(ROOT/'docs/gui/gt710-breengel-pictured.json')]
            self.assertEqual(subprocess.run(command,capture_output=True).returncode,0)
            pictured = next(n for n in NAMES if n in pictured_cases())
            ordinary = next(n for n in NAMES if n not in pictured_cases())
            for changed in [original.replace(f'CASE {pictured} PASS ms=1 sha256='+'a'*64,f'CASE {pictured} PASS ms=1'),
                            original.replace(f'CASE {ordinary} PASS ms=1',f'CASE {ordinary} PASS ms=1 sha256='+'a'*64),
                            original.replace('DONE passed=401 failed=0','DONE passed=400 failed=1')]:
                (p/'serial.txt').write_text(changed)
                self.assertNotEqual(subprocess.run(command,capture_output=True).returncode,0)
            (p/'serial.txt').write_text(original)
            run=json.loads((p/'run.json').read_text());run['arch']='x86_64';(p/'run.json').write_text(json.dumps(run))
            self.assertNotEqual(subprocess.run(command,capture_output=True).returncode,0)

    def test_scorer_requires_each_picture_digest(self):
        pictured = pictured_cases()
        self.assertFalse(pictured - set(NAMES))
        # Hashless ordinary records are valid; this rules out requiring hashes
        # indiscriminately on the complete manifest.
        original = capture()
        for name in set(NAMES) - pictured:
            original = original.replace(f'CASE {name} PASS ms=1 sha256=' + 'a' * 64,
                                        f'CASE {name} PASS ms=1')
        producer = {'breengel': {'serial': original, 'boot': boot()}}
        self.assertEqual(API['cases']('breengel-fronts', '', producer)[0]['result'], 'PASS')
        for name in sorted(pictured):
            producer['breengel']['serial'] = original.replace(
                f'CASE {name} PASS ms=1 sha256=' + 'a' * 64, f'CASE {name} PASS ms=1')
            with self.subTest(pictured=name):
                self.assertEqual(API['cases']('breengel-fronts', '', producer)[0]['result'], 'FAIL')

    def test_scorer_allows_failures_only_outside_selection(self):
        original = capture()
        outside = [name for name in NAMES if name not in SELECTED]
        for name in outside:
            original = original.replace(f'CASE {name} PASS ms=1', f'CASE {name} FAIL ms=1')
            original = re.sub(r'(CASE '+re.escape(name)+r' FAIL ms=1(?: sha256=[a-f0-9]{64})?)(?=\n)', r'\1 msg=not-implemented', original)
        original = original.replace('DONE passed=401 failed=0',
                                    f'DONE passed=304 failed={len(outside)}')
        producer = {'breengel': {'serial': original, 'boot': boot()}}
        self.assertEqual(API['cases']('breengel-fronts', '', producer)[0]['result'], 'PASS')
        producer['breengel']['serial'] = original.replace(
            'CASE composition/compose-fullscreen FAIL ms=1 sha256=' + 'a' * 64,
            'CASE composition/compose-fullscreen FAIL ms=1')
        self.assertEqual(API['cases']('breengel-fronts', '', producer)[0]['result'], 'FAIL')

    def test_selection(self):
        step = json.loads((ROOT / 'docs/gui/gt710-step-9-1.json').read_text())
        self.assertEqual(set(step), {'id', 'suite', 'cases', 'boardMilestone',
                                     'title', 'basis', 'added', 'total', 'reasons'})
        self.assertEqual((step['id'], step['suite']), ('gt710-step-9-1', 'breengel'))
        self.assertEqual(step['boardMilestone'], '25.1')
        self.assertEqual((step['added'], step['total']), (304, 304))
        self.assertTrue(step['title'])
        self.assertTrue(step['basis'])
        for category, reason in step['reasons'].items():
            self.assertEqual(reason['added'], sum(
                name.startswith(category + '/') for name in SELECTED))
            self.assertTrue(reason['why'])
        self.assertEqual(sum(reason['added'] for reason in step['reasons'].values()), 304)
        self.assertEqual(len(SELECTED), 304)
        self.assertEqual(step['cases'], SELECTED)
        self.assertIn('docs/gui/gt710-step-9-1.json', (ROOT / 'scripts/metal/metal-build').read_text())

    def test_scorer_rejects_missing_and_failed_selected_cases(self):
        producer = {'breengel': {'serial': capture(), 'boot': boot()}}
        def score():
            return API['cases']('breengel-fronts', '', producer)[0]['result']
        self.assertEqual(score(), 'PASS')
        original = capture()
        for name in SELECTED:
            line = f'SUITE breengel CASE {name} PASS ms=1' + (' sha256=' + 'a' * 64 if name in pictured_cases() else '')
            for replacement in ('', line.replace(' PASS ', ' FAIL ').replace(' sha256=', ' sha256=') + ' msg=broken'):
                producer['breengel']['serial'] = original.replace(line + '\n', replacement + ('\n' if replacement else ''))
                with self.subTest(case=name, replacement=bool(replacement)):
                    self.assertEqual(score(), 'FAIL')
        for line in ('SUITE breengel DONE passed=401 failed=0 skipped=0 total=401\n',
                     'SUITE breengel MEMORY reclaimed=PASS free_before=1000 free_after=1000 allowance=64\n'):
            producer['breengel']['serial'] = original.replace(line, '')
            self.assertEqual(score(), 'FAIL')
        for name in ('panel/stage-layout', 'raster/fill-rect'):
            producer['breengel']['serial'] = original.replace(
                f'CASE {name} PASS ms=1 sha256=' + 'a' * 64,
                f'CASE {name} PASS ms=1')
            with self.subTest(pictured=name):
                self.assertEqual(score(), 'FAIL')


if __name__ == '__main__':
    unittest.main()

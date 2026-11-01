"""Step 9.1 selection and BreenGL capture scoring."""
import json
from pathlib import Path
import runpy
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


def capture():
    rows = ['SUITE breengel START cases=401',
            'SUITE breengel MEMORY before free=1000 heap_free_kb=500 user_pages=100']
    rows += [f'SUITE breengel CASE {n} PASS ms=1 sha256=' + 'a' * 64 for n in NAMES]
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
    def test_selection(self):
        step = json.loads((ROOT / 'docs/gui/gt710-step-9-1.json').read_text())
        self.assertEqual(set(step), {'id', 'suite', 'cases'})
        self.assertEqual((step['id'], step['suite']), ('gt710-step-9-1', 'breengel'))
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
            line = f'SUITE breengel CASE {name} PASS ms=1 sha256=' + 'a' * 64
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

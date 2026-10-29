"""PR 8.4 selection and retained GPU evidence checks."""
import json
from pathlib import Path
import re
import runpy
import shutil
import tempfile
import unittest
from unittest.mock import patch
import sys

from gt710_alpha_report_test import retained_capture
from gt710_resources_report_test import API, ROOT, SHA, NAMES

ADDED = {'profile/binary32-emulation', 'profile/large-coordinates'} | {
    'pipelines/' + n for n in ('coverage-spans-square', 'coverage-spans-winding',
        'coverage-spans-corpus', 'line-bresenham', 'line-clip', 'line-extreme',
        'circle-fill', 'circle-outline', 'rounded-rect', 'gradient', 'triangle-integer')}


class ShapesReport(unittest.TestCase):
    def test_shapes_acceptance_cli_selects_shapes(self):
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            pr = dict(number=1, state='OPEN', headRefOid=SHA)
            proof = dict(passed=True, boots=[], checks=[], selected=dict(passed=90, total=90),
                         consoleHandoffToDoneMS=100, gpuElapsedMS=200)
            with patch.object(sys, 'argv', ['gt710-suite', '--shapes', '--acceptance-directory', tmp,
                                           '--sha', SHA, '--pr', '1']), \
                 patch('subprocess.check_output', side_effect=[SHA, '', json.dumps(pr), json.dumps(pr)]), \
                 patch.dict(API['main'].__globals__, alpha_acceptance=lambda *args: proof):
                self.assertEqual(API['main'](), 0)
            self.assertTrue(json.loads((directory / 'acceptance.json').read_text())['passed'])

    def test_selection_and_registration(self):
        alpha = json.loads((ROOT / 'docs/gui/gt710-step-8-3.json').read_text())['cases']
        step = json.loads((ROOT / 'docs/gui/gt710-step-8-4.json').read_text())
        selected = [n for n in NAMES if n in set(alpha) | ADDED]
        self.assertEqual((step['id'], step['suite'], step['cases']), ('gt710-step-8-4', 'gpu', selected))
        self.assertEqual(len(selected), 90)
        later = json.loads((ROOT / 'docs/gui/gt710-step-8-5.json').read_text())['cases']
        self.assertFalse(set(selected) & set(later))
        manifest = json.loads((ROOT / 'docs/suites/gt710.json').read_text())
        API['validate_manifest'](manifest)
        category = next(c for c in manifest['categories'] if c['id'] == 'shapes')
        self.assertTrue(category['hostRunner'])
        self.assertEqual([c['id'] for c in category['cases']], ['suite'])
        self.assertIn('docs/gui/gt710-step-8-4.json', (ROOT / 'scripts/metal/metal-build').read_text())

    def test_selected_failures_and_evidence(self):
        metal = runpy.run_path(str(ROOT / 'scripts/metal/metal_score.py'))
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            gpu = retained_capture(directory)
            target = directory / 'definition/docs/gui/gt710-step-8-4.json'
            shutil.copyfile(ROOT / 'docs/gui/gt710-step-8-4.json', target)
            serial = (gpu / 'serial.txt').read_text()
            for name in ADDED:
                serial = serial.replace(f'CASE {name} FAIL ms=1 msg=not implemented', f'CASE {name} PASS ms=1')
            serial = serial.replace('passed=77 failed=93', 'passed=90 failed=80')
            (gpu / 'serial.txt').write_text(serial)
            self.assertEqual(API['record_resources_capture'](directory, SHA, 'shapes'), 0)
            def score():
                return metal['score'](directory)['milestones']['metal-gt710/shapes']['passed']
            self.assertTrue(score())  # Remaining 80 failures are outside the selection (G13).
            for name in json.loads(target.read_text())['cases']:
                bad = serial.replace(f'CASE {name} PASS ms=1', f'CASE {name} FAIL ms=1 msg=broken').replace('passed=90 failed=80', 'passed=89 failed=81')
                (gpu / 'serial.txt').write_text(bad)
                with self.subTest(case=name): self.assertFalse(score())
            for bad in (re.sub(r'^GT710_CAPS_REPROBE .*\n', '', serial, count=1, flags=re.M),
                        re.sub(r'^SUITE gpu MEMORY .*\n', '', serial, flags=re.M),
                        serial.replace('MEMORY reclaimed=PASS', 'MEMORY reclaimed=FAIL')):
                (gpu / 'serial.txt').write_text(bad)
                self.assertFalse(score())


if __name__ == '__main__':
    unittest.main()

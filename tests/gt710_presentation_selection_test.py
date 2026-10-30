"""PR 8.6 presentation selection and cumulative scoring."""
import json
from pathlib import Path
import tempfile
import unittest

from gt710_alpha_report_test import retained_capture
from gt710_resources_report_test import API, ROOT, NAMES, SHA


class PresentationSelection(unittest.TestCase):
    def test_selection_and_registration(self):
        prior = [json.loads((ROOT / f'docs/gui/gt710-step-8-{i}.json').read_text())['cases'] for i in range(2, 6)]
        previous = set().union(*map(set, prior))
        expected = {n for n in NAMES if n.startswith('present/')} | {
            'readback/owner-only', 'isolation/display-context-unreachable',
            'isolation/no-addresses', 'isolation/client-cannot-scanout',
            'isolation/irq-masked-bound'}
        step = json.loads((ROOT / 'docs/gui/gt710-step-8-6.json').read_text())
        self.assertEqual(len(expected), 21)
        self.assertEqual(step, dict(id='gt710-step-8-6', suite='gpu', cases=[n for n in NAMES if n in expected]))
        self.assertFalse(previous & expected)
        self.assertEqual(expected, {n for n in NAMES if not n.startswith('performance/')} - previous)
        self.assertEqual(len(previous | expected), 161)
        manifest = json.loads((ROOT / 'docs/suites/gt710.json').read_text())
        API['validate_manifest'](manifest)
        category = next(c for c in manifest['categories'] if c['id'] == 'presentation')
        self.assertTrue(category['hostRunner'])
        self.assertEqual([c['id'] for c in category['cases']], ['panic', 'suite'])
        self.assertEqual(API['STEP_SELECTIONS']['presentation'], 'gt710-step-8-6.json')

    def test_missing_or_failed_selected_case_rejected(self):
        selected = set().union(*(set(json.loads((ROOT / f'docs/gui/gt710-step-8-{i}.json').read_text())['cases']) for i in range(2, 7)))
        self.assertEqual(len(selected), 161)
        with tempfile.TemporaryDirectory() as tmp:
            gpu = retained_capture(Path(tmp))
            serial = (gpu / 'serial.txt').read_text()
            boot = dict(json.loads((gpu / 'run.json').read_text()), bootedCommit=SHA)
            for name in selected:
                serial = serial.replace(f'CASE {name} FAIL ms=1 msg=not implemented', f'CASE {name} PASS ms=1')
            serial = serial.replace('passed=77 failed=93', 'passed=161 failed=9')
            API['check_case']('presentation', 'suite', '', {'gpu': {'serial': serial, 'boot': boot}})
            for name in selected:
                line = f'SUITE gpu CASE {name} PASS ms=1\n'
                self.assertIn(line, serial)
                for bad in (serial.replace(line, ''), serial.replace(line, f'SUITE gpu CASE {name} FAIL ms=1 msg=broken\n').replace('passed=161 failed=9', 'passed=160 failed=10')):
                    with self.subTest(case=name, missing=line not in bad), self.assertRaises(ValueError):
                        API['check_case']('presentation', 'suite', '', {'gpu': {'serial': bad, 'boot': boot}})


if __name__ == '__main__':
    unittest.main()

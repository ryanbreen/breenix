"""PR 8.5 selection and cumulative capture scoring."""
import json
from pathlib import Path
import shutil
import tempfile
import unittest
from unittest.mock import patch

from gt710_alpha_report_test import retained_capture
from gt710_resources_report_test import API, ROOT, SHA, NAMES

EXTRA = {
    'device/device-error-reported', 'readback/error-invalidates-targets',
    'textures/copy-region', 'textures/immutable-while-pinned',
    'textures/destroy-deferred', 'textures/quota', 'profile/scissor-exact',
}
EXCLUDED = {'isolation/display-context-unreachable', 'isolation/no-addresses',
            'isolation/client-cannot-scanout'}
ADDED = {n for n in NAMES if n.startswith('submit/') or
         (n.startswith('isolation/') and n not in EXCLUDED)} | EXTRA


class IsolationReport(unittest.TestCase):
    def test_selection_and_registration(self):
        step = json.loads((ROOT / 'docs/gui/gt710-step-8-5.json').read_text())
        self.assertEqual(len(ADDED), 51)
        self.assertEqual((step['id'], step['suite'], step['cases']),
                         ('gt710-step-8-5', 'gpu', [n for n in NAMES if n in ADDED]))
        previous = json.loads((ROOT / 'docs/gui/gt710-step-8-4.json').read_text())['cases']
        self.assertFalse(set(previous) & ADDED)
        self.assertEqual(len(set(previous) | ADDED), 141)
        manifest = json.loads((ROOT / 'docs/suites/gt710.json').read_text())
        API['validate_manifest'](manifest)
        category = next(c for c in manifest['categories'] if c['id'] == 'isolation')
        self.assertTrue(category['hostRunner'])
        self.assertEqual([c['id'] for c in category['cases']], ['suite'])
        effort = json.loads((ROOT / 'docs/efforts/metal.json').read_text())
        self.assertTrue(any(c['id'] == 'isolation' and c['measure']['categories'] == ['isolation']
                            for e in effort['efforts'] for c in e['milestones']))
        self.assertIn('docs/gui/gt710-step-8-5.json', (ROOT / 'scripts/metal/metal-build').read_text())

    def test_acceptance_scores_four_steps_from_one_pair(self):
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            producer = dict(console=dict(boot=dict(id='console'), serial=''))
            def record(path, sha, category):
                (path / ('gt710-' + category + '.json')).write_text(json.dumps(dict(cases=[dict(id=category)])))
                return 0
            with patch.dict(API['alpha_acceptance'].__globals__,
                    alpha_capture_proof=lambda *a: dict(captureCommit=SHA),
                    resources_capture=lambda *a: producer, capture_categories=lambda *a: [],
                    score=lambda *a: None, record_resources_capture=record):
                proof = API['alpha_acceptance'](directory, SHA, {}, 'isolation')
            self.assertEqual([c['category'] for c in proof['checks']],
                             ['resources', 'alpha', 'shapes', 'isolation'])

    def test_selected_failure_fails_score(self):
        metal = __import__('runpy').run_path(str(ROOT / 'scripts/metal/metal_score.py'))
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            gpu = retained_capture(directory)
            for step in ('8-4', '8-5'):
                target = directory / ('definition/docs/gui/gt710-step-' + step + '.json')
                shutil.copyfile(ROOT / ('docs/gui/gt710-step-' + step + '.json'), target)
            serial = (gpu / 'serial.txt').read_text()
            selected = set(json.loads((ROOT / 'docs/gui/gt710-step-8-4.json').read_text())['cases']) | ADDED
            for name in selected:
                serial = serial.replace(f'CASE {name} FAIL ms=1 msg=not implemented', f'CASE {name} PASS ms=1')
            serial = serial.replace('passed=77 failed=93', 'passed=141 failed=29')
            (gpu / 'serial.txt').write_text(serial)
            self.assertEqual(API['record_resources_capture'](directory, SHA, 'isolation'), 0)
            def score():
                return metal['score'](directory)['milestones']['metal-gt710/isolation']['passed']
            self.assertTrue(score())
            for name in selected:
                bad = serial.replace(f'CASE {name} PASS ms=1', f'CASE {name} FAIL ms=1 msg=broken').replace('passed=141 failed=29', 'passed=140 failed=30')
                (gpu / 'serial.txt').write_text(bad)
                with self.subTest(case=name):
                    self.assertFalse(score())

    def test_cli_accepts_isolation(self):
        with patch('sys.argv', ['gt710-suite', '--isolation', '--acceptance-directory', '/nonexistent', '--sha', SHA, '--pr', '1']), \
             patch('subprocess.check_output', side_effect=[SHA, '', json.dumps(dict(number=1, state='OPEN', headRefOid=SHA)), json.dumps(dict(number=1, state='OPEN', headRefOid=SHA))]), \
             patch.dict(API['main'].__globals__, alpha_acceptance=lambda *args: (_ for _ in ()).throw(ValueError('isolation reached'))), \
             patch.object(Path, 'write_text'):
            self.assertEqual(API['main'](), 1)


if __name__ == '__main__':
    unittest.main()

"""Saved FRAME paths extend the grammar without relaxing its case binding."""
import importlib.util
import copy
import json
import re
from pathlib import Path
import unittest
ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('artifact_verdict', ROOT/'scripts/suite-verdict.py')
VERDICT = importlib.util.module_from_spec(spec)
spec.loader.exec_module(VERDICT)

class ArtifactRecords(unittest.TestCase):
    def test_scenario_frames_are_declared_and_required_by_the_manifest(self):
        manifest = json.loads((ROOT/'docs/suites/ui-testing.json').read_text())
        stages = {'proof/form-scenario': ['initial','typed','checked','slider','submitted'],
                  'proof/menu-scenario': ['initial','opened','chosen'],
                  'proof/list-scenario': ['initial','scrolled','selected']}
        cases = [f"{category['id']}/{case['id']}" for category in manifest['categories'] for case in category['cases']]
        def records(omit=None, extra=False):
            rows = [('START', None, 'SUITE ui-testing START cases=108')]
            for case in cases:
                checks = stages.get(case, []) + (['undeclared'] if extra and case=='proof/form-scenario' else [])
                for check in checks:
                    if (case,check)==omit: continue
                    line = (f'SUITE ui-testing FRAME {case} {check} def=0123abcd ref=1234abcd '
                            'source=window:widgets commit=5 epoch=0 w=1280 h=800 sha256='+'a'*64+' result=match')
                    rows.append(('FRAME',re.match(r'SUITE ui-testing FRAME ([^ ]+)',line),line))
                line=f'SUITE ui-testing CASE {case} PASS ms=1'
                rows.append(('PASS',re.match(r'SUITE ui-testing CASE ([^ ]+)',line),line))
            return rows+[('DONE',None,'SUITE ui-testing DONE passed=108 failed=0 skipped=0 total=108')]
        self.assertIsNone(VERDICT.result_problem(manifest,records(),cases))
        missing=VERDICT.result_problem(manifest,records(('proof/form-scenario','typed')),cases)
        self.assertIn('typed',missing)
        self.assertIn('never ran',missing)
        self.assertIn('undeclared',VERDICT.result_problem(manifest,records(extra=True),cases))
        duplicate=copy.deepcopy(manifest)
        duplicate['frameChecks'].append(duplicate['frameChecks'][0])
        self.assertIn('duplicate',VERDICT.result_problem(duplicate,records(),cases))

    def test_saved_pass_and_failure_paths_and_refusals(self):
        base = ('SUITE ui-testing FRAME diagnosis/local pixels def=12345678 ref=formula '
                'source=window:app commit=41 epoch=0 w=2 h=1 sha256=' + 'a'*64)
        path = ' path=/var/uitest/ui-testing/diagnosis/local'
        mismatch = ' result=mismatch compared=2 differing=1 first=1,0 got=#283263 want=#28323c'
        parse = lambda suffix: VERDICT.parse_result(base + suffix, 'ui-testing', 'diagnosis/local', ['pixels'], False)
        self.assertEqual(parse(mismatch + ' artifacts=complete' + path), ('FRAME:pixels', None, 'pixels'))
        self.assertEqual(parse(' result=match artifacts=complete' + path), ('FRAME:pixels', 'pixels', None))
        self.assertEqual(parse(' result=match artifacts=none:io'), ('FRAME:pixels', 'pixels', None))
        for suffix in [
            mismatch + ' artifacts=none:cap' + path,
            mismatch + ' artifacts=complete' + path.replace('/local', '/other'),
            mismatch + ' artifacts=complete' + path.replace('/local', '/../local'),
            mismatch + ' artifacts=complete path=relative',
            ' result=match artifacts=complete',
            ' result=match artifacts=complete' + path + ' extra=1',
        ]:
            with self.subTest(suffix=suffix), self.assertRaises(ValueError):
                parse(suffix)

if __name__ == '__main__': unittest.main()

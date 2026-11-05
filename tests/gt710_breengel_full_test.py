"""Step 9.4 requires the complete suite, latched observations and memory PASS."""
import json
import subprocess
import sys
import unittest
from gt710_breengel_selection_test import API, ROOT, NAMES, boot
from gt710_breengel_evidence_test import composition_capture


class FullAcceptance(unittest.TestCase):
    def test_full_selection_is_delivered_with_the_capture(self):
        step = json.loads((ROOT / 'docs/gui/gt710-step-9-4.json').read_text())
        self.assertEqual(step['cases'], NAMES)
        self.assertIn('docs/gui/gt710-step-9-4.json',
                      (ROOT / 'scripts/metal/metal-build').read_text())
        help_text = subprocess.check_output(
            [sys.executable, str(ROOT / 'scripts/metal/gt710-suite'), '--help'], text=True)
        self.assertIn('--breengel-full', help_text)

    def test_full_gate_rejects_the_three_cases_outside_step_9_3(self):
        original = composition_capture()
        self.assertEqual(len(API['breengel_fronts_suite'](original, boot(), 4)), 401)
        prior = json.loads((ROOT / 'docs/gui/gt710-step-9-3.json').read_text())['cases']
        added = set(NAMES) - set(prior)
        self.assertEqual(added, {'composition/present-rate', 'isolation/retired-ops',
                                'isolation/test-hooks-absent'})
        for name in sorted(added):
            failed = original.replace(f'CASE {name} PASS ms=1', f'CASE {name} FAIL ms=1')
            failed = failed.replace(next(line for line in failed.splitlines()
                if line.startswith(f'SUITE breengel CASE {name} ')),
                next(line for line in failed.splitlines()
                if line.startswith(f'SUITE breengel CASE {name} ')) + ' msg=failed')
            failed = failed.replace('DONE passed=401 failed=0', 'DONE passed=400 failed=1')
            producer = {'breengel': dict(serial=failed, boot=boot())}
            with self.subTest(case=name):
                result = API['cases']('breengel-full', '', producer)[0]
                self.assertEqual(result['result'], 'FAIL')
                self.assertIn('selected BreenGL case failed', result['reason'])

    def test_full_gate_keeps_memory_and_latched_observation_checks(self):
        original = composition_capture()
        for bad in (original.replace('free_after=1000 allowance=48', 'free_after=900 allowance=48')
                            .replace('after free=1000', 'after free=900')
                            .replace('reclaimed=PASS', 'reclaimed=FAIL'),
                    original.replace('"latched_confirmed": 1,', '"latched_confirmed": 0,')):
            with self.subTest(memory='reclaimed=FAIL' in bad), self.assertRaises(ValueError):
                API['breengel_fronts_suite'](bad, boot(), 4)


if __name__ == '__main__':
    unittest.main()

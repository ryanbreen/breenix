"""The full GPU gate rejects incomplete or misleading performance evidence."""
import contextlib
import io
import json
import unittest

from gt710_resources_report_test import API, NAMES, ROOT, boot, transcript

VALUES = {
    'clear-rate-2560': [('clears-per-second', 60, '', 60, 9223372036854775807)],
    'rect-throughput': [('submission-us', 1, 'us', 0, 50000)],
    'submit-latency': [('median-us', 1, 'us', 0, 2000), ('max-us', 1, 'us', 0, 10000)],
    'first-draw-latency': [('slowest-us', 1, 'us', 0, 10000)],
    'readback-time-2560': [('readback-us', 1, 'us', 0, 100000)],
    'upload-bandwidth': [('upload-us', 1, 'us', 0, 100000)],
    'coverage-throughput': [('coverage-us', 1, 'us', 0, 16700)],
    'wait-no-spin': [('wait-us', 100, 'us', None, None), ('cpu-us', 1, 'us', 0, 5)],
    'present-rate-2560': [('presents-per-second', 60, '', 60, 9223372036854775807)],
}


def fixture():
    serial = transcript(all_pass=True)
    for case, values in VALUES.items():
        lines = ''
        for key, value, unit, low, high in values:
            lines += f'SUITE gpu VALUE performance/{case} {key}={value}{unit}'
            if low is not None:
                lines += f' expect={low}..{high}{unit}'
            lines += '\n'
        serial = serial.replace(f'SUITE gpu CASE performance/{case} PASS', lines + f'SUITE gpu CASE performance/{case} PASS')
    for i, name in enumerate([''] + NAMES):
        serial += 'GT710_CASE_HEAP ' + json.dumps(dict(schema=1, boundary=i, case=name,
            values=dict(native_resources=i, native_contexts=i, submit_n=i, submit_ns=2*i))) + '\n'
    return serial


class PerformanceReport(unittest.TestCase):
    def score(self, serial):
        with contextlib.redirect_stdout(io.StringIO()) as output:
            API['resources_suite'](serial, boot(), 'gt710-step-8-7.json')
        return output.getvalue()

    def test_full_gate_and_mutations(self):
        clean = fixture()
        self.assertIn('native_resources', self.score(clean))
        bad = [clean.replace('CASE performance/rect-throughput PASS', 'CASE performance/rect-throughput FAIL', 1).replace('FAIL ms=1\n', 'FAIL ms=1 msg=slow\n', 1).replace('passed=170 failed=0', 'passed=169 failed=1'),
               clean.replace('skipped=0', 'skipped=1'),
               clean.replace('SUITE gpu VALUE performance/rect-throughput submission-us=1us expect=0..50000us\n', ''),
               clean.replace('submission-us=1us', 'submission-us=50001us'),
               clean.replace('CASE ' + NAMES[0] + ' ', 'CASE TEMP ', 1).replace('CASE ' + NAMES[1] + ' ', 'CASE ' + NAMES[0] + ' ', 1).replace('CASE TEMP ', 'CASE ' + NAMES[1] + ' ', 1),
               '\n'.join(line for line in clean.split('\n') if '"boundary": 7,' not in line)]
        for item in bad:
            with self.subTest(item=bad.index(item)), self.assertRaises(ValueError):
                self.score(item)

    def test_selection_is_complete_and_registered(self):
        step = json.loads((ROOT / 'docs/gui/gt710-step-8-7.json').read_text())
        self.assertEqual(step['cases'], NAMES)
        self.assertEqual(API['STEP_SELECTIONS']['performance'], 'gt710-step-8-7.json')
        self.assertIn('docs/gui/gt710-step-8-7.json', (ROOT / 'scripts/metal/metal-build').read_text())


if __name__ == '__main__':
    unittest.main()

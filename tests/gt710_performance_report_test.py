"""The full GPU gate rejects incomplete or misleading performance evidence."""
import contextlib
import io
import json
import hashlib
import re
import tempfile
from pathlib import Path
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


def heap_names():
    source = (ROOT / 'libs/gpu-abi/src/heap_trace.rs').read_text()
    return re.findall(r'"([a-z_]+)"', source.split('pub const NAMES:', 1)[1].split('];', 1)[0])


def fixture():
    from gt710_clock_report_test import row
    serial = row('startup') + transcript(all_pass=True) + row('final')
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
            values={key: (1 if key == 'schema' else i) for key in heap_names()})) + '\n'
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

    def test_complete_86_capture_reaches_the_six_performance_failures(self):
        names = heap_names()
        legacy = set(names[:56])
        expected = ['performance/' + case for case in (
            'clear-rate-2560', 'rect-throughput', 'readback-time-2560',
            'upload-bandwidth', 'coverage-throughput', 'present-rate-2560')]
        serial = fixture()
        records = []
        for line in serial.splitlines(keepends=True):
            if line.startswith('GT710_CASE_HEAP '):
                row = json.loads(line.removeprefix('GT710_CASE_HEAP '))
                row['values'] = {k: v for k, v in row['values'].items() if k in legacy}
                records.append('GT710_CASE_HEAP ' + json.dumps(row) + '\n')
        serial = ''.join(line for line in serial.splitlines(keepends=True)
                         if not line.startswith(('GT710_CASE_HEAP ', 'GT710_CLOCK_V1 ')))
        for name in expected:
            serial = serial.replace('CASE ' + name + ' PASS ms=1',
                                    'CASE ' + name + ' FAIL ms=1 msg=outside bound')
        serial = serial.replace('passed=170 failed=0', 'passed=164 failed=6')
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            def anchor(prefix, body):
                return f'{prefix} {len(body)} {hashlib.sha256(body).hexdigest()}\n'
            first = b'GT710_PIX 0 1:ff000001\n'
            final = ''.join(records).encode()
            serial = ('GT710_MODE {"schema":1}\n' + anchor('GT710_BULK', first)
                      + serial.replace('SUITE_SEQUENCE READY gpu\n',
                          anchor('GT710_CASE_HEAP_BULK', final)
                          + anchor('GT710_FINAL_BULK', final) + 'SUITE_SEQUENCE READY gpu\n'))
            (path / 'serial.txt').write_text(serial)
            (path / 'gt710-pixels.rle').write_bytes(first)
            (path / 'gt710-final.rle').write_bytes(final)
            decoded = API['capture_serial'](path)
            with self.assertRaises(ValueError) as error:
                self.score(decoded)
            self.assertEqual(re.findall(r'(performance/[a-z0-9-]+):', str(error.exception)), expected)
            # The old format is readable, but it cannot pass the current full gate.
            current = fixture()
            without_new_slots = '\n'.join(
                ('GT710_CASE_HEAP ' + json.dumps(dict(json.loads(line[16:]),
                    values={k: v for k, v in json.loads(line[16:])['values'].items() if k in legacy})))
                if line.startswith('GT710_CASE_HEAP ') else line
                for line in current.split('\n'))
            with self.assertRaisesRegex(ValueError, 'cost slots missing'):
                self.score(without_new_slots)
            (path / 'gt710-final.rle').write_bytes(final.replace(b'"schema": 1', b'"schema": 2', 1))
            with self.assertRaisesRegex(ValueError, 'length/digest'):
                API['capture_serial'](path)

    def test_selection_is_complete_and_registered(self):
        step = json.loads((ROOT / 'docs/gui/gt710-step-8-7.json').read_text())
        self.assertEqual(step['cases'], NAMES)
        self.assertEqual(API['STEP_SELECTIONS']['performance'], 'gt710-step-8-7.json')
        self.assertIn('docs/gui/gt710-step-8-7.json', (ROOT / 'scripts/metal/metal-build').read_text())


if __name__ == '__main__':
    unittest.main()

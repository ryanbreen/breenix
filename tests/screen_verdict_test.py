#!/usr/bin/env python3
"""Exercise screen verdicts with small serial and approval-table fixtures."""
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCORER = ROOT / 'scripts/screen-verdict.py'
DIGEST = 'a' * 64
CONFIG = 'c' * 64


class ScreenVerdictTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.dir = Path(self.tmp.name)
        self.manifest = {'id': 'gpu', 'categories': [{'id': 'draw', 'cases': [{'id': 'one'}, {'id': 'two'}]}]}
        self.entry = {'suite': 'gpu', 'case': 'draw/one', 'checkpoint': 'end',
                      'sha256': DIGEST, 'mode': '1280x800',
                      'native': [{'out': 'sor0', 'raster': '1280x1024', 'config': CONFIG,
                                  'crc': '12345678', 'crc2': '87654321'}]}
        self.line = f'SUITE gpu SCREEN draw/one end sha256={DIGEST} mode=1280x800 binding=1280x800'

    def run_score(self, lines, *, entries=None, args=(), include_cases=True):
        (self.dir / 'manifest.json').write_text(json.dumps(self.manifest))
        (self.dir / 'table.json').write_text(json.dumps({'version': 1, 'entries': [self.entry] if entries is None else entries}))
        prefix = ['SUITE gpu CASE draw/one PASS ms=1', 'SUITE gpu CASE draw/two PASS ms=1'] if include_cases else []
        (self.dir / 'serial.txt').write_text('\n'.join(prefix + lines) + '\n')
        return subprocess.run([sys.executable, str(SCORER), str(self.dir / 'manifest.json'),
                               str(self.dir / 'serial.txt'), '--table', str(self.dir / 'table.json'),
                               *args], capture_output=True, text=True)

    def test_match_and_unapproved(self):
        extra = f'SUITE gpu SCREEN draw/two intent sha256={DIGEST} mode=1280x800 binding=1280x800'
        result = self.run_score([self.line, extra])
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn('SCREENS PASS approved=1 matched=1 unapproved=1', result.stdout)
        self.assertIn('draw/two intent', result.stdout)

    def test_missing_mismatch_duplicate_error_and_bad_records(self):
        cases = [[], [self.line.replace(DIGEST, 'b' * 64)],
                 [self.line.replace('1280x800', '640x480')], [self.line, self.line],
                 ['SUITE gpu SCREEN draw/one end error=readback-failed'],
                 [self.line.replace('draw/one', 'draw/unknown')],
                 [self.line + ' bad=field']]
        for lines in cases:
            with self.subTest(lines=lines):
                result = self.run_score(lines)
                self.assertNotEqual(result.returncode, 0, result.stdout)
                self.assertIn('SCREENS FAIL:', result.stdout)

    def test_checkpoint_must_follow_its_case(self):
        result = self.run_score([self.line, 'SUITE gpu CASE draw/one PASS ms=1'], include_cases=False)
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn('precedes CASE', result.stdout)

    def test_binding_geometry(self):
        line = self.line.replace('binding=1280x800', 'binding=2560x1600')
        self.assertEqual(self.run_score([line]).returncode, 0)
        self.assertNotEqual(self.run_score([self.line.split(' binding=')[0]]).returncode, 0)
        for binding in ['640x480', '2560x0', 'unknown', '2560x1600 extra=1']:
            self.assertNotEqual(self.run_score([self.line.replace('binding=1280x800', 'binding=' + binding)]).returncode, 0)

    def test_native_crc_and_configuration(self):
        args = ('--native', '--config', CONFIG)
        suffix = ' crc=12345678 crc2=87654321 raster=1280x1024 out=sor0'
        self.assertEqual(self.run_score([self.line + suffix], args=args).returncode, 0)
        for line in [self.line, self.line + suffix.replace('12345678', '11111111'),
                     self.line + suffix.replace('87654321', '11111111'),
                     self.line + suffix.replace('sor0', 'sor1')]:
            with self.subTest(line=line):
                self.assertNotEqual(self.run_score([line], args=args).returncode, 0)
        self.assertNotEqual(self.run_score([self.line + suffix], args=('--native', '--config', 'd' * 64)).returncode, 0)

    def test_declared_end_is_listed_but_not_eligible(self):
        for reason in ['device-reset', 'target-invalidated']:
            line = f'SUITE gpu SCREEN draw/one end unobservable reason={reason} errno=5 eio_reason=4 failed_submission=1234'
            result = self.run_score([line], entries=[])
            self.assertEqual(result.returncode, 0, result.stdout)
            self.assertIn('unapproved=0', result.stdout)
            self.assertIn(f'INELIGIBLE draw/one end reason={reason}', result.stdout)
            self.assertNotEqual(self.run_score([line]).returncode, 0,
                                'a declared record cannot satisfy an approved digest')
        for line in [
            'SUITE gpu SCREEN draw/one end unobservable reason=device-error',
            'SUITE gpu SCREEN draw/one intent unobservable reason=device-reset',
            'SUITE gpu SCREEN draw/one end error=readback-failed errno=110 eio_reason=0 failed_submission=0',
        ]:
            self.assertNotEqual(self.run_score([line], entries=[]).returncode, 0)

    def test_host_capture_mismatch(self):
        capture = self.dir / 'captures/gpu/draw/one/end.sha256'
        capture.parent.mkdir(parents=True)
        capture.write_text(DIGEST + '\n')
        args = ('--host-captures', str(self.dir / 'captures'))
        self.assertEqual(self.run_score([self.line], args=args).returncode, 0)
        capture.write_text('b' * 64 + '\n')
        self.assertNotEqual(self.run_score([self.line], args=args).returncode, 0)


if __name__ == '__main__':
    unittest.main()

"""Exercise the shared Parallels and VMware verdict path without a VM."""
import contextlib
import io
import json
from pathlib import Path
import runpy
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
WAIT = runpy.run_path(str(ROOT / 'scripts/wait-boot-done.py'))


class VMClockVerdictTest(unittest.TestCase):
    def test_suite_clock_failure_preserves_suite_verdict(self):
        manifest = json.loads((ROOT / 'docs/suites/time.json').read_text())
        cases = [f"{category['id']}/{case['id']}" for category in manifest['categories']
                 for case in category['cases']]
        suite = ('SUITE time START cases=89\n' +
                 ''.join(f'SUITE time CASE {case} PASS ms=0\n' for case in cases) +
                 'SUITE time DONE passed=89 failed=0 skipped=0 total=89\n')
        for offset in (None, -14400, 3600, 0):
            with self.subTest(offset=offset), tempfile.TemporaryDirectory() as directory:
                started = time.time()
                clock = '' if offset is None else f'[clock] UTC epoch={int(started) + offset} source=UEFI\n'
                serial = Path(directory) / 'serial'
                serial.write_text(clock + suite)
                output = io.StringIO()
                with patch.object(sys, 'argv', ['wait', str(serial), 'suite', '--suite', 'time',
                                              '--started', str(started)]), contextlib.redirect_stdout(output):
                    self.assertEqual(WAIT['main'](), 0 if offset == 0 else 1)
                self.assertIn('PASS: SUITE time DONE passed=89 failed=0', output.getvalue())
                self.assertIn('UTC clock PASS' if offset == 0 else 'UTC clock FAIL', output.getvalue())


if __name__ == '__main__':
    unittest.main()

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
        owners = 'user_stacks=17 user_pages=300 page_tables=13 page_cache=0 kernel_stacks=40 buffers=8 other=2 receipts=0 settle_ms=100'
        suite = (f'SUITE time START cases={len(cases)}\n' +
                 f'SUITE time MEMORY before free=1000 heap_free_kb=500 {owners}\n' +
                 ''.join(f'SUITE time CASE {case} PASS ms=0\n' for case in cases) +
                 f'SUITE time DONE passed={len(cases)} failed=0 skipped=0 total={len(cases)}\n' +
                 f'SUITE time MEMORY after free=990 heap_free_kb=496 {owners}\n' +
                 'SUITE time MEMORY reclaimed=PASS free_before=1000 free_after=990 allowance=64\n')
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
                self.assertIn(f'PASS: SUITE time DONE passed={len(cases)} failed=0', output.getvalue())
                self.assertIn('UTC clock PASS' if offset == 0 else 'UTC clock FAIL', output.getvalue())


if __name__ == '__main__':
    unittest.main()

#!/usr/bin/env python3
"""Current core producer admission: executable CAPS and real suite records.

Coordinator G2 ruling, 2026-11-01: f3ceabdf superseded the refusal-only 8.1
production context documented in docs/gui/gt710-core-acceptance.md.
"""
from pathlib import Path
import runpy
import re
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
CORE = runpy.run_path(str(ROOT / 'tests/gt710_core_report_test.py'))
RES = runpy.run_path(str(ROOT / 'tests/gt710_resources_report_test.py'))
API = CORE['API']


class CurrentProducer(unittest.TestCase):
    def caps(self):
        with tempfile.TemporaryDirectory() as tmp:
            return API['core_row'](CORE['stage'](Path(tmp)))

    def capture(self, all_pass=True, data=None, probe_rows=None):
        with tempfile.TemporaryDirectory() as tmp:
            return RES['stage'](Path(tmp), data, probe_rows) + RES['transcript'](all_pass=all_pass)

    def test_executable_admission_requires_every_shared_case_to_pass(self):
        self.assertEqual(API['core_producer'](self.capture(), self.caps(), CORE['gpu_boot']()), 900000)
        mixed = self.capture(all_pass=False)
        all_failed = self.capture()
        all_failed = re.sub(r'(SUITE gpu CASE \S+) PASS ms=1',
                            r'\1 FAIL ms=1 msg=broken', all_failed)
        all_failed = all_failed.replace('passed=170 failed=0', 'passed=0 failed=170')
        for serial in (mixed, all_failed):
            with self.subTest(tally=serial[serial.index('SUITE gpu DONE'):][:70]), self.assertRaises(ValueError):
                API['core_producer'](serial, self.caps(), CORE['gpu_boot']())

    def test_caps_need_both_executable_origins_and_their_real_probe_images(self):
        data = RES['caps_bytes']()
        data[120:128] = bytes(8)
        rows = RES['probes']()
        rows[0] = dict(rows[0], first=f"{int(rows[0]['first'][:2], 16) ^ 255:02x}" + rows[0]['first'][2:])
        for changed in (self.capture(data=data), self.capture(probe_rows=rows)):
            with self.subTest(changed=changed[:40]), self.assertRaises(ValueError):
                API['core_producer'](changed, self.caps(), CORE['gpu_boot']())


if __name__ == '__main__':
    unittest.main()

"""The scorer must validate the catalogue the native upload receiver consumes."""
import hashlib
import json
from pathlib import Path
import runpy
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
API = runpy.run_path(str(ROOT / 'scripts/metal/gt710-suite'))
BOARD = runpy.run_path(str(ROOT / 'tests/gt710_runtime_catalogue_test.py'))['BOARD_TEXT']
CATALOGUE = {'default': 1, 'modes': [[1024, 768, 1], [1280, 1024, 1], [2560, 2048, 2]]}


class EvidenceCatalogue(unittest.TestCase):
    def test_native_slots_keep_the_exact_edid_catalogue(self):
        def score(startup, final=None):
            def record(row):
                return ('GT710_EVIDENCE_MODES ' + json.dumps(row, separators=(',', ':')) + '\n').encode()
            body = record(startup)
            serial = BOARD + f'GT710_BULK {len(body)} {hashlib.sha256(body).hexdigest()}\n'
            with tempfile.TemporaryDirectory() as temporary:
                p = Path(temporary)
                (p / 'gt710-pixels.rle').write_bytes(body)
                if final is not None:
                    other = record(final)
                    (p / 'gt710-final.rle').write_bytes(other)
                    serial += ('SUITE gpu START cases=1\nSUITE gpu CASE fixture/a PASS ms=1\n'
                        'SUITE gpu DONE passed=1 failed=0 skipped=0 total=1\n'
                        'SUITE gpu MEMORY reclaimed=PASS free_before=100 free_after=100 allowance=48\n'
                        f'GT710_FINAL_BULK {len(other)} {hashlib.sha256(other).hexdigest()}\n'
                        'SUITE_SEQUENCE READY gpu\n')
                return API['capture_serial_body'](p, serial)
        # This is the 86-byte producer record rejected at startup byte 189743
        # in 20261114T100121Z-metal, repeated in its final slot.
        self.assertIn('GT710_BULK ', score(CATALOGUE, CATALOGUE))
        for bad in [dict(CATALOGUE, default=0), dict(CATALOGUE, default=True),
                    dict(CATALOGUE, extra=0),
                    {'default': 1, 'modes': [[1024, 768, 1], [1280, 1024, 1], [2560, 1600, 2]]}]:
            with self.subTest(bad=bad), self.assertRaises(ValueError):
                score(CATALOGUE, bad)
        # A duplicate or unknown artifact is still refused even with fresh digests.
        for suffix in [b'GT710_EVIDENCE_MODES '+json.dumps(CATALOGUE).encode()+b'\n', b'GT710_UNKNOWN {}\n']:
            body = ('GT710_EVIDENCE_MODES '+json.dumps(CATALOGUE)+'\n').encode()+suffix
            with tempfile.TemporaryDirectory() as temporary:
                p = Path(temporary);(p/'gt710-pixels.rle').write_bytes(body)
                serial=BOARD+f'GT710_BULK {len(body)} {hashlib.sha256(body).hexdigest()}\n'
                with self.assertRaises(ValueError): API['capture_serial_body'](p, serial)


if __name__ == '__main__':
    unittest.main()

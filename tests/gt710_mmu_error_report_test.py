"""G18 raw MMU evidence must survive the final upload with an exact digest."""
import hashlib
import json
from pathlib import Path
import runpy
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
API = runpy.run_path(str(ROOT / 'scripts/metal/gt710-suite'))


class MmuErrorReport(unittest.TestCase):
    def test_raw_fault_and_anchor_are_required_and_consistent(self):
        startup = b'GT710_PIX 0 1:ff003039\n'
        row = dict(instance=0x123000, epoch=17, sequence=3, unit=0,
                   raw=[0x123, 0xf8003000, 0xff, 0xc0], answer=0x60010000)
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp)
            (p / 'gt710-pixels.rle').write_bytes(startup)
            def capture(row, anchor_change=lambda a: a):
                record = ('GT710_MMU_ERROR_V1 ' + json.dumps(row, separators=(',', ':')) + '\n').encode()
                anchor = (f'GT710_MMU_ERROR_BULK epoch=17 sequence=3 bytes={len(record)-1} '
                          f'sha256={hashlib.sha256(record[:-1]).hexdigest()}\n')
                (p / 'gt710-final.rle').write_bytes(record)
                (p / 'serial.txt').write_text(
                    'GT710_MODE {"schema":1}\n'
                    f'GT710_BULK {len(startup)} {hashlib.sha256(startup).hexdigest()}\n'
                    'SUITE gpu START total=1\nSUITE gpu CASE device/device-error-reported PASS ms=1\n'
                    + anchor_change(anchor) + 'SUITE gpu DONE passed=1 failed=0 skipped=0 total=1\n'
                    'SUITE gpu MEMORY reclaimed=PASS free_before=100 free_after=100 allowance=48\n'
                    f'GT710_FINAL_BULK {len(record)} {hashlib.sha256(record).hexdigest()}\n'
                    'SUITE_SEQUENCE READY gpu\n')
                return API['capture_serial'](p)
            self.assertIn('GT710_MMU_ERROR_V1 ', capture(row))
            for change in [lambda a: '', lambda a: a.replace('sequence=3', 'sequence=2'),
                           lambda a: a.replace('bytes=', 'bytes=0'),
                           lambda a: a.replace('sha256=', 'sha256=0')]:
                with self.assertRaises(ValueError):
                    capture(row, change)
            for field, value in [('instance', 0x124000), ('sequence', 2), ('unit', 1),
                                 ('raw', [0x123, 0xf8003000, 0xff, 0x40]),
                                 ('raw', [0x123, 0xf8003000, 0xff, 0xc5]),
                                 ('answer', 0x10), ('answer', 0x60010002)]:
                with self.subTest(field=field, value=value), self.assertRaises(ValueError):
                    capture(dict(row, **{field: value}))


if __name__ == '__main__':
    unittest.main()

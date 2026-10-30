"""G19 refusal records retain their words and authenticate their serial anchor."""
import hashlib
import json
from pathlib import Path
import runpy
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
API = runpy.run_path(str(ROOT / 'scripts/metal/gt710-suite'))
REGISTERS = [0x400704, 0x400708, 0x400700, 0x409c18, 0x2634, 0x2640, 0x262c]


class GrRefusalReport(unittest.TestCase):
    def test_refusal_words_and_each_anchor_are_authenticated(self):
        row = dict(status=[0xbadf1201, 0, 0, 0], registers=REGISTERS,
                   values=[0x80070000, 0, 0x81, 0, 0, 0x8000a000, 0])
        startup = b'GT710_PIX 0 1:ff003039\n'
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp)
            (p / 'gt710-pixels.rle').write_bytes(startup)
            def capture(row, change=lambda a: a, serial_record=False):
                record = ('GT710_GR_REFUSAL_V1 ' + json.dumps(row) + '\n').encode()
                anchor = (f'GT710_GR_REFUSAL_BULK bytes={len(record)-1} '
                          f'sha256={hashlib.sha256(record[:-1]).hexdigest()}\n')
                (p / 'gt710-final.rle').write_bytes(record)
                (p / 'serial.txt').write_text(
                    'GT710_MODE {"schema":1}\n'
                    f'GT710_BULK {len(startup)} {hashlib.sha256(startup).hexdigest()}\n'
                    'SUITE gpu START cases=1\nSUITE gpu CASE device/version PASS ms=1\n'
                    + change(anchor) + (record.decode() if serial_record else '')
                    + 'SUITE gpu DONE passed=1 failed=0 skipped=0 total=1\n'
                    'SUITE gpu MEMORY reclaimed=PASS free_before=100 free_after=100 allowance=48\n'
                    f'GT710_FINAL_BULK {len(record)} {hashlib.sha256(record).hexdigest()}\n'
                    'SUITE_SEQUENCE READY gpu\n')
                return API['capture_serial'](p)
            self.assertIn('GT710_GR_REFUSAL_V1 ' + json.dumps(row), capture(row))
            pri = dict(row, pri_registers=[0x400100, 0x200, 0x409614, 0x404170],
                       pri_values=[0xbadf1201, 0xe011316d, 0x270, 0x10])
            self.assertIn('GT710_GR_REFUSAL_V1 ' + json.dumps(pri), capture(pri))
            for field, value in [('pri_registers', [0x400100]), ('pri_values', [0]*3),
                                 ('pri_values', [True, 0, 0, 0]), ('pri_values', [-1]*4)]:
                with self.subTest(field=field, value=value), self.assertRaises(ValueError):
                    capture(dict(pri, **{field: value}))
            for change in [lambda a: '', lambda a: a + a,
                           lambda a: a.replace('bytes=', 'bytes=0'),
                           lambda a: a.replace('sha256=', 'sha256=0')]:
                with self.assertRaises(ValueError):
                    capture(row, change)
            for field, value in [('status', [0]), ('status', [True, 0, 0, 0]),
                                 ('status', [-1, 0, 0, 0]),
                                 ('registers', REGISTERS[::-1]), ('values', [0]*6),
                                 ('values', [1 << 32]*7)]:
                with self.subTest(field=field, value=value), self.assertRaises(ValueError):
                    capture(dict(row, **{field: value}))
            with self.assertRaises(ValueError):
                capture(row, serial_record=True)


if __name__ == '__main__':
    unittest.main()

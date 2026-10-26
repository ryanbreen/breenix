#!/usr/bin/env python3
"""Trace receipt corruption and census separation, using synthetic evidence."""
import base64
import gzip
import hashlib
import io
from pathlib import Path
import sys
import tarfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'scripts/metal'))
import metal_census
import metal_trace


def frame():
    raw = io.BytesIO()
    with tarfile.open(fileobj=raw, mode='w') as archive:
        for name in sorted(metal_trace.FILES):
            value = b'synthetic evidence\n'
            member = tarfile.TarInfo(name)
            member.size = len(value)
            archive.addfile(member, io.BytesIO(value))
    data = gzip.compress(raw.getvalue())
    return ['metal-trace-start', f'TRACE {len(data)} {hashlib.sha256(data).hexdigest()}',
            'BEGIN metal-nouveau-trace-v1', *base64.encodebytes(data).decode().splitlines(),
            'END metal-nouveau-trace-v1', 'metal-trace-ok']


class ReceiptTests(unittest.TestCase):
    def test_receipt_and_corruption(self):
        lines = frame()
        receipt = metal_trace.Trace()
        for line in lines:
            receipt.feed(line)
        self.assertTrue(receipt.done)
        self.assertEqual(receipt.files.keys(), metal_trace.FILES)
        lines[3] = ('A' if lines[3][0] != 'A' else 'B') + lines[3][1:]
        with self.assertRaises(ValueError):
            receipt = metal_trace.Trace()
            for line in lines:
                receipt.feed(line)

    def test_missing_tail_and_wire_budget(self):
        receipt = metal_trace.Trace()
        for line in frame()[:-1]:
            receipt.feed(line)
        self.assertFalse(receipt.done)
        with self.assertRaises(ValueError):
            receipt.feed('metal-trace-start')
        receipt = metal_trace.Trace()
        receipt.feed('metal-trace-start')
        with self.assertRaises(ValueError):
            receipt.feed('TRACE 2000000 ' + '0' * 64)

    def test_progress_is_bounded_to_pre_frame(self):
        receipt = metal_trace.Trace()
        receipt.feed('metal-trace-start')
        self.assertFalse(receipt.feed('metal-trace-progress phase=module bytes=4096'))
        receipt.feed(frame()[1])
        with self.assertRaises(ValueError):
            receipt.feed('metal-trace-progress phase=module bytes=8192')

    def test_census_still_forbids_trace_kernel(self):
        config = '\n'.join('CONFIG_' + s + '=y' for s in
                           ('MMIOTRACE', 'EFI_STUB', 'BLK_DEV_INITRD', 'SERIAL_8250_CONSOLE', 'MODULES'))
        config += '\nCONFIG_DRM_NOUVEAU=m\nCONFIG_TG3=m\nCONFIG_SATA_AHCI=m\n'
        metal_trace.validate_config(config)
        with self.assertRaises(ValueError):
            metal_census.validate_config(config)
        with self.assertRaises(ValueError):
            metal_trace.validate_config(config.replace('CONFIG_TG3=m', 'CONFIG_TG3=y'))
        with self.assertRaises(ValueError):
            metal_trace.validate_config(config.replace('CONFIG_DRM_NOUVEAU=m', 'CONFIG_DRM_NOUVEAU=y'))


if __name__ == '__main__':
    unittest.main()

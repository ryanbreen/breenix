"""Authenticate first-CAPS bulk timing and retain the GPU IRQ bound."""
import hashlib
import json
from pathlib import Path
import runpy
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
API = runpy.run_path(str(ROOT / 'scripts/metal/gt710-suite'))


class StartupReport(unittest.TestCase):
    def capture(self, path, maximum=500, ready=True):
        row = dict(schema=1, irq_masked_max_us=maximum, ready=ready)
        data = ('GT710_CORE_STARTUP ' + json.dumps(row, separators=(',', ':')) + '\n').encode()
        anchor = f'{len(data)} {hashlib.sha256(data).hexdigest()}\n'
        (path / 'gt710-pixels.rle').write_bytes(data)
        (path / 'serial.txt').write_text('GT710_CORE_STARTUP_BULK ' + anchor + 'GT710_BULK ' + anchor)
        return API['capture_serial'](path)

    def test_exact_digest_and_unchanged_masked_bound(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            for maximum in [0, 1, 500]:
                self.assertEqual(API['core_startup'](self.capture(path, maximum)), maximum)
            for maximum in [-1, 501, 2001, True, '500']:
                with self.subTest(maximum=maximum), self.assertRaises(ValueError):
                    self.capture(path, maximum)
            with self.assertRaises(ValueError):
                self.capture(path, ready=False)
            joined = self.capture(path)
            for text in ['', joined + joined, joined.replace('"schema":1', '"schema":2')]:
                with self.subTest(text=text), self.assertRaises(ValueError):
                    API['core_startup'](text)
            serial = (path / 'serial.txt').read_text()
            for text in [serial.replace('GT710_CORE_STARTUP_BULK', 'MISSING'),
                         serial.replace('GT710_BULK', 'MISSING'),
                         serial + joined.splitlines()[0] + '\n']:
                (path / 'serial.txt').write_text(text)
                with self.assertRaises(ValueError):
                    API['capture_serial'](path)
            (path / 'serial.txt').write_text(serial)
            data = (path / 'gt710-pixels.rle').read_bytes()
            (path / 'gt710-pixels.rle').write_bytes(data.replace(b'500', b'499'))
            with self.assertRaises(ValueError):
                API['capture_serial'](path)


if __name__ == '__main__':
    unittest.main()

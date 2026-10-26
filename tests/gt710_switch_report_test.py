"""Judge mode switches from retained device state, EDID and full pixels."""
from pathlib import Path
import runpy
import unittest

ROOT = Path(__file__).resolve().parents[1]
API = runpy.run_path(str(ROOT / 'scripts/metal/gt710-suite'))


class AdvertisedMode(unittest.TestCase):
    def test_established_and_standard_dmt_need_the_exact_advertisement(self):
        raw = bytearray(128)
        raw[:8] = bytes([0, 255, 255, 255, 255, 255, 255, 0])
        raw[18:21] = bytes([1, 3, 128])
        expected = dict(width=1024, height=768, clock_khz=65000,
                        hsync_start=1048, hsync_end=1184, htotal=1344,
                        vsync_start=771, vsync_end=777, vtotal=806, flags=24)
        for bit, code, count in [(8, [1, 1], 1), (0, [0x61, 0x40], 1),
                                 (8, [0x61, 0x40], 1), (4, [1, 1], 0),
                                 (16, [1, 1], 0), (0, [0x61, 0x4a], 0)]:
            raw[36] = bit
            raw[38:40] = bytes(code)
            raw[127] = 0
            raw[127] = -sum(raw) & 255
            with self.subTest(bit=bit, code=code):
                self.assertEqual(API['edid_timings'](raw).count(expected), count)


if __name__ == '__main__':
    unittest.main()

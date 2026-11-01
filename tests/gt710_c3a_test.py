"""C3a: v24 core thresholds and the MIT GK104 sensor transaction."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'scripts/metal'))
from gt710_c1_test import image
import gt710_clock

class Thermal(unittest.TestCase):
    def test_v24_core_thresholds_and_bounds(self):
        rom = bytearray(image())
        rom[0x600] = 0x24
        rom[-1] = 0
        rom[-1] = -sum(rom) & 255
        result = gt710_clock.decode_image(bytes(rom))['thermal']
        self.assertEqual(result['version'], 0x24)
        self.assertEqual(result['thresholds'], {
            'critical': {'c': 95, 'hysteresis': 3},
            'down_clock': {'c': 90, 'hysteresis': 2},
            'fan_boost': {'c': 80, 'hysteresis': 4},
            'shutdown': {'c': 105, 'hysteresis': 5}})
        # Nouveau stops at the first positive (ambient) section; a later
        # core-looking section must not override the first core thresholds.
        rom[0x603] = 9
        rom[0x619:0x61f] = bytes([0, 0, 0, 7, 0x10, 0x02])
        self.assertEqual(gt710_clock.thermal(gt710_clock.Image(rom), 0x600)['thresholds'], result['thresholds'])
        for header, stride, count in [(3, 3, 7), (4, 2, 7), (4, 3, 255)]:
            bad = bytearray(rom[:0x630]);bad[0x601:0x604] = bytes([header, stride, count])
            with self.assertRaises(ValueError):
                gt710_clock.thermal(gt710_clock.Image(bad), 0x600)

    def test_sensor_setup_restores_only_fuse_access_and_reads_degrees(self):
        with tempfile.TemporaryDirectory() as directory:
            out = Path(directory)
            rustc = (['taskpolicy', '-b'] if sys.platform == 'darwin' else []) + ['rustc', '--edition=2021', '-Dwarnings']
            subprocess.run(rustc + ['--test', str(ROOT / 'tests/gt710_c3a_test.rs'), '-o', str(out / 'thermal')], check=True)
            subprocess.run([str(out / 'thermal')], check=True)

if __name__ == '__main__': unittest.main()

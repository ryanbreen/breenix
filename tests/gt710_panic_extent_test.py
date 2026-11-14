"""Native panic coverage follows the EDID; virtual-panel coverage stays fixed."""
from pathlib import Path
import runpy
import unittest
from gt710_panic_report_test import VALID
from gt710_runtime_catalogue_test import BOARD_TEXT

ROOT = Path(__file__).resolve().parents[1]
CHECK = runpy.run_path(str(ROOT / 'scripts/check-gpu-panic.py'))


class PanicExtent(unittest.TestCase):
    def test_native_requires_the_monitor_target_and_its_full_pixel_count(self):
        native = BOARD_TEXT + VALID.replace('pixels=4096000', 'pixels=1310720').replace(
            'target=42 bound=42', 'target=42 bound=42 width=1280 height=1024')
        self.assertTrue(CHECK['check_native'](native)[0])
        for bad in (native.replace('1310720', '4096000'),
                    native.replace('width=1280', 'width=2560'),
                    native.replace('height=1024', 'height=800'),
                    native.replace(BOARD_TEXT, ''),
                    native.replace('width=1280 ', ''),
                    native.replace('pixels=1310720', 'pixels=1310719')):
            with self.subTest(serial=bad):
                self.assertFalse(CHECK['check_native'](bad)[0])
        virtual = VALID[VALID.index('[BXCAP:BEGIN'):VALID.index('[GPU_PANIC_NATIVE')]
        self.assertTrue(CHECK['check'](virtual)[0])
        self.assertFalse(CHECK['check'](virtual.replace('4096000', '1310720'))[0])


if __name__ == '__main__':
    unittest.main()

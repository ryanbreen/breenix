"""The native restoring-divider loop must not join an exited BRK arm."""
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[1]


class DividerControlFlow(unittest.TestCase):
    def test_divider_break_has_no_fallthrough_reconvergence(self):
        # Shared routine 11 has one conditional, whose taken arm exits the loop.
        # MIT Mesa nv50_ir_from_nir.cpp:1605-1653 excludes that BRK from joins.
        tgsi = (ROOT / 'kernel/src/gpu/virgl/profile/r11.tgsi').read_text()
        self.assertEqual(len(re.findall(r'^UIF ', tgsi, re.M)), 1)
        self.assertRegex(tgsi, r'UIF [^\n]+\nBRK\nENDIF\nENDLOOP')
        native = (ROOT / 'kernel/assets/kepler/probe-int-11.asm').read_text()
        self.assertRegex(native, r'\bprebrk\b')
        self.assertRegex(native, r'\bbrk\b')
        self.assertFalse(bool(re.search(r'\bjoin(?:at)?\b', native)),
                            'a loop-exit arm does not converge on the fallthrough block')


if __name__ == '__main__':
    unittest.main()

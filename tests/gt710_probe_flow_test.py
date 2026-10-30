"""The native restoring-divider loop must not join an exited BRK arm."""
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[1]


class DividerControlFlow(unittest.TestCase):
    def test_spans_control_regions_fit_without_crs_storage(self):
        # Mesa nvc0_program.c:723-730 budgets two stack entries per control
        # region against 16 before requiring local spill storage. This native
        # program has no CRS storage. Include loop and divergent branch regions,
        # rather than counting only explicit JOINATs.
        native = (ROOT / 'kernel/assets/kepler/pipeline-coverage_spans.asm').read_text()
        regions = []
        for line in native.splitlines():
            match = re.match(r'^([0-9a-f]{8}):.*\b(joinat|prebrk|precont|bra) (0x[0-9a-f]+)', line)
            if not match:
                continue
            start, end = int(match[1], 16), int(match[3], 16)
            if end <= start or (match[2] == 'bra' and '$p' not in line):
                continue
            regions.append((start, end))
        self.assertTrue(regions)
        header = (ROOT / 'kernel/assets/kepler/pipeline-coverage_spans.sph').read_bytes()
        self.assertEqual(int.from_bytes(header[12:16], 'little') & 0xffffff, 0)
        for pc, _ in regions:
            entries = 2 * sum(start <= pc < end for start, end in regions)
            self.assertLessEqual(entries, 16, f'control stack needs spill storage at {pc:#x}')

    def test_shape_kill_survives_native_compilation(self):
        for name in ('line', 'circle_fill', 'circle_outline', 'rounded_rect', 'triangle', 'coverage_spans'):
            with self.subTest(pipeline=name):
                native = (ROOT / f'kernel/assets/kepler/pipeline-{name}.asm').read_text()
                self.assertTrue(re.search(r'\bdiscard\b', native) is not None,
                                'uncovered fragments must preserve the destination')

    def test_coverage_spans_limits_live_join_nesting(self):
        # Pinned MIT Mesa nv50_ir_from_nir.cpp:1640-1642 suppresses joins
        # beyond conditional depth six. Spans adds two enclosing loop entries.
        native = (ROOT / 'kernel/assets/kepler/pipeline-coverage_spans.asm').read_text()
        joins = [(int(m[1], 16), int(m[2], 16)) for m in re.finditer(
            r'^([0-9a-f]{8}):[^\n]*\bjoinat (0x[0-9a-f]+)', native, re.M)]
        self.assertTrue(joins)
        for pc, _ in joins:
            self.assertLessEqual(sum(start <= pc < end for start, end in joins), 6,
                                 f'too many live reconvergence entries at {pc:#x}')

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

"""G35(3): memory verdicts use the runner's allowance, never the guest's choice."""
from pathlib import Path
import re
import runpy
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / 'libs/libbreenix/src/suite_memory.rs'
SCORER = runpy.run_path(str(ROOT / 'scripts/suite-verdict.py'))
METAL = runpy.run_path(str(ROOT / 'scripts/metal/gt710-suite'))


def serial(allowance, retained=49, verdict='PASS'):
    return '\n'.join([
        'SUITE breengel START cases=1',
        'SUITE breengel MEMORY before free=1000 heap_free_kb=100 user_pages=100',
        'SUITE breengel CASE fixture/one PASS ms=1',
        'SUITE breengel DONE passed=1 failed=0 skipped=0 total=1',
        f'SUITE breengel MEMORY after free={1000-retained} heap_free_kb=100 user_pages={100+retained}',
        f'SUITE breengel MEMORY reclaimed={verdict} free_before=1000 free_after={1000-retained} allowance={allowance}',
    ]) + '\n'


def records(text):
    shapes = {
        'START': r'SUITE breengel START .*',
        'PASS': r'SUITE breengel CASE .*',
        'DONE': r'SUITE breengel DONE .*',
        'MEMORY': r'SUITE breengel MEMORY (before|after) free=(\d+) .*',
        'RECLAIMED': r'SUITE breengel MEMORY reclaimed=(PASS|FAIL) free_before=(\d+) free_after=(\d+) allowance=(\d+)',
    }
    return [(kind, match, line) for line in text.splitlines()
            for kind, shape in shapes.items() if (match := re.fullmatch(shape, line))]


class MemoryAllowance(unittest.TestCase):
    def test_runner_allowance_stays_48(self):
        self.assertEqual(re.findall(
            r'^pub const MEMORY_RECLAIM_ALLOWANCE_PAGES: u64 = (\d+);$',
            SOURCE.read_text(), re.M), ['48'])

    def test_retained_definition_carries_the_allowance_source(self):
        self.assertIn("'libs/libbreenix/src/suite_memory.rs'",
                      (ROOT / 'scripts/metal/metal-build').read_text())

    def test_common_verdict_refuses_noncanonical_allowance(self):
        for allowance in (64, 47):
            with self.subTest(allowance=allowance):
                ok, why = SCORER['memory_problem'](records(serial(allowance, retained=0)))
                self.assertFalse(ok, why)
                self.assertIn('allowance', why)
        ok, why = SCORER['memory_problem'](records(serial(64)))
        self.assertFalse(ok, why)
        self.assertIn('allowance', why)
        self.assertTrue(SCORER['memory_problem'](records(serial(48, retained=48)))[0])
        self.assertFalse(SCORER['memory_problem'](records(serial(48)))[0])

    def test_production_breengel_pass_requires_48(self):
        with self.assertRaisesRegex(ValueError, 'allowance'):
            METAL['breengel_memory_accounting'](serial(64))
        self.assertEqual(METAL['breengel_memory_accounting'](serial(48, retained=48)),
                         dict(status='PASS', reason_pages=0))

    def test_artifact_memory_verdict_requires_48(self):
        # Artifact loading also accepts memory boundaries before a category scorer runs.
        for suite in ('console', 'gpu', 'breengel'):
            with self.subTest(suite=suite), self.assertRaisesRegex(ValueError, 'allowance'):
                METAL['capture_serial_body'](ROOT, serial(64).replace('breengel', suite))

    def test_allowance_is_read_from_the_runner_source_and_missing_source_refuses(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / SOURCE.relative_to(ROOT)
            source.parent.mkdir(parents=True)
            # Vary only the fixture source to distinguish a source read from a literal 48.
            source.write_text('pub const MEMORY_RECLAIM_ALLOWANCE_PAGES: u64 = 47;\n')
            with patch.dict(SCORER['memory_problem'].__globals__, REPO=root):
                self.assertTrue(SCORER['memory_problem'](records(serial(47, retained=47)))[0])
                self.assertFalse(SCORER['memory_problem'](records(serial(48, retained=47)))[0])
                source.unlink()
                self.assertFalse(SCORER['memory_problem'](records(serial(47, retained=47)))[0])


if __name__ == '__main__':
    unittest.main()

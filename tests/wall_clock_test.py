"""The host UTC gate must reject local time, missing and incomplete records."""
from pathlib import Path
import runpy
import tempfile
import unittest

Clock = runpy.run_path(str(Path(__file__).resolve().parents[1] / 'scripts/wall_clock.py'))['WallClockCheck']


class WallClockTest(unittest.TestCase):
    def check(self, offset, source='UEFI', newline=True):
        with tempfile.TemporaryDirectory() as directory:
            clock = Clock(Path(directory) / 'serial', 1800000000.5)
            clock.observe(f'[clock] UTC epoch={1800000000 + offset} source={source}' + ('\n' if newline else ''), 1800000000.5)
            return clock.verdict()[0]

    def test_each_platform_source_matches_host(self):
        for source in ('CMOS', 'PL031', 'UEFI'):
            self.assertTrue(self.check(0, source))

    def test_positive_and_negative_local_offsets_fail(self):
        for offset in (-43200, -14400, -3600, 3600, 19800, 43200):
            self.assertFalse(self.check(offset))

    def test_launch_receipt_bracket(self):
        self.assertTrue(self.check(0))
        self.assertFalse(self.check(-1))
        self.assertFalse(self.check(1))

    def test_stalled_receipt_preserves_boot_epoch(self):
        with tempfile.TemporaryDirectory() as directory:
            clock = Clock(Path(directory) / 'serial', 1800000000.5)
            clock.observe('[clock] UTC epoch=1800000000 source=PL031\n', 1800000030.5)
            self.assertTrue(clock.verdict()[0])

    def test_stale_sidecar_is_removed_even_without_a_record(self):
        with tempfile.TemporaryDirectory() as directory:
            serial = Path(directory) / 'serial'
            sidecar = Path(str(serial) + '.utc.json')
            sidecar.write_text('{"passed":true}')
            clock = Clock(serial)
            self.assertFalse(sidecar.exists())
            self.assertFalse(clock.verdict()[0])

    def test_incomplete_record_does_not_count(self):
        self.assertFalse(self.check(0, newline=False))

    def test_announcements_and_missing_records_fail(self):
        with tempfile.TemporaryDirectory() as directory:
            clock = Clock(Path(directory) / 'serial', 1800000000.5)
            clock.observe('kernel::test_exec: -> Userspace will emit [clock] UTC epoch=1800000000 source=CMOS\n', 1800000000)
            self.assertFalse(clock.verdict()[0])

    def test_duplicate_records_fail(self):
        with tempfile.TemporaryDirectory() as directory:
            clock = Clock(Path(directory) / 'serial', 1800000000.5)
            clock.observe('[clock] UTC epoch=1800000000 source=CMOS\n' * 2, 1800000000)
            self.assertFalse(clock.verdict()[0])


if __name__ == '__main__':
    unittest.main()

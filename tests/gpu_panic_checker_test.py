"""Exercise stopped-boot acceptance and malformed record handling."""
import runpy
from pathlib import Path
import unittest

check = runpy.run_path(str(Path(__file__).resolve().parents[1] / "scripts/check-gpu-panic.py"))["check"]
VALID = ("[BXCAP:BEGIN seq=1 edge=PANIC]\n"
         "[BXCAP:END seq=1 edge=PANIC verdict=complete truncated=0 sections_skipped=0]\n"
         "[GPU_PANIC_SCREEN shown=1 observed=1 pixels=4096000 mismatches=0]\n")


class PanicCheckerTest(unittest.TestCase):
    def test_complete_capture(self):
        self.assertTrue(check(VALID)[0])

    def test_reject_bad_capture_or_screen(self):
        for bad in (
            VALID.replace("seq=1 ", ""),
            VALID.replace("END seq=1", "END seq=2"),
            VALID.replace("truncated=0", "truncated=1"),
            VALID.replace("sections_skipped=0", "sections_skipped=1"),
            VALID.replace("observed=1", "observed=0"),
            VALID.replace("pixels=4096000", "pixels=1"),
            VALID.replace("mismatches=0", "mismatches=1"),
            VALID + "soft-lockup\n",
            VALID + VALID,
            VALID.splitlines()[2] + "\n" + "\n".join(VALID.splitlines()[:2]),
        ):
            with self.subTest(serial=bad):
                self.assertFalse(check(bad)[0])

    def test_order_uses_matched_record(self):
        # An incomplete literal prefix is not the complete END record.
        bad = "[BXCAP:END \n" + VALID.splitlines()[0] + "\n" + VALID.splitlines()[2] + "\n" + VALID.splitlines()[1]
        self.assertFalse(check(bad)[0])


if __name__ == "__main__":
    unittest.main()

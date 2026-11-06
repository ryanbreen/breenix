"""Exercise production native present routing for retained and changed bindings."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tests'))
from gt710_gr_live_unwind_test import item
from gt710_receipt_timing_test import source


class RetainedAcceptance(unittest.TestCase):
    def test_retained_refresh_uses_fresh_ack_and_real_flips_keep_their_latch(self):
        native = source('kernel/src/gpu/gk208/native.rs')
        code = item(native, '\nfn present_inner(')
        fixture = (ROOT / 'tests/fixtures/gt710_retained_acceptance.rs').read_text()
        fixture = fixture.replace('@ROOT@', str(ROOT)).replace('// PRESENT_INNER', code)
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp)
            (p / 'retained.rs').write_text(fixture)
            subprocess.run(priority + ['rustc', '--edition=2021', '--test', '-Dwarnings', str(p / 'retained.rs'), '-o', str(p / 'retained')], check=True)
            subprocess.run([str(p / 'retained'), '--nocapture'], check=True)


if __name__ == '__main__':
    unittest.main()

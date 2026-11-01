"""C2 coefficient and bounded register programming checks against MIT nouveau."""
from pathlib import Path
import os
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]

class CoreClock(unittest.TestCase):
    def test_nouveau_coefficients_divider_program_and_post_ceiling(self):
        module = ROOT / 'kernel/src/gpu/gk208/c2.rs'
        self.assertTrue(module.exists(), 'C2 GPC control is not implemented')
        with tempfile.TemporaryDirectory() as directory:
            out = Path(directory)
            subprocess.run(['cc', '-Wall', '-Wextra', '-Werror', str(ROOT / 'tests/fixtures/gt710_pll_oracle.c'), '-o', str(out / 'oracle')], check=True)
            priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
            subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings', '--test', str(ROOT / 'tests/gt710_c2_test.rs'), '-o', str(out / 'check')], check=True)
            subprocess.run([str(out / 'check'), '--nocapture'], env=dict(os.environ, GT710_PLL_ORACLE=str(out / 'oracle')), check=True)

if __name__ == '__main__':
    unittest.main()

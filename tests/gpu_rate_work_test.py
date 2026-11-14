"""Count rate-frame commands and pixel traffic against the main clear loop (#2977)."""
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest
from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


class RateWork(unittest.TestCase):
    def test_rate_loops_only_do_rendering_presentation_and_original_corner_checks(self):
        source = (ROOT / 'userspace/programs/src/suite_gpu/performance.rs').read_text()
        constants = '\n'.join(re.findall(r'^const (?:W|H|BG|FG|FRAMES|CLEARS_PER_S|PRESENTS_PER_S):[^;]+;', source, re.M))
        helpers = source[source.index('fn present('):source.index('/// Colour')]
        code = (ROOT / 'tests/fixtures/gpu_rate_work.rs').read_text()
        code = code.replace('// CONSTANTS', constants).replace('// PRESENT', helpers)
        for name in ['colour', 'rate_met', 'clear_rate_2560', 'present_rate_2560']:
            code += '\n' + item(source, ('pub fn ' if name.endswith('2560') else 'fn ') + name + '(')
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp)
            (p / 'check.rs').write_text(code)
            lib = p / 'libgpu_abi.rlib'
            subprocess.run(priority + ['rustc', '--edition=2021', '--crate-name', 'gpu_abi', '--crate-type', 'rlib',
                            str(ROOT / 'libs/gpu-abi/src/lib.rs'), '-o', str(lib)], check=True)
            subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings', str(p / 'check.rs'),
                            '--extern', f'gpu_abi={lib}', '-o', str(p / 'check')], check=True)
            for case in ['clear', 'present']:
                with self.subTest(case=case):
                    subprocess.run([str(p / 'check'), case], check=True)


if __name__ == '__main__':
    unittest.main()

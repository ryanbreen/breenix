"""Time the production checkpoint calls with deterministic operation clocks."""
from pathlib import Path
import os
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class ArenaCallTiming(unittest.TestCase):
    def test_checkpoint_reports_calls_before_return(self):
        rel = 'libs/libuitest/src/arena.rs'
        old = os.environ.get('UITEST_OLD_COMMIT')
        source = (subprocess.check_output(['git', 'show', f'{old}:{rel}'], cwd=ROOT, text=True)
                  if old else (ROOT / rel).read_text())
        source = source.replace('//!', '//')
        source = source.replace('fs::{File, OpenOptions}, ', '')
        source = source.replace('std::fs::', 'crate::fixture_fs::')
        source = source.replace('std::time::Instant', 'crate::Clock')
        source = source.replace('    fn mmap(', '    #[link_name="system_mmap"]\n    fn mmap(')
        source = source.replace('    fn munmap(', '    #[link_name="system_munmap"]\n    fn munmap(')
        source = source.replace('    fn clock_gettime(', '    #[link_name="fixture_clock_gettime"]\n    fn clock_gettime(')
        source = 'use crate::fixture_fs::{File, OpenOptions};\n' + source
        fixture = (ROOT / 'tests/fixtures/arena_call_timing.rs').read_text()
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            (path / 'arena.rs').write_text(source)
            (path / 'check.rs').write_text(fixture)
            def run(args):
                if sys.platform == 'darwin':
                    args = ['taskpolicy', '-b', *args]
                result = subprocess.run(args, cwd=ROOT, capture_output=True, text=True,
                                        env={**os.environ, 'CARGO_BUILD_JOBS': '4'})
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                return result
            run(['rustc', '--edition=2021', '--crate-name', 'libpixcheck', '--crate-type=rlib',
                 '-Dwarnings', 'libs/libpixcheck/src/lib.rs', '-o', str(path / 'liblibpixcheck.rlib')])
            run(['rustc', '--edition=2021', '-Dwarnings', str(path / 'check.rs'),
                 '--extern', f'libpixcheck={path}/liblibpixcheck.rlib', '-o', str(path / 'check')])
            result = run([str(path / 'check')])
            print(result.stdout)


if __name__ == '__main__':
    unittest.main()

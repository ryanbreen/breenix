"""Host checks over the production shared mapping and local artifact writer."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
ROOT = Path(__file__).resolve().parents[1]
class ArenaChecks(unittest.TestCase):
    def test_arena_and_local_artifacts(self):
        with tempfile.TemporaryDirectory() as d:
            d = Path(d)
            def run(args):
                if sys.platform == 'darwin': args = ['taskpolicy', '-b', *args]
                result = subprocess.run(args, cwd=ROOT, capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                return result
            run(['rustc', '--edition=2021', '--crate-name', 'libpixcheck', '--crate-type=rlib', '-Dwarnings', 'libs/libpixcheck/src/lib.rs', '-o', str(d/'liblibpixcheck.rlib')])
            run(['rustc', '--edition=2021', '--test', '-Dwarnings', 'tests/uitest_arena.rs', '--extern', f'libpixcheck={d}/liblibpixcheck.rlib', '-o', str(d/'check')])
            result = run([str(d/'check')])
            print(result.stdout)
if __name__ == '__main__': unittest.main()

"""Exercise selection lifetime through bterm's production terminal wrapper."""
from pathlib import Path
import os
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class ScreenSelection(unittest.TestCase):
    def test_screen_replacement(self):
        source = (ROOT / 'userspace/programs/src/bterm.rs').read_text()
        wrapper = source[source.index('struct TermEmu {'):source.index('    /// Paint changed cells:')]
        fixture = (ROOT / 'tests/fixtures/bterm_screen_selection_host.rs').read_text()
        fixture = fixture.replace('SELECTION_PATH', str(ROOT / 'userspace/programs/src/bterm/selection.rs'))
        fixture = fixture.replace('// TERMINAL_CODE', wrapper + '\n}')
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        env = dict(os.environ, CARGO_BUILD_JOBS='4')
        with tempfile.TemporaryDirectory(prefix='bterm-selection-') as directory:
            path = Path(directory)
            (path / 'check.rs').write_text(fixture)
            subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings', '--crate-type=rlib',
                '--crate-name=libvt', str(ROOT / 'libs/libvt/src/lib.rs'), '-o', str(path / 'libvt.rlib')], check=True, env=env)
            subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings', '--test',
                '--extern', f'libvt={path / "libvt.rlib"}', str(path / 'check.rs'), '-o', str(path / 'check')], check=True, env=env)
            subprocess.run([str(path / 'check')], check=True)


if __name__ == '__main__':
    unittest.main()

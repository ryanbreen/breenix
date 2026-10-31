"""Execute console owner painting/observation against separate GPU and BAR stores.

The native console uses the same scanout API as ARM. A legacy framebuffer flush
refuses and its BAR contains a different image, reproducing the metal regression.
"""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tests'))
from gt710_gr_live_unwind_test import item


class ConsoleGpuOwner(unittest.TestCase):
    def test_native_owner_paints_and_reads_the_shared_scanout(self):
        source = (ROOT / 'userspace/programs/src/suite_console.rs').read_text()
        start = source.index('/// What the display owner painted:')
        end = source.index('fn screen_display_taken_stops_drawing()')
        functions = source[start:end].replace('#[cfg(target_arch = "x86_64")]', '#[cfg(all())]').replace('#[cfg(target_arch = "aarch64")]', '#[cfg(any())]')
        fixture = (ROOT / 'tests/fixtures/console_gpu_owner.rs').read_text()
        # Keep the production x86 cfg selection. The shared gpu_display query is
        # injected as admitted; paint/read operations execute production code.
        fixture = fixture.replace('// OWNER_FUNCTIONS', functions)
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            (path / 'owner.rs').write_text(fixture)
            subprocess.run(priority + ['rustc', '--edition=2021', '--test',
                str(path / 'owner.rs'), '-o', str(path / 'owner')], check=True)
            subprocess.run([str(path / 'owner'), '--nocapture'], check=True)


if __name__ == '__main__':
    unittest.main()

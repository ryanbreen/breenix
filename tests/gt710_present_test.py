"""Exercise retained EVO state and presentation unwind at injected boundaries.

These host cases execute production slices; device pixels remain the metal
scorer's responsibility.
"""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tests'))
from gt710_gr_live_unwind_test import item


class Present(unittest.TestCase):
    def test_retained_state_and_console_restore(self):
        native = (ROOT / 'kernel/src/gpu/gk208/native.rs').read_text()
        gr = (ROOT / 'kernel/src/gpu/gk208/gr/mod.rs').read_text()
        handle = item(native, 'pub struct Evo {') + '\n' + item(native, 'impl Evo {')
        flip = item(native, 'fn flip(')
        flow = item(gr, 'fn present_probe(').replace('super::native::', 'native::').replace('super::copy::', 'copy::').replace('super::hash(', 'hash(')
        fixture = (ROOT / 'tests/fixtures/gt710_present.rs').read_text()
        fixture = fixture.replace('// HANDLE', handle + '\n' + flip).replace('// FLOW', flow)
        # The takeover returns its final cursor, rather than a fresh channel.
        takeover = item(native, 'pub fn takeover(')
        self.assertIn('Result<(ConsoleSurface, Evo)', takeover)
        self.assertIn('Ok((console, Evo {', takeover)
        self.assertIn('push: evo, put, notifier', takeover)
        self.assertIn('flip(regs,total,evo,put,notifier,head,', takeover)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            (path / 'present.rs').write_text(fixture)
            priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
            subprocess.run(priority + ['rustc', '--edition=2021', '--test', '-Dwarnings', str(path / 'present.rs'), '-o', str(path / 'present')], check=True, cwd=ROOT)
            result = subprocess.run([str(path / 'present'), '--nocapture'], check=True, capture_output=True, text=True)
            print(result.stdout)


if __name__ == '__main__':
    unittest.main()

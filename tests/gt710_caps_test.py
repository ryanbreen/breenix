"""Build the native CAPS fixture and observation checks on the host."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
ROOT = Path(__file__).resolve().parents[1]
class NativeCaps(unittest.TestCase):
    def test_caps_and_observation(self):
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as directory:
            out = Path(directory)
            subprocess.run(priority + ['rustc', '--edition=2021', '--crate-name', 'gpu_abi',
                '--crate-type', 'rlib', str(ROOT / 'libs/gpu-abi/src/lib.rs'),
                '-o', str(out / 'libgpu_abi.rlib')], check=True, cwd=ROOT)
            source = (ROOT / 'tests/gt710_caps_test.rs').read_text().replace('../kernel/', str(ROOT / 'kernel') + '/')
            (out / 'caps.rs').write_text(source)
            subprocess.run(priority + ['rustc', '--edition=2021', '--test', '-Dwarnings',
                '--extern', f'gpu_abi={out / "libgpu_abi.rlib"}',
                str(out / 'caps.rs'), '-o', str(out / 'caps')], check=True, cwd=ROOT)
            subprocess.run([str(out / 'caps')], check=True)
if __name__ == '__main__':
    unittest.main()

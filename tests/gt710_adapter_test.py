"""Run native adapter lifecycle checks without a device or a kernel boot."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]

class NativeAdapter(unittest.TestCase):
    def test_native_completion_and_room(self):
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as directory:
            out = Path(directory)
            subprocess.run(priority + ['rustc', '--edition=2021', '--crate-name', 'gpu_abi',
                '--crate-type', 'rlib', str(ROOT / 'libs/gpu-abi/src/lib.rs'),
                '-o', str(out / 'libgpu_abi.rlib')], check=True, cwd=ROOT)
            source = (ROOT / 'tests/gt710_adapter_test.rs').read_text()
            source = source.replace('../../kernel/', str(ROOT / 'kernel') + '/')
            source = source.replace('../kernel/', str(ROOT / 'kernel') + '/')
            (out / 'adapter.rs').write_text(source)
            subprocess.run(priority + ['rustc', '--edition=2021', '--test', '-Dwarnings',
                '--extern', f'gpu_abi={out / "libgpu_abi.rlib"}',
                str(out / 'adapter.rs'), '-o', str(out / 'adapter')], check=True, cwd=ROOT)
            subprocess.run([str(out / 'adapter')], check=True)

if __name__ == '__main__':
    unittest.main()

"""Build and execute the native prepared stream/deadline check on a host."""
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]

class PreparedPanic(unittest.TestCase):
    def test_prepared_stream_and_terminal_failures(self):
        with tempfile.TemporaryDirectory() as tmp:
            lib = Path(tmp) / 'libgpu_abi.rlib'
            subprocess.run(['rustc', '--edition=2021', '--crate-type=rlib', '--crate-name=gpu_abi',
                            '--cfg', 'feature="alloc"', str(ROOT / 'libs/gpu-abi/src/lib.rs'), '-o', str(lib)], check=True)
            binary = Path(tmp) / 'prepared'
            subprocess.run(['rustc', '--edition=2021', '--test', '-Dwarnings',
                            '--extern', f'gpu_abi={lib}', str(ROOT / 'tests/gt710_panic_prepared_test.rs'),
                            '-o', str(binary)], check=True)
            subprocess.run([str(binary)], check=True)

if __name__ == '__main__':
    unittest.main()

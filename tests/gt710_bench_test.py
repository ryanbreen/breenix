"""Compile the userspace baseline against the real ABI and a recording device."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class Baseline(unittest.TestCase):
    def test_production_streams_and_completion_boundary(self):
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp)
            rustc = (['taskpolicy', '-b'] if sys.platform == 'darwin' else []) + ['rustc', '--edition=2021', '-Dwarnings']
            subprocess.run(rustc + ['--crate-name', 'gpu_abi', '--crate-type', 'rlib',
                                   str(ROOT / 'libs/gpu-abi/src/lib.rs'), '-o', str(out / 'libgpu_abi.rlib')], check=True)
            subprocess.run(rustc + ['--test', str(ROOT / 'tests/gt710_bench_test.rs'),
                                   '--extern', f'gpu_abi={out}/libgpu_abi.rlib', '-o', str(out / 'bench')], check=True)
            result = subprocess.run([str(out / 'bench'), '--nocapture', '--test-threads=1'], check=True, text=True, capture_output=True)
            import importlib.util
            spec = importlib.util.spec_from_file_location('clock', ROOT / 'scripts/metal/gt710_clock.py')
            clock = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(clock)
            import json
            rows = [json.loads(line.split('GT710_BENCH ', 1)[1]) for line in result.stdout.splitlines() if 'GT710_BENCH ' in line]
            self.assertEqual(len(clock.benchmarks(rows)), 6)


if __name__ == '__main__':
    unittest.main()

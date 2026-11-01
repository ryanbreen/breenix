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
            subprocess.run(rustc + ['--crate-name', 'gpu_abi', '--crate-type', 'rlib', '--cfg', 'feature="alloc"',
                                   str(ROOT / 'libs/gpu-abi/src/lib.rs'), '-o', str(out / 'libgpu_abi.rlib')], check=True)
            subprocess.run(rustc + ['--test', str(ROOT / 'tests/gt710_bench_test.rs'),
                                   '--extern', f'gpu_abi={out}/libgpu_abi.rlib', '-o', str(out / 'bench')], check=True)
            result = subprocess.run([str(out / 'bench'), '--nocapture', '--test-threads=1'], text=True, capture_output=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            import json
            # Both C2 sequences complete fifteen trials, including a low-clock
            # trial below 900 ms. Failed completion contributes no trial.
            rows = [json.loads(line.split('GT710_BENCH ', 1)[1]) for line in result.stdout.splitlines() if 'GT710_BENCH ' in line]
            temperatures = [json.loads(line.split('GT710_TEMPERATURE ', 1)[1]) for line in result.stdout.splitlines() if 'GT710_TEMPERATURE ' in line]
            self.assertEqual(len(temperatures), 5)
            self.assertTrue(all(row['registers']==[[512,3760267629]] for row in temperatures))
            self.assertEqual(len(rows), 89)
            self.assertTrue(all(row['completion'] for row in rows))
            core = [row for row in rows if row['kind']=='core']
            gpc = [row for row in rows if row['kind']=='gpc']
            self.assertEqual((len(core),len(gpc)), (62,27))
            self.assertTrue(all(row['work']==819200000 for row in core))
            self.assertTrue(all(row['work']==524288 and row['batches']==8 for row in gpc))



if __name__ == '__main__':
    unittest.main()

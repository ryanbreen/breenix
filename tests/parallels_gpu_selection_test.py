"""Exercise run.sh suite resolution and sampler launch without starting a VM."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
SOURCE = (ROOT / "run.sh").read_text()


class GpuSelectionTest(unittest.TestCase):
    def resolve(self, suites):
        start = SOURCE.index("# --suite ID:")
        end = SOURCE.index('if [ -n "$ONLY" ]; then\n    [ -n "$SUITE" ]', start)
        result = subprocess.run(
            ["bash", "-c", SOURCE[start:end] + '\nprintf "%s\\n%s\\n" "$SUITE" "$GPU_CORE"'],
            env={**os.environ, "BREENIX_ROOT": str(ROOT), "SUITE": suites,
                 "PARALLELS": "true", "VMWARE": "false", "GPU_CORE": "false", "ONLY": ""},
            capture_output=True, text=True, check=True)
        return result.stdout.splitlines()

    def test_all_resolves_before_gpu_core_selection(self):
        suites, core = self.resolve("all")
        self.assertIn("gpu", suites.split(","))
        self.assertEqual(core, "true")

    def test_explicit_sequence_selects_gpu_core(self):
        self.assertEqual(self.resolve("gpu,memory"), ["gpu,memory", "true"])

    def test_sampler_starts_for_gpu_in_a_sequence(self):
        start = SOURCE.index('    GPU_TIME_RECORD=""')
        end = SOURCE.index('    echo ""', start)
        with tempfile.TemporaryDirectory() as directory:
            record = Path(directory) / "sampler"
            shell = ('python3() { printf \'%s\\n\' "$@" > "$SAMPLE_ARGS"; }\n'
                     + SOURCE[start:end] + '\nwait\nprintf "%s" "$GPU_TIME_RECORD"')
            for suites in ["gpu", "gpu,memory", "memory,gpu"]:
                with self.subTest(suites=suites):
                    result = subprocess.run(["bash", "-c", shell], env={**os.environ,
                        "BREENIX_ROOT": str(ROOT), "SUITE": suites, "ONLY": "",
                        "SERIAL_LOG": "serial", "PARALLELS_VM": "test-vm", "SAMPLE_ARGS": str(record)},
                        capture_output=True, text=True, check=True)
                    self.assertEqual(result.stdout, "serial.gpu-time.jsonl")
                    self.assertIn("sample", record.read_text().splitlines())


if __name__ == "__main__":
    unittest.main()

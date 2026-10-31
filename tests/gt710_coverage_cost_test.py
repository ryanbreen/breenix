"""Execute the native submit encoder selected by production with a payload budget."""
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
PRIORITY = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []


class CoverageCost(unittest.TestCase):
    def test_encoded_stream_cost(self):
        core = (ROOT / 'kernel/src/gpu/mod.rs').read_text()
        native = core.split('let cost_translate =', 1)[1]
        name = re.search(r'crate::gk208::encode::(\w+)\(&words, &resolved\)', native)[1]
        source = (ROOT / 'tests/gt710_coverage_cost_test.rs').read_text()
        source = source.replace('RUNTIME_TRANSLATE', name).replace('../kernel/', str(ROOT / 'kernel') + '/')
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp)
            (p / 'check.rs').write_text(source)
            subprocess.run(PRIORITY + ['rustc', '--edition=2021', '-Dwarnings', '--cfg', 'feature="alloc"',
                '--crate-name', 'gpu_abi', '--crate-type', 'rlib', str(ROOT / 'libs/gpu-abi/src/lib.rs'),
                '-o', str(p / 'libgpu_abi.rlib')], check=True)
            subprocess.run(PRIORITY + ['rustc', '--edition=2021', '-Dwarnings', '--test',
                '--extern', f'gpu_abi={p / "libgpu_abi.rlib"}', str(p / 'check.rs'), '-o', str(p / 'check')], check=True)
            subprocess.run([str(p / 'check'), '--test-threads=1', '--nocapture'], check=True)

    def test_phase_fields_reach_the_case_scorer(self):
        files = ['kernel/src/gpu/gk208/cost.rs', 'libs/gpu-abi/src/heap_trace.rs', 'scripts/metal/gt710-suite']
        for path in files:
            text = (ROOT / path).read_text()
            for phase in ['scratch_prepare', 'scratch_copy', 'draw_execute', 'native_submit']:
                for suffix in ['_n', '_ns']:
                    self.assertIn('"' + phase + suffix + '"' if path.endswith('.rs') else
                                  "'" + phase + suffix + "'", text, (path, phase, suffix))


if __name__ == '__main__':
    unittest.main()

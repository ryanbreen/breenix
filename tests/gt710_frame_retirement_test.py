"""Run the production native draw publisher at its whole-frame storage release boundary."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tests'))
from gt710_gr_live_unwind_test import item


class FrameRetirement(unittest.TestCase):
    def test_release_rebinds_the_whole_frame_before_unmap(self):
        source = (ROOT / 'kernel/src/gpu/gk208/gr/channel.rs').read_text()
        publisher = item(source, '        pub(crate) fn publish_draw(')
        report = item(source, 'pub fn report(')
        harness = (ROOT / 'tests/fixtures/gt710_frame_retirement.rs').read_text()
        harness = harness.replace('// PUBLISH_DRAW', publisher).replace('// REPORT', report)
        harness = harness.replace('ROOT/', str(ROOT) + '/')
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            (path / 'retirement.rs').write_text(harness)
            subprocess.run(priority + ['rustc', '--edition=2021', '--crate-name', 'gpu_abi',
                           '--crate-type', 'rlib', '--cfg', 'feature="alloc"', '-Dwarnings',
                           str(ROOT / 'libs/gpu-abi/src/lib.rs'), '-o', str(path / 'libgpu_abi.rlib')], check=True)
            subprocess.run(priority + ['rustc', '--edition=2021', '--test', '-Dwarnings',
                           '--extern', f'gpu_abi={path / "libgpu_abi.rlib"}',
                           str(path / 'retirement.rs'), '-o', str(path / 'retirement')], check=True)
            subprocess.run([str(path / 'retirement')], check=True)


if __name__ == '__main__':
    unittest.main()

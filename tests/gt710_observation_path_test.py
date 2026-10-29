"""Execute production CAPS and READBACK record construction on the host.

Only the device, core storage and binding inputs are substituted. The record
construction comes from read_back, rather than calling the native row helper.
GT710_OBSERVATION_REV selects the source revision for the red check.
"""
from pathlib import Path
import os
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tests'))
from gt710_gr_live_unwind_test import item


class ObservationPath(unittest.TestCase):
    def test_production_caps_install_and_observation_row(self):
        revision = os.environ.get('GT710_OBSERVATION_REV')

        def source(path):
            if revision:
                return subprocess.check_output(['git', 'show', f'{revision}:{path}'], cwd=ROOT, text=True)
            return (ROOT / path).read_text()

        core = source('kernel/src/gpu/mod.rs')
        readback = item(core, 'fn read_back(')
        # Execute the production serializer through the boundary before user-copy.
        row = readback[readback.index('    let mut rec ='):readback.index('    // Publish pixels')]
        fixture = (ROOT / 'tests/fixtures/gt710_observation_path.rs').read_text()
        fixture = fixture.replace('// INSTALL', item(core, 'pub(crate) fn install_native('))
        fixture = fixture.replace('// CAPS', item(core, '    fn caps_record('))
        fixture = fixture.replace('// RECORD', row).replace('@ROOT@', str(ROOT))
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory(prefix='gt710-observation-') as directory:
            out = Path(directory)
            (out / 'observation.rs').write_text(source('kernel/src/gpu/observation.rs'))
            for name, path in [('caps', 'kernel/src/gpu/gk208/caps.rs'),
                               ('completion', 'kernel/src/gpu/completion.rs')]:
                (out / f'{name}.rs').write_text(source(path))
            (out / 'fixture.rs').write_text(fixture)
            subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings',
                '--crate-name', 'gpu_abi', '--crate-type', 'rlib',
                str(ROOT / 'libs/gpu-abi/src/lib.rs'), '-o', str(out / 'libgpu_abi.rlib')], check=True)
            subprocess.run(priority + ['rustc', '--edition=2021', '--test', '-Dwarnings',
                '--extern', f'gpu_abi={out / "libgpu_abi.rlib"}',
                str(out / 'fixture.rs'), '-o', str(out / 'fixture')], check=True)
            subprocess.run([str(out / 'fixture'), '--test-threads=1'], check=True)


if __name__ == '__main__':
    unittest.main()

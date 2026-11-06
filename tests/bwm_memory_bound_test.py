"""Path-include bwm's production allocation sites with host-only GPU boundaries."""
from pathlib import Path
import os
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class MemoryBound(unittest.TestCase):
    def test_660_epochs(self):
        with tempfile.TemporaryDirectory(prefix='bwm-memory-') as directory:
            path = Path(directory)
            deps = '\n'.join(f'{name} = {{ path = "{ROOT / "libs" / name}" }}' for name in ['gpu-abi', 'breengel-abi', 'input-abi'])
            (path / 'Cargo.toml').write_text(
                '[package]\nname="bwm-memory-host"\nversion="0.0.0"\nedition="2021"\n'
                '[workspace]\n[dependencies]\n' + deps + '\n'
                f'[[test]]\nname="memory"\npath="{ROOT / "tests/fixtures/bwm_memory_bound.rs"}"\n')
            env = dict(os.environ, CARGO_BUILD_JOBS='4')
            command = (['taskpolicy', '-b'] if sys.platform == 'darwin' else []) + ['cargo', '-Z', 'bindeps', 'test', '--manifest-path', str(path / 'Cargo.toml'), '--', '--test-threads=1']
            subprocess.run(command, cwd=path, env=env, check=True)


if __name__ == '__main__':
    unittest.main()

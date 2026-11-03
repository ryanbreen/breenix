"""Render the guest image handoff on the host with its real bitmap font and pixels."""
from pathlib import Path
import json
import os
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class ImagePanel(unittest.TestCase):
    def test_small_and_wide_fixture_panels(self):
        source = (ROOT / 'userspace/programs/src/suite_breengel/images.rs').read_text()
        panel = source[source.index('// An opaque, lossless view'):source.index('pub(crate) const INFLATE')]
        fixture = (ROOT / 'tests/image_panel_test.rs').read_text().replace('// GUEST_PANEL', panel)
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / 'src').mkdir()
            (root / 'src/lib.rs').write_text(fixture)
            (root / 'Cargo.toml').write_text('[package]\nname="image-panel-check"\nversion="0.1.0"\nedition="2021"\n[workspace]\n[dependencies]\n' +
                '\n'.join(f'{name}={{path={json.dumps(str(ROOT / "libs" / name))}}}' for name in ['libgfx-ref', 'libimg']))
            env = dict(os.environ, CARGO_BUILD_JOBS='4', RUSTFLAGS='-Dwarnings',
                       BREENIX_PANEL_ROOT=str(ROOT),
                       CARGO_TARGET_DIR=str(ROOT / 'target/image-panel-host'))
            priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
            subprocess.run(priority + ['cargo', 'test', '--manifest-path', str(root / 'Cargo.toml'), '--lib'],
                           env=env, check=True)


if __name__ == '__main__':
    unittest.main()

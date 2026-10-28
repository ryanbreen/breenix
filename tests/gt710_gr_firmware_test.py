#!/usr/bin/env python3
"""Check committed 7.1 assets and the kernel's build-time verifier using Python only."""
import hashlib
import json
from pathlib import Path
import re
import runpy
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
NAMES = ('fecs.code.bin', 'fecs.data.bin', 'gpccs.code.bin', 'gpccs.data.bin')
TRIANGLE = tuple(f'diagnostic-triangle-{stage}.{extension}'
                 for stage in ('vertex', 'fragment')
                 for extension in ('bin', 'sph', 'json', 'asm'))
CONTROLS = tuple(f'{name}.{extension}'
                 for name in ('constant-colour-fragment', 'drop-colour-vertex')
                 for extension in ('bin', 'sph', 'json', 'asm'))
GRADIENT = tuple(f'gradient-fragment.{extension}' for extension in ('bin', 'sph', 'json', 'asm'))
RECIPE = 'cargo run -p xtask -- kepler-assets'


class Firmware(unittest.TestCase):
    def test_committed_assets_constants_and_build_verification(self):
        expected = json.loads((ROOT / 'docs/gui/provenance/kepler-toolchain.sha256.json').read_text())
        source = (ROOT / 'kernel/src/gpu/gk208/gr/firmware.rs').read_text()
        constants = dict(re.findall(r'pub const (\w+)_SHA256: &str = "([0-9a-f]{64})";', source))
        assets = ROOT / 'kernel/assets/kepler'
        self.assertEqual({p.name for p in assets.iterdir()}, set(NAMES + TRIANGLE + CONTROLS + GRADIENT) | {'manifest.json'})
        for name in NAMES:
            self.assertEqual(constants[name.replace('.', '_').upper()], expected[name])
            data = (assets / name).read_bytes()
            self.assertTrue(data and len(data) % 4 == 0, name)
            self.assertEqual(hashlib.sha256(data).hexdigest(), expected[name])
            self.assertIn(f'include_bytes!("../../../../assets/kepler/{name}")', source)
        manifest = json.loads((assets / 'manifest.json').read_text())
        for name in TRIANGLE + CONTROLS + GRADIENT:
            data = (assets / name).read_bytes()
            self.assertEqual(hashlib.sha256(data).hexdigest(), expected[name], name)
            self.assertEqual(manifest['files'][name]['sha256'], expected[name], name)
            self.assertEqual(manifest['files'][name]['bytes'], len(data), name)
        self.assertEqual(hashlib.sha256((assets / 'manifest.json').read_bytes()).hexdigest(), expected['manifest.json'])
        verify = runpy.run_path(str(ROOT / 'kernel/verify_kepler_firmware.py'))['verify_assets']
        verify(ROOT)
        # Exercise exactly the verifier invoked by build.rs, independently of
        # Cargo's artifact cache, without installing any assembler prerequisites.
        build = (ROOT / 'kernel/build.rs').read_text()
        self.assertIn('verify_kepler_firmware.py', build)
        self.assertIn('status.success()', build)
        with tempfile.TemporaryDirectory() as directory:
            copy = Path(directory)
            for relative in ['kernel/assets/kepler', 'kernel/src/gpu/gk208/gr', 'docs/gui/provenance']:
                (copy / relative).mkdir(parents=True)
            shutil.copytree(assets, copy / 'kernel/assets/kepler', dirs_exist_ok=True)
            shutil.copy(ROOT / 'kernel/src/gpu/gk208/gr/firmware.rs', copy / 'kernel/src/gpu/gk208/gr')
            shutil.copy(ROOT / 'docs/gui/provenance/kepler-toolchain.sha256.json', copy / 'docs/gui/provenance')
            verify(copy)
            for name in NAMES + TRIANGLE + CONTROLS + GRADIENT + ('manifest.json',):
                path = copy / 'kernel/assets/kepler' / name
                original = path.read_bytes()
                for changed in (None, bytes([original[0] ^ 1]) + original[1:]):
                    if changed is None:
                        path.unlink()
                    else:
                        path.write_bytes(changed)
                    with self.assertRaisesRegex(ValueError, re.escape(name)) as caught:
                        verify(copy)
                    self.assertIn(RECIPE, str(caught.exception))
                    result = subprocess.run([sys.executable, str(ROOT / 'kernel/verify_kepler_firmware.py'), str(copy)], capture_output=True, text=True)
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn(name, result.stderr)
                    self.assertIn(RECIPE, result.stderr)
                    path.write_bytes(original)
        print('PASS: committed assets and SHA-256 constants match 7.1; missing/altered images and manifest rejected')


if __name__ == '__main__':
    unittest.main()

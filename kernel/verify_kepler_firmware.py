#!/usr/bin/env python3
"""Verify the committed source-built GR firmware before compiling the x86 kernel."""
import hashlib
import json
from pathlib import Path
import re
import sys

NAMES = ('fecs.code.bin', 'fecs.data.bin', 'gpccs.code.bin', 'gpccs.data.bin')
TRIANGLE = tuple(f'diagnostic-triangle-{stage}.{extension}'
                 for stage in ('vertex', 'fragment')
                 for extension in ('bin', 'sph', 'json', 'asm'))
CONTROLS = tuple(f'{name}.{extension}'
                 for name in ('constant-colour-fragment', 'drop-colour-vertex')
                 for extension in ('bin', 'sph', 'json', 'asm'))
REBUILD = 'regenerate with cargo run -p xtask -- kepler-assets --check and commit kernel/assets/kepler/'


def verify_assets(root):
    root = Path(root)
    assets = root / 'kernel/assets/kepler'
    expected = json.loads((root / 'docs/gui/provenance/kepler-toolchain.sha256.json').read_text())
    source = (root / 'kernel/src/gpu/gk208/gr/firmware.rs').read_text()
    constants = dict(re.findall(r'pub const (\w+)_SHA256: &str = "([0-9a-f]{64})";', source))
    for name in NAMES + TRIANGLE + CONTROLS + ('manifest.json',):
        try:
            data = (assets / name).read_bytes()
        except FileNotFoundError as error:
            raise ValueError(f'missing Kepler asset {name}; {REBUILD}') from error
        if hashlib.sha256(data).hexdigest() != expected[name]:
            raise ValueError(f'altered Kepler asset {name}: SHA-256 mismatch; {REBUILD}')
        if name in NAMES:
            if not data or len(data) % 4:
                raise ValueError(f'{name}: expected nonempty little-endian u32 image; {REBUILD}')
            if constants.get(name.replace('.', '_').upper()) != expected[name]:
                raise ValueError(f'{name}: firmware.rs SHA-256 constant mismatch; {REBUILD}')
    # The generated manifest describes the complete 7.1 package.
    manifest = json.loads((assets / 'manifest.json').read_text())
    if (manifest['schema'], manifest['chipset'], manifest['byte_order']) != (1, 0x106, 'little'):
        raise ValueError(f'manifest.json: wrong firmware format; {REBUILD}')
    for name in NAMES + TRIANGLE + CONTROLS:
        entry = manifest['files'][name]
        license_name = 'MIT' if name in NAMES or name.endswith('.sph') else 'PolyForm-Noncommercial-1.0.0'
        if entry != dict(bytes=(assets / name).stat().st_size, license=license_name, sha256=expected[name]):
            raise ValueError(f'manifest.json: inconsistent {name}; {REBUILD}')


if __name__ == '__main__':
    try:
        verify_assets(Path(sys.argv[1]) if len(sys.argv) == 2 else Path(__file__).resolve().parents[1])
    except (OSError, ValueError, KeyError) as error:
        sys.exit(str(error))

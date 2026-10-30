#!/usr/bin/env python3
"""Check the native catalogue assets and their embedded code layout."""
import hashlib
import json
from pathlib import Path
import re
import unittest

from gt710_sph_test import Fields, check

ROOT = Path(__file__).resolve().parents[1]
ASSETS = ROOT / 'kernel/assets/kepler'
PIPELINES = {'pipeline-over': 5, 'pipeline-over_straight': 6,
             'pipeline-coverage': 7, 'pipeline-bitmap_text': 9,
             'pipeline-coverage_spans': 8, 'pipeline-line': 10,
             'pipeline-circle_fill': 11, 'pipeline-circle_outline': 12,
             'pipeline-rounded_rect': 13, 'pipeline-gradient_v': 14,
             'pipeline-triangle': 15}
ROUTINES = tuple(range(25))
PROGRAMS = list(PIPELINES) + [f'probe-int-{n:02}' for n in ROUTINES]


class CatalogueAssets(unittest.TestCase):
    def test_assets_and_identity(self):
        manifest = json.loads((ASSETS / 'manifest.json').read_text())
        hashes = json.loads((ROOT / 'docs/gui/provenance/kepler-toolchain.sha256.json').read_text())
        entries = {entry['asset']: entry for entry in manifest['shaders']}
        for name in PROGRAMS:
            with self.subTest(program=name):
                entry = entries[name]
                self.assertEqual(entry['pipeline'], PIPELINES.get(name, 16))
                if name.startswith('probe-int-'):
                    self.assertEqual(entry['routine'], int(name[-2:]))
                    self.assertEqual(entry['name'], 'probe_int')
                else:
                    self.assertEqual(entry['name'], name.removeprefix('pipeline-'))
                    self.assertNotIn('routine', entry)
                for ext in ('bin', 'sph', 'json', 'asm'):
                    filename = f'{name}.{ext}'
                    data = (ASSETS / filename).read_bytes()
                    digest = hashlib.sha256(data).hexdigest()
                    self.assertEqual(digest, hashes[filename])
                    self.assertEqual(digest, manifest['files'][filename]['sha256'])
                    self.assertEqual(len(data), manifest['files'][filename]['bytes'])
                self.assertEqual(entry['metadata'], json.loads((ASSETS / f'{name}.json').read_text()))
                metadata = entry['metadata']
                self.assertEqual(metadata['chipset'], 0x106)
                self.assertEqual(metadata['stage'], 'fragment')
                self.assertGreaterEqual(metadata['gprs'], 4)
                self.assertLessEqual(metadata['gprs'], 255)
                assembly = (ASSETS / f'{name}.asm').read_text()
                registers = {int(n) for n in re.findall(r'\$r([0-9]+)\b', assembly)} - {255}
                self.assertLess(max(registers), metadata['gprs'])
                sph = (ASSETS / f'{name}.sph').read_bytes()
                check(Fields(), sph, '2', (
                    ('SPH_TYPE', 0, 2), ('VERSION', 0, 3), ('SHADER_TYPE', 0, 5),
                    ('MRT_ENABLE', 0, 1),
                    ('KILLS_PIXELS', 0, int(name in {
                        'pipeline-over', 'pipeline-over_straight', 'pipeline-coverage',
                        'pipeline-bitmap_text', 'pipeline-coverage_spans', 'pipeline-line', 'pipeline-circle_fill',
                        'pipeline-circle_outline', 'pipeline-rounded_rect', 'pipeline-triangle'})),
                    ('IMAP_POSITION_X', 0, 1), ('IMAP_POSITION_Y', 0, 1),
                    ('OMAP_RED', 0, 1), ('OMAP_GREEN', 0, 1),
                    ('OMAP_BLUE', 0, 1), ('OMAP_ALPHA', 0, 1),
                ))
                self.assertEqual(entry['metadata']['relocations'], 0)
                self.assertEqual(entry['metadata']['builtin_calls'], 0)
                self.assertEqual(entry['source_sha256'], hashlib.sha256(
                    (ROOT / entry['source']).read_bytes()).hexdigest())
                self.assertEqual(entry['metadata']['code_bytes'], (ASSETS / f'{name}.bin').stat().st_size)

    def test_embedded_layout(self):
        source = (ROOT / 'kernel/src/gpu/gk208/gr/shaders.rs').read_text()
        spans = []
        for name in PROGRAMS:
            symbol = name.upper().replace('-', '_')
            with self.subTest(program=name):
                match = re.search(rf'pub const {symbol}_OFFSET: usize = (0x[0-9a-f]+);', source)
                self.assertIsNotNone(match, name)
                offset = int(match.group(1), 16)
                self.assertGreater(offset, 0x2430)
                self.assertEqual((offset + 80) % 128, 0)
                for ext in ('sph', 'bin'):
                    self.assertIn(f'include_bytes!("../../../../assets/kepler/{name}.{ext}")', source)
                self.assertIn(f'include_str!("../../../../assets/kepler/{name}.json")', source)
                self.assertEqual((ASSETS / f'{name}.sph').stat().st_size, 80)
                spans.append((offset, offset + 80 + (ASSETS / f'{name}.bin').stat().st_size, name))
        spans.sort()
        old_end = 0x2430 + 80 + (ASSETS / 'probe-texel-fetch.bin').stat().st_size
        self.assertGreaterEqual(spans[0][0], old_end)
        for previous, current in zip(spans, spans[1:]):
            self.assertLessEqual(previous[1], current[0], (previous, current))


if __name__ == '__main__':
    unittest.main()

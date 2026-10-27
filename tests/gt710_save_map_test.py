"""Offline replay of retained source-built FECS saves, without boot acceptance.

The first capture has both poisons. The second aborted after its 0x55 save;
do not synthesize a missing 0xAA save or claim it proved save repeatability.
"""
import ast
import hashlib
import json
from pathlib import Path
import unittest
import zlib

ROOT = Path(__file__).resolve().parents[1]
FIXTURES = ROOT / 'tests/fixtures/gt710_save_map'
CAPTURES = json.loads((FIXTURES / 'captures.json').read_text())


def scorer_written(record):
    # Replay the production scorer's map statements directly: the two retained
    # failed captures lack a complete GT710_GR record for graphics_engine.
    tree = ast.parse((ROOT / 'scripts/metal/gt710-suite').read_text())
    engine = next(n for n in tree.body if isinstance(n, ast.FunctionDef)
                  and n.name == 'graphics_engine')
    body = next(n for n in engine.body if isinstance(n, ast.Try)).body
    start = next(i for i, n in enumerate(body) if isinstance(n, ast.Assign)
                 and ast.unparse(n.targets[0]) == 'layout')
    end = next(i for i in range(start, len(body)) if isinstance(body[i], ast.For)
               and ast.unparse(body[i].target) == 'save')
    def need(ok, why):
        if not ok:
            raise ValueError(why)
    scope = dict(e=record, size=record['fecs_size'], gpcs=1, need=need)
    exec(compile(ast.Module(body=body[start:end], type_ignores=[]),
                 'gt710-suite:save-map', 'exec'), scope)
    return scope['written']


def unexplained(images, written):
    failures = set()
    for poison, image in images.items():
        failures.update(i for i, (v, used) in enumerate(zip(image, written))
                        if not used and v != poison)
    if set(images) == {85, 170}:
        failures.update(i for i, (a, b, used) in
                        enumerate(zip(images[85], images[170], written))
                        if used and a != b)
    return sorted(failures)


class RetainedSaveMap(unittest.TestCase):
    def test_retained_saves_have_zero_unexplained_bytes(self):
        written = scorer_written(CAPTURES['record'])
        for boot in CAPTURES['boots']:
            with self.subTest(boot=boot['boot']):
                images = {}
                for item in boot['images']:
                    data = zlib.decompress((FIXTURES / item['file']).read_bytes())
                    self.assertEqual(len(data), len(written))
                    self.assertEqual(hashlib.sha256(data).hexdigest(), item['sha256'])
                    images[item['poison']] = data
                bad = unexplained(images, written)
                self.assertEqual(len(bad), 0,
                                 f"{boot['boot']}: {len(bad)} unexplained bytes; first={bad[:16]}")

    def test_written_byte_mutation_is_rejected_independently_of_fixture_hashes(self):
        written = scorer_written(CAPTURES['record'])
        boot = CAPTURES['boots'][0]
        images = {item['poison']: zlib.decompress((FIXTURES / item['file']).read_bytes())
                  for item in boot['images']}
        self.assertEqual(set(images), {85, 170})
        self.assertTrue(written[256])
        before = set(unexplained(images, written))
        self.assertNotIn(256, before)
        data = bytearray(images[170]); data[256] ^= 1
        images[170] = data
        self.assertEqual(set(unexplained(images, written)), before | {256})
        self.assertEqual(unexplained(images, written)[0], 256)

    def test_terminal_writes_and_the_first_gap_byte_remain_checked(self):
        written = scorer_written(CAPTURES['record'])
        # Literal end observations, independent of the scorer's arithmetic.
        for start, end in [(1052,1056), (5068,5072), (19460,19472),
                           (25864,25872), (33816,33824), (40964,40976),
                           (44092,44096), (48524,48528), (50492,50496),
                           (61272,61280), (67592,67600)]:
            self.assertTrue(all(written[start:end]))
            self.assertFalse(written[end])
        # Zero-sized strands allocate space but do not issue memory writes.
        for start in [41216, 44288, 44544]:
            self.assertFalse(any(written[start:start+256]))
        boot = CAPTURES['boots'][0]
        original = {item['poison']: zlib.decompress((FIXTURES / item['file']).read_bytes())
                    for item in boot['images']}
        for offset in [1052, 1055, 1056, 19460, 19471, 19472, 41216]:
            images = dict(original)
            data = bytearray(images[170]); data[offset] ^= 1
            images[170] = data
            self.assertIn(offset, unexplained(images, written))


if __name__ == '__main__':
    unittest.main()

"""Check the independent exact oracles for the second context and gradient."""
import hashlib
import json
from fractions import Fraction as F
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[1]


def edge(a, b, p):
    return (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0])


def raster(vertices):
    points = [[F(v) for v in vertex['window']] for vertex in vertices]
    area = edge(*points)
    assert area > 0
    mask, weights = bytearray(), {}
    for y in range(64):
        for x in range(64):
            p = (F(2*x + 1, 2), F(2*y + 1, 2))
            e = [edge(points[1], points[2], p), edge(points[2], points[0], p), edge(points[0], points[1], p)]
            assert all(e), (x, y)
            mask.append(int(all(v > 0 for v in e)))
            if mask[-1]:
                weights[x, y] = [v / area for v in e]
    return mask, weights


def colour(vertices, weights):
    rgba = [sum(weights[i] * F(vertices[i]['rgba'][j]) for i in range(3)) for j in range(4)]
    return rgba, [int(rgba[j] * 255) for j in (2, 1, 0, 3)]


class ContextOracles(unittest.TestCase):
    def test_v2_and_gradient(self):
        data = json.loads((ROOT / 'docs/gui/provenance/gt710-contexts.json').read_text(), parse_float=F)
        v1 = json.loads((ROOT / 'docs/gui/provenance/gt710-triangle.json').read_text(), parse_float=F)
        v2 = data['v2']
        self.assertEqual(v2['target'], v1['target'])
        self.assertEqual(v2['viewport'], v1['viewport'])
        self.assertEqual(v2['origin'], 'top-left')
        self.assertEqual(v2['pixel_centres'], 'half-integer')
        self.assertEqual(len(v2['vertices']), 3)
        for vertex in v2['vertices']:
            clip = list(map(F, vertex['clip']))
            window = list(map(F, vertex['window']))
            rgba = list(map(F, vertex['rgba']))
            self.assertEqual(len(clip), 4)
            self.assertEqual(clip[2:], [0, 1])
            self.assertTrue(all(v.denominator & (v.denominator - 1) == 0 for v in clip + rgba))
            self.assertEqual(window, [clip[i] * 32 + 32 for i in range(2)])
            self.assertTrue(all((v * 4).denominator == 1 for v in window))
            self.assertEqual(len(rgba), 4)
            self.assertEqual(rgba[3], 1)
            self.assertTrue(all(0 <= c <= 1 for c in rgba))
        mask, weights = raster(v2['vertices'])
        old_mask, old_weights = raster(v1['vertices'])
        self.assertNotEqual(mask, old_mask)
        self.assertEqual(hashlib.sha256(mask).hexdigest(), v2['coverage_sha256'])
        self.assertGreaterEqual(len(v2['samples']), 6)
        self.assertEqual(len({(s['x'], s['y']) for s in v2['samples']}), len(v2['samples']))
        channels = [set(), set(), set()]
        for sample in v2['samples']:
            pos = sample['x'], sample['y']
            self.assertIn(pos, weights)
            rgba, bgra = colour(v2['vertices'], weights[pos])
            self.assertTrue(all(0 <= c * 255 % 1 < F(1, 4) for c in rgba))
            self.assertEqual(sample['bgra'], bgra)
            old = colour(v1['vertices'], old_weights[pos])[1] if pos in old_weights else [0, 0, 0, 0]
            self.assertNotEqual(bgra, old)
            self.assertNotEqual(bgra, [191, 128, 64, 255])
            for j in range(3):
                channels[j].add(bgra[j])
        self.assertTrue(all(len(c) >= 3 for c in channels))

        gradient = data['gradient']
        self.assertEqual(gradient['target'], v1['target'])
        self.assertEqual(gradient['viewport'], v1['viewport'])
        self.assertEqual(gradient['formula_bgra'], ['4*x', '4*y', '2*(x+y)', '255'])
        self.assertEqual(gradient['vertices_clip'], [[-3, -3, 0, 1], [5, -3, 0, 1], [-3, 5, 0, 1]])
        verts = [{'window': [F(p[0])*32+32, F(p[1])*32+32]} for p in gradient['vertices_clip']]
        gradient_mask, _ = raster(verts)
        self.assertTrue(all(gradient_mask))
        image = bytes(component for y in range(64) for x in range(64)
                      for component in (4*x, 4*y, 2*(x+y), 255))
        self.assertEqual(len(image), 16384)
        self.assertEqual(len({image[i:i+4] for i in range(0, len(image), 4)}), 4096)
        self.assertEqual(hashlib.sha256(image).hexdigest(), gradient['image_sha256'])


if __name__ == '__main__':
    unittest.main()

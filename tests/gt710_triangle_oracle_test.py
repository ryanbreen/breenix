"""Independently verify the exact geometry and literal read-back oracle."""
import hashlib
import json
from fractions import Fraction as F
from pathlib import Path
import unittest

ORACLE = Path(__file__).resolve().parents[1] / 'docs/gui/provenance/gt710-triangle.json'


def edge(a, b, p):
    return (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0])


class TriangleOracle(unittest.TestCase):
    def test_geometry_and_literal_samples(self):
        data = json.loads(ORACLE.read_text(), parse_float=F)
        target = data['target']
        width, height, pitch = target['width'], target['height'], target['pitch']
        self.assertEqual(target['format'], 'B8G8R8A8')
        self.assertEqual(target['layout'], 'pitch-linear')
        self.assertTrue(0 < width <= 64 and 0 < height <= 64)
        self.assertTrue(pitch >= width * 4 and pitch % 256 == 0)
        self.assertEqual(target['background_word'] >> 24, 0)
        self.assertEqual(data['origin'], 'top-left')
        self.assertEqual(data['pixel_centres'], 'half-integer')
        scale = list(map(F, data['viewport']['scale']))
        translate = list(map(F, data['viewport']['translate']))
        vertices = data['vertices']
        self.assertEqual(len(vertices), 3)
        points, colours = [], []
        for vertex in vertices:
            clip = list(map(F, vertex['clip']))
            self.assertEqual(len(clip), 4)
            self.assertEqual(clip[3], 1)
            point = list(map(F, vertex['window']))
            self.assertEqual(len(point), 2)
            self.assertEqual(point, [clip[i] * scale[i] + translate[i] for i in range(2)])
            self.assertTrue(all((v * 4).denominator == 1 for v in point))
            points.append(point)
            colour = list(map(F, vertex['rgba']))
            self.assertEqual(len(colour), 4)
            self.assertEqual(colour[3], 1)
            self.assertTrue(all(0 <= c <= 1 and c.denominator & (c.denominator - 1) == 0 for c in colour))
            colours.append(colour)
        area = edge(points[0], points[1], points[2])
        self.assertTrue(area > 0 and area.denominator == 1 and area.numerator & (area.numerator - 1) == 0)
        mask = bytearray()
        weights = {}
        for y in range(height):
            for x in range(width):
                p = (F(2*x + 1, 2), F(2*y + 1, 2))
                e = [edge(points[1], points[2], p), edge(points[2], points[0], p), edge(points[0], points[1], p)]
                self.assertNotIn(0, e, (x, y))
                inside = all(v > 0 for v in e)
                mask.append(int(inside))
                if inside:
                    weights[x, y] = [v / area for v in e]
        self.assertEqual(hashlib.sha256(mask).hexdigest(), data['coverage_sha256'])
        samples = data['samples']
        self.assertGreaterEqual(len(samples), 6)
        self.assertEqual(len({(s['x'], s['y']) for s in samples}), len(samples))
        channels = [set(), set(), set()]
        for sample in samples:
            w = weights[sample['x'], sample['y']]
            rgba = [sum(w[i] * colours[i][j] for i in range(3)) for j in range(4)]
            self.assertTrue(all(0 <= c * 255 % 1 < F(1, 4) for c in rgba))
            expected = [int(rgba[j] * 255) for j in (2, 1, 0, 3)]
            self.assertEqual(sample['bgra'], expected)
            self.assertNotEqual(expected, [191, 128, 64, 255])
            for j in range(3):
                channels[j].add(expected[j])
        self.assertTrue(all(len(values) >= 3 for values in channels))


if __name__ == '__main__':
    unittest.main()

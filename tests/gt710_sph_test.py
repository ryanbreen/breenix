#!/usr/bin/env python3
"""Decode committed triangle SPHs from NVIDIA's pinned MIT field definitions.

The bit ranges come only from cla097sph.h, independently of the compiler and
its JSON metadata. Expected values below are literals for the diagnostic pair.
"""
import ast
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[1]
HEADER = ROOT / 'vendor/kepler/nvidia/classes/3d/cla097sph.h'
ASSETS = ROOT / 'kernel/assets/kepler'


def expression(text, index):
    """Evaluate the header's integer/index arithmetic without executing C."""
    def visit(node):
        if isinstance(node, ast.Constant) and type(node.value) is int:
            return node.value
        if isinstance(node, ast.Name) and node.id == 'i':
            return index
        if isinstance(node, ast.BinOp):
            left, right = visit(node.left), visit(node.right)
            if isinstance(node.op, ast.Add):
                return left + right
            if isinstance(node.op, ast.Mult):
                return left * right
        raise ValueError(f'unsupported header expression: {text}')
    return visit(ast.parse(text, mode='eval').body)


class Fields:
    def __init__(self):
        self.ranges = dict((name, (high, low)) for name, high, low in re.findall(
            r'^#define\s+(\w+)(?:\(i\))?\s+MW\((.+):(.+)\)\s*$',
            HEADER.read_text(), re.M))

    def bounds(self, name, index=0):
        high, low = (expression(part, index) for part in self.ranges[name])
        if not 0 <= low <= high < 640:
            raise ValueError(f'invalid SPH field: {name}[{index}]')
        return low, (1 << (high - low + 1)) - 1

    def read(self, data, name, index=0):
        low, mask = self.bounds(name, index)
        return (int.from_bytes(data, 'little') >> low) & mask

    def replace(self, data, name, index, value):
        low, mask = self.bounds(name, index)
        if value & ~mask:
            raise ValueError(f'value outside field: {name}')
        bits = (int.from_bytes(data, 'little') & ~(mask << low)) | (value << low)
        return bits.to_bytes(len(data), 'little')


def check(fields, data, prefix, expected):
    if len(data) != 80:
        raise AssertionError(f'expected 80-byte SPH, got {len(data)}')
    for field, index, literal in expected:
        name = f'NVA097_SPHV3T{prefix}_{field}'
        actual = fields.read(data, name, index)
        if actual != literal:
            raise AssertionError(f'{name}[{index}]: expected {literal}, got {actual}')


VERTEX = (
    ('SPH_TYPE', 0, 1),       # TYPE_01_VTG
    ('VERSION', 0, 3),
    ('SHADER_TYPE', 0, 1),    # VERTEX
) + tuple((f'GENERIC_IMAP_{c}', i, 1) for i in (0, 1) for c in 'XYZW') + tuple(
    (f'OMAP_POSITION_{c}', 0, 1) for c in 'XYZW'
) + tuple((f'GENERIC_OMAP_{c}', 0, 1) for c in 'XYZW')

FRAGMENT = (
    ('SPH_TYPE', 0, 2),       # TYPE_02_PS
    ('VERSION', 0, 3),
    ('SHADER_TYPE', 0, 5),    # PIXEL
    ('KILLS_PIXELS', 0, 0),
    ('DOES_LOAD_OR_STORE', 0, 0),
    ('SHADER_LOCAL_MEMORY_LOW_SIZE', 0, 0),
    ('SHADER_LOCAL_MEMORY_HIGH_SIZE', 0, 0),
    ('SHADER_LOCAL_MEMORY_CRS_SIZE', 0, 0),
) + tuple((f'GENERIC_IMAP_{c}', 0, 3) for c in 'XYZW') + tuple(
    (f'OMAP_{c}', 0, 1) for c in ('RED', 'GREEN', 'BLUE', 'ALPHA')
)


class TriangleSPH(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.fields = Fields()
        cls.vertex = (ASSETS / 'diagnostic-triangle-vertex.sph').read_bytes()
        cls.fragment = (ASSETS / 'diagnostic-triangle-fragment.sph').read_bytes()

    def test_vertex(self):
        check(self.fields, self.vertex, 1, VERTEX)

    def test_fragment(self):
        check(self.fields, self.fragment, 2, FRAGMENT)

    def reject_interpolation(self, value):
        # Establish that rejection cannot be caused by an unrelated bad field.
        check(self.fields, self.fragment, 2, FRAGMENT)
        mutated = self.fragment
        for c in 'XYZW':
            mutated = self.fields.replace(mutated, f'NVA097_SPHV3T2_GENERIC_IMAP_{c}', 0, value)
        self.assertNotEqual(mutated, self.fragment)
        with self.assertRaisesRegex(AssertionError, r'GENERIC_IMAP_X\[0\]: expected 3'):
            check(self.fields, mutated, 2, FRAGMENT)

    def test_constant_generic_zero_rejected(self):
        self.reject_interpolation(1)  # CONSTANT

    def test_unused_generic_zero_rejected(self):
        self.reject_interpolation(0)  # UNUSED


if __name__ == '__main__':
    unittest.main(verbosity=2)

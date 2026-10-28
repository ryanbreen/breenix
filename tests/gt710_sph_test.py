#!/usr/bin/env python3
"""Decode committed diagnostic SPHs from NVIDIA's pinned MIT field definitions.

The bit ranges come only from cla097sph.h, independently of the compiler and
its JSON metadata. Expected linkage values below are literals.
"""
import ast
import json
from pathlib import Path
import re
import subprocess
import sys
import tempfile
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

GRADIENT = (
    ('SPH_TYPE', 0, 2),
    ('VERSION', 0, 3),
    ('SHADER_TYPE', 0, 5),
    # G2: Mesa 24.3.4 nvc0_program.c:442-448 sets 0x4000 for
    # !separateFragData; cla097sph.h:413-415 names that bit MRT_ENABLE.
    # Colour target 0 is OMAP RGBA index 0 below (cla097sph.h:616-627)
    # and output_slots in the independently checked metadata.
    ('MRT_ENABLE', 0, 1),
    ('KILLS_PIXELS', 0, 0),
    ('DOES_LOAD_OR_STORE', 0, 0),
    ('SHADER_LOCAL_MEMORY_LOW_SIZE', 0, 0),
    ('SHADER_LOCAL_MEMORY_HIGH_SIZE', 0, 0),
    ('SHADER_LOCAL_MEMORY_CRS_SIZE', 0, 0),
) + tuple((f'IMAP_POSITION_{c}', 0, 1) for c in 'XY') + tuple(
    (f'GENERIC_IMAP_{c}', 0, 0) for c in 'XYZW'
) + tuple((f'OMAP_{c}', 0, 1) for c in ('RED', 'GREEN', 'BLUE', 'ALPHA'))


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


class GradientSPH(unittest.TestCase):
    def test_position_input_and_colour_zero(self):
        check(Fields(), (ASSETS / 'gradient-fragment.sph').read_bytes(), 2, GRADIENT)
        meta = json.loads((ASSETS / 'gradient-fragment.json').read_text())
        self.assertEqual(meta['chipset'], 0x106)
        self.assertEqual(meta['stage'], 'fragment')
        self.assertEqual(meta['input_slots'], [{'index': 0, 'address': 0x70, 'mask': 3}])
        self.assertEqual(meta['output_slots'], [{'index': 0, 'address': 0, 'mask': 15}])
        self.assertEqual(meta['builtin_calls'], 0)
        self.assertEqual(meta['relocations'], 0)
        self.assertEqual(meta['tls_bytes'], 0)
        self.assertEqual(meta['constant_buffer_bytes'], 0)
        self.assertEqual(meta['texture_mask'], 0)
        self.assertEqual(meta['code_bytes'], (ASSETS / 'gradient-fragment.bin').stat().st_size)

    def test_missing_position_component_rejected(self):
        fields = Fields()
        data = (ASSETS / 'gradient-fragment.sph').read_bytes()
        check(fields, data, 2, GRADIENT)
        for component in 'XY':
            mutated = fields.replace(data, f'NVA097_SPHV3T2_IMAP_POSITION_{component}', 0, 0)
            with self.assertRaisesRegex(AssertionError, f'IMAP_POSITION_{component}'):
                check(fields, mutated, 2, GRADIENT)

    def test_kernel_constants_match_committed_metadata(self):
        meta = json.loads((ASSETS / 'gradient-fragment.json').read_text())
        self.assertGreaterEqual(meta['gprs'], 4)
        self.assertLessEqual(meta['gprs'], 255)
        harness = f'''
#[path = "{ROOT / 'kernel/src/gpu/gk208/gr/shaders.rs'}"]
pub mod shaders;
fn main() {{
    assert_eq!(shaders::GRADIENT_FRAGMENT_GPRS, {meta['gprs']});
    assert_eq!(shaders::GRADIENT_FRAGMENT_HEADER,
        include_bytes!("{ASSETS / 'gradient-fragment.sph'}"));
    assert_eq!(shaders::GRADIENT_FRAGMENT_CODE,
        include_bytes!("{ASSETS / 'gradient-fragment.bin'}"));
}}
'''
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            (path / 'gradient.rs').write_text(harness)
            priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
            subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings',
                           str(path / 'gradient.rs'), '-o', str(path / 'gradient')], check=True)
            subprocess.run([str(path / 'gradient')], check=True)


if __name__ == '__main__':
    unittest.main(verbosity=2)

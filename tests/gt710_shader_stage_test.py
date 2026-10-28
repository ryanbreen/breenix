#!/usr/bin/env python3
"""Check the diagnostic pair's straight-line GK110 stage dataflow, Python only.

Instruction spellings: vendored MIT envytools envydis/gk110.c:555-560
(pass/mul/flat/sc), :1585-1586 (mov/interp), :1618-1619 (attribute ld/st),
:1691 (sched), :1693-1695 (calls), :1701 (exit), :1604-1606 (local memory).
Only the diagnostic slice is accepted; unknown instructions fail closed.
SPH OMAP: NVIDIA MIT cla097sph.h:616-627. Colour register assignment:
Mesa MIT nvc0_program.c:135-153, packed colour targets, four GPRs per target.
These are interface facts; no decoder/compiler implementation is copied here.
"""
import json
from pathlib import Path
import re
import unittest

from gt710_sph_test import Fields

ROOT = Path(__file__).resolve().parents[1]
ASSETS = ROOT / 'kernel/assets/kepler'
CONTROLS = ROOT / 'tests/fixtures/gt710_shader_controls'
LINE = re.compile(r'([0-9a-f]{8}): ([0-9a-f]{8}) ([0-9a-f]{8})\s+(.+)')
REG = r'\$r(\d+)'
ATTR = r'a\[(0x[0-9a-f]+)\]'


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def check(assembly, meta, sph, stage):
    """Symbolically execute allowed instructions, then inspect stores/exit GPRs."""
    require(meta['stage'] == stage and meta['chipset'] == 0x106, 'wrong stage/chipset')
    count = meta['gprs']
    require(type(count) is int and 0 < count <= 255, 'invalid GPR count')
    for field in ('tls_bytes', 'builtin_calls', 'relocations'):
        require(meta[field] == 0, f'forbidden {field}')
    fields = Fields()
    require(len(sph) == 80, 'invalid SPH size')
    prefix = '1' if stage == 'vertex' else '2'
    for size in ('LOW', 'HIGH', 'CRS'):
        require(fields.read(sph, f'NVA097_SPHV3T{prefix}_SHADER_LOCAL_MEMORY_{size}_SIZE') == 0,
                'SPH requests TLS')
    registers, stores, loads, interpolated = {}, {}, set(), set()
    ended = False
    lines = assembly.splitlines()
    require(len(lines) * 8 == meta['code_bytes'], 'assembly/code size mismatch')
    for index, line in enumerate(lines):
        match = LINE.fullmatch(line)
        require(match is not None, f'malformed disassembly: {line}')
        offset, _, _, instruction = match.groups()
        require(int(offset, 16) == index * 8, 'non-contiguous disassembly')
        require(not ended, 'instruction after exit')
        for register in re.findall(REG, instruction):
            require(int(register) < count, f'GPR r{register} outside json count {count}')
        if re.fullmatch(r'sched(?: 0x[0-9a-f]+){7}', instruction):
            continue
        if instruction == 'exit':
            ended = True
            continue
        load = re.fullmatch(rf'ld b32 {REG} {ATTR} 0x0 0x0', instruction)
        store = re.fullmatch(rf'st b32 {ATTR} {REG} 0x0 0x0', instruction)
        interp = re.fullmatch(rf'interp pass f32 {REG} {ATTR} 0x0 0x0 0x0', instruction)
        move = re.fullmatch(rf'mov b32 {REG} ({REG}|0x[0-9a-f]+)', instruction)
        if load and stage == 'vertex':
            dest, address = int(load[1]), int(load[2], 16)
            registers[dest] = ('input', address)
            loads.add(address)
        elif store and stage == 'vertex':
            address, src = int(store[1], 16), int(store[2])
            require(src in registers, 'store from undefined GPR')
            stores[address] = registers[src]
        elif interp and stage == 'fragment':
            dest, address = int(interp[1]), int(interp[2], 16)
            registers[dest] = ('interpolated', address)
            interpolated.add(address)
        elif move:
            dest, operand, src = move.groups()
            require(src is None or int(src) in registers, 'move from undefined GPR')
            registers[int(dest)] = registers[int(src)] if src is not None else ('constant', operand)
        else:
            raise AssertionError(f'forbidden instruction: {instruction}')
    require(ended, 'missing exit')
    if stage == 'vertex':
        for output, attribute in ((0x70, 0x80), (0x80, 0x90)):
            for component in range(4):
                address = output + 4 * component
                require(stores.get(address) == ('input', attribute + 4 * component),
                        f'vertex output 0x{address:x} is not its input component')
        require(loads == set(range(0x80, 0xa0, 4)), 'vertex input attributes incomplete')
        require(set(stores) == set(range(0x70, 0x90, 4)), 'unexpected vertex outputs')
    else:
        # OMAP selects packed colour targets; components within each occupy XYZW.
        targets = [tuple(fields.read(sph, f'NVA097_SPHV3T2_OMAP_{c}', target)
                         for c in ('RED', 'GREEN', 'BLUE', 'ALPHA')) for target in range(8)]
        require(targets[0] == (1, 1, 1, 1), 'SPH target 0 must export RGBA')
        base = 0
        for target, mask in enumerate(targets):
            if any(mask):
                if target == 0:
                    for component in range(4):
                        require(registers.get(base + component) == ('interpolated', 0x80 + 4 * component),
                                f'fragment target 0 component {component} is not unmodified interpolation')
                base += 4
        require(interpolated == set(range(0x80, 0x90, 4)), 'fragment varying incomplete')


def read_program(directory, name):
    return ((directory / (name + '.asm')).read_text(),
            json.loads((directory / (name + '.json')).read_text()),
            (directory / (name + '.sph')).read_bytes())


class ShaderStages(unittest.TestCase):
    def test_vertex(self):
        program = read_program(ASSETS, 'diagnostic-triangle-vertex')
        check(*program, 'vertex')
        self.assertEqual(program[1]['gprs'], 8)

    def test_fragment(self):
        program = read_program(ASSETS, 'diagnostic-triangle-fragment')
        check(*program, 'fragment')
        self.assertEqual(program[1]['gprs'], 4)

    def test_compiled_constant_fragment_rejected(self):
        program = read_program(CONTROLS, 'constant-colour-fragment')
        with self.assertRaisesRegex(AssertionError, 'fragment target 0 component'):
            check(*program, 'fragment')

    def test_compiled_dropped_vertex_output_rejected(self):
        program = read_program(CONTROLS, 'drop-colour-vertex')
        with self.assertRaisesRegex(AssertionError, 'vertex output 0x80'):
            check(*program, 'vertex')

    def test_forbidden_resources_and_dataflow_rejected(self):
        asm, meta, sph = read_program(ASSETS, 'diagnostic-triangle-fragment')
        for field, value in (('gprs', 3), ('tls_bytes', 16), ('relocations', 1), ('builtin_calls', 1)):
            with self.subTest(field=field), self.assertRaises(AssertionError):
                check(asm, dict(meta, **{field: value}), sph, 'fragment')
        for instruction in ('interp flat f32 $r0 a[0x80] 0x0 0x0 0x0',
                            'mov b32 $r0 0x3f800000', 'call 0x0',
                            'ld b32 $r0 l[0x0]',
                            'interp pass f32 $r0 a[0x84] 0x0 0x0 0x0'):
            mutated = asm.replace('interp pass f32 $r0 a[0x80] 0x0 0x0 0x0', instruction)
            with self.subTest(instruction=instruction), self.assertRaises(AssertionError):
                check(mutated, meta, sph, 'fragment')


if __name__ == '__main__':
    unittest.main(verbosity=2)

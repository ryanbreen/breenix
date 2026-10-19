"""G4 word-literal experiment through g3-egl.py's public renderer API.

Packet fields use the P/D/H layouts already recorded in gpu-core.md. Programs are
Breenix-authored from L's instruction definitions. This records renderer behavior;
the guest profile suite remains the acceptance check.
"""
from pathlib import Path
import re

def float_word(x):
    return struct.unpack('<I', struct.pack('<f', x))[0]

VERT = 'VERT\nDCL IN[0]\nDCL OUT[0], POSITION\nMOV OUT[0], IN[0]\nEND\n'

def draw(text, tokens, operands):
    draw.n += 1
    cid = 300 + draw.n
    c = Ctx(cid)
    target, vertex = cid * 2, cid * 2 + 1
    c.resource(target, 2, BGRA, (1 << 1) | (1 << 3), 1, 1, 256)
    c.resource(vertex, 0, R8, 1 << 4, 48, 1, 0)
    c.cmd(1, 1, [1, 0, 0] + [0xF << 27] * 8)
    c.cmd(1, 3, [2, 0, 0, 0, 0])
    c.cmd(1, 2, [3, 1 << 29, float_word(1), 0, 0, float_word(1), 0, 0, 0])
    c.cmd(1, 4, shader_words(4, 0, VERT, 11))
    c.cmd(1, 4, shader_words(5, 1, text, tokens))
    c.cmd(1, 5, [6, 0, 0, 0, 29])
    c.cmd(1, 8, [7, target, BGRA, 0, 0])
    for kind, handle in [(1, 1), (3, 2), (2, 3), (5, 6)]:
        c.cmd(2, kind, [handle])
    c.cmd(31, 0, [4, 0]); c.cmd(31, 0, [5, 1])
    c.cmd(5, 0, [1, 0, 7])
    half = float_word(0.5)
    c.cmd(4, 0, [0, half, half, half, half, half, half])
    c.cmd(6, 0, [8, 0, vertex])
    vertices = [float_word(x) for x in [-1, -1, 1, -1, -1, 1, 1, -1, 1, 1, -1, 1]]
    c.cmd(9, 0, [vertex, 0, 0, 0, 0, 0, 0, 0, 48, 1, 1] + vertices)
    c.cmd(12, 0, [1, 0] + operands)
    c.cmd(8, 0, [0, 6, 4, 0, 1, 0, 0, 0, 0, 0, 5, 0])
    assert c.flush() == 0
    out = C.create_string_buffer(256)
    box = Box(0, 0, 0, 1, 1, 1); iov = Iov(C.cast(out, ptr), 256)
    assert lib.virgl_renderer_transfer_read_iov(target, cid, 0, 256, 0, C.byref(box), 0, C.byref(iov), 1) == 0
    result = struct.unpack('<I', out.raw[:4])[0]
    lib.virgl_renderer_context_destroy(cid)
    return result

draw.n = 0
header = 'FRAG\nDCL OUT[0], COLOR\nDCL CONST[0]\nDCL TEMP[0..2]\n'
immediates = 'IMM[0] UINT32 {150, 4294967295, 0, 998277249}\nIMM[1] UINT32 {0, 8, 16, 24}\nIMM[2] UINT32 {255, 16, 65535, 0}\n'
pack = ''
for component, shift in [('x', 'z'), ('y', 'y'), ('z', 'x'), ('w', 'w')]:
    pack += f'USHR TEMP[1].{component}, TEMP[0].xxxx, IMM[1].{shift * 4}\n'
    pack += f'AND TEMP[1].{component}, TEMP[1].{component * 4}, IMM[2].xxxx\n'
pack += 'U2F TEMP[1], TEMP[1]\nMUL OUT[0], TEMP[1], IMM[0].wwww\nEND\n'
raw = header + immediates + 'XOR TEMP[0].x, IMM[0].xxxx, IMM[0].yyyy\n' + pack
synthesized = header + immediates.replace('4294967295', '0')
synthesized += 'UADD TEMP[2].x, CONST[0].xxxx, IMM[2].zzzz\nSHL TEMP[2].x, TEMP[2].xxxx, IMM[2].yyyy\nOR TEMP[2].x, TEMP[2].xxxx, IMM[2].zzzz\nXOR TEMP[0].x, IMM[0].xxxx, TEMP[2].xxxx\n' + pack
for label, text, tokens, expected in [('raw literal XOR', raw, 68, 0x7fc00000), ('runtime literal XOR', synthesized, 80, 0xffffff69)]:
    result = draw(text, tokens, [0, 0, 0, 0])
    print(f'G4 {label} {result:08x}', flush=True)
    assert result == expected

root = Path(__file__).resolve().parents[2] / 'kernel/src/gpu/virgl/profile'
profile = (root / 'profile.tgsi').read_text()
tokens = int(re.search(r'tokens: (\d+)', (root / 'mod.rs').read_text()).group(1))
for routine, inputs, expected in [(15, [0xcf000000], 0x80000000), (21, [0x7fffffff, 1, 0, 0, 0x7fffffff], 1)]:
    operands = inputs + [0] * (16 - len(inputs)) + [0, routine, 0, 0]
    result = draw(profile, tokens, operands)
    print(f'G4 repaired routine {routine} {result:08x}', flush=True)
    assert result == expected

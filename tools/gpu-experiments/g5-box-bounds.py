"""Box-filter source-bound experiment through the public renderer API.

Uses the P/D/H packet layouts recorded in gpu-core.md, not renderer sources.
"""
def fw(v):
    return struct.unpack('<I', struct.pack('<f', v))[0]


def sample(text, cid):
    c = Ctx(cid)
    target, vertex, texture = cid * 4, cid * 4 + 1, cid * 4 + 2
    c.resource(target, 2, BGRA, (1 << 1) | (1 << 3), 1, 1, 256)
    c.resource(vertex, 0, R8, 1 << 4, 48, 1, 0)
    backing = c.resource(texture, 2, BGRA, 1 << 3, 3, 1, 256)
    backing[0:12] = bytes([17, 34, 51, 255]) * 3
    box = Box(0, 0, 0, 3, 1, 1)
    iov = Iov(C.cast(backing, ptr), 256)
    assert lib.virgl_renderer_transfer_write_iov(texture, cid, 0, 256, 0, C.byref(box), 0, C.byref(iov), 1) == 0
    c.cmd(1, 1, [1, 0, 0] + [0xF << 27] * 8)
    c.cmd(1, 3, [2, 0, 0, 0, 0])
    c.cmd(1, 2, [3, 1 << 29, fw(1), 0, 0, fw(1), 0, 0, 0])
    vertex_text = 'VERT\nDCL IN[0]\nDCL OUT[0], POSITION\nMOV OUT[0], IN[0]\nEND\n'
    c.cmd(1, 4, shader_words(4, 0, vertex_text, 11))
    c.cmd(1, 4, shader_words(5, 1, text, 10000))
    c.cmd(1, 5, [6, 0, 0, 0, 29])
    c.cmd(1, 8, [7, target, BGRA, 0, 0])
    c.cmd(1, 7, [8, 2 | (2 << 3) | (2 << 6) | (2 << 11), 0, 0, 0, 0, 0, 0, 0])
    c.cmd(1, 6, [9, texture, BGRA, 0, 0, 0x688])
    for kind, handle in [(1, 1), (3, 2), (2, 3), (5, 6)]:
        c.cmd(2, kind, [handle])
    c.cmd(31, 0, [4, 0]); c.cmd(31, 0, [5, 1])
    c.cmd(5, 0, [1, 0, 7])
    c.cmd(4, 0, [0] + [fw(0.5)] * 6)
    c.cmd(6, 0, [8, 0, vertex])
    c.cmd(10, 0, [1, 0, 9]); c.cmd(18, 0, [1, 0, 8])
    c.cmd(7, 0, [4, fw(49 / 255), fw(39 / 255), fw(25 / 255), fw(1), 0, 0, 0])
    constants = [0, 0, 1, 1, 1, 0, 3, 1] + [0] * 16 + [3, 1, 0, 0] + [0] * 4
    c.cmd(12, 0, [1, 0] + constants)
    vertices = [fw(x) for x in [-1, -1, 1, -1, -1, 1, 1, -1, 1, 1, -1, 1]]
    c.cmd(9, 0, [vertex, 0, 0, 0, 0, 0, 0, 0, 48, 1, 1] + vertices)
    c.cmd(8, 0, [0, 6, 4, 0, 1, 0, 0, 0, 0, 0, 5, 0])
    answer = c.flush()
    out = C.create_string_buffer(bytes([0xA5]) * 256)
    read_iov = Iov(C.cast(out, ptr), 256)
    box = Box(0, 0, 0, 1, 1, 1)
    transfer = lib.virgl_renderer_transfer_read_iov(target, cid, 0, 256, 0, C.byref(box), 0, C.byref(read_iov), 1)
    word = struct.unpack('<I', out.raw[:4])[0]
    lib.virgl_renderer_context_destroy(cid)
    return answer, transfer, word


from pathlib import Path
import subprocess
root = Path(__file__).resolve().parents[2]
baseline = subprocess.check_output(['git', '-C', str(root), 'show',
    '3211c7a8:kernel/src/gpu/virgl/pipelines/4.tgsi'], text=True)
current_text = (root / 'kernel/src/gpu/virgl/pipelines/4.tgsi').read_text()
for label, text, cid in [('baseline', baseline, 601), ('bounded', current_text, 602)]:
    answer, transfer, word = sample(text, cid)
    print(f'G5 box {label} submit={answer} transfer={transfer} pixel={word:08x}', flush=True)
    assert answer == transfer == 0
    assert (word == 0xff312719) == (label == 'bounded')

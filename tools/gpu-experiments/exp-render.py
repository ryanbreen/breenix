import struct
def f32(x): return struct.unpack('<I', struct.pack('<f', x))[0]
VS = "VERT\nDCL IN[0]\nDCL OUT[0], POSITION\nMOV OUT[0], IN[0]\nEND\n"
SOLID = "FRAG\nDCL OUT[0], COLOR\nDCL CONST[0]\nMOV OUT[0], CONST[0]\nEND\n"
from pathlib import Path
COPY = Path(__file__).with_name('copy_fs.txt').read_text()
RT, SV, VB = 1 << 1, 1 << 3, 1 << 4
c = Ctx(1)
TW, TH, W, H = 5, 3, 8, 6
tex_pitch = 256
tex_mem = c.resource(10, 2, BGRA, SV, TW, TH, tex_pitch * TH)
c.resource(11, 2, BGRA, RT | SV, W, H, 256 * H)
c.resource(12, 0, R8, VB, 48 * 64, 1, 0)
# texture contents: pixel (x,y) = bytes (x*40+1, y*80+2, 7, 255-x)
for y in range(TH):
    row = b''.join(bytes([x * 40 + 1, y * 80 + 2, 7, 255 - x]) for x in range(TW))
    C.memmove(C.addressof(tex_mem) + y * tex_pitch, row, len(row))
box = Box(0, 0, 0, TW, TH, 1); iov = Iov(C.cast(tex_mem, ptr), tex_pitch * TH)
print('write', lib.virgl_renderer_transfer_write_iov(10, 1, 0, tex_pitch, 0, C.byref(box), 0, C.byref(iov), 1))
# state
c.cmd(1, 1, [1, 0, 0] + [0xF << 27] * 8)          # blend
c.cmd(1, 3, [2, 0, 0, 0, 0])                      # dsa
c.cmd(1, 2, [3, 1 << 29, f32(1), 0, 0, f32(1), 0, 0, 0])
c.cmd(1, 4, shader_words(4, 0, VS, 11)); c.cmd(1, 4, shader_words(5, 1, COPY, 39))
c.cmd(1, 5, [6, 0, 0, 0, 29])
c.cmd(1, 8, [7, 11, BGRA, 0, 0])                  # surface
c.cmd(1, 6, [8, 10, BGRA, 0, 0, 0 | 1 << 3 | 2 << 6 | 3 << 9])   # sampler view
c.cmd(1, 7, [9, (2 | 2 << 3 | 2 << 6) | 0 << 9 | 2 << 11, 0, 0, 0, 0, 0, 0, 0])  # sampler state clamp/nearest/no mip
for kind, h in ((2, 1), (2, 3), (2, 2), (2, 6)): pass
c.cmd(2, 1, [1]); c.cmd(2, 3, [2]); c.cmd(2, 2, [3]); c.cmd(2, 5, [6])
c.cmd(31, 0, [4, 0]); c.cmd(31, 0, [5, 1])
c.cmd(10, 0, [1, 0, 8])                           # set sampler views: fs, slot 0, view 8
c.cmd(18, 0, [1, 0, 9])                           # bind sampler states
c.cmd(5, 0, [1, 0, 7])
half = f32(8192)
c.cmd(4, 0, [0, half, half, f32(0.5), half, half, f32(0.5)])
c.cmd(7, 0, [4, f32(0.2), f32(0.2), f32(0.2), f32(1), 0, 0, 0])
def cc(p): return f32((p - 8192) / 8192)
def rect(x0, y0, x1, y1):
    a, b, cx, d = cc(x0), cc(y0), cc(x1), cc(y1)
    return [a, b, cx, b, a, d, cx, b, cx, d, a, d]
# draw: dst rect (2,1)-(6,4) sampling from texel (1,0): offset = (1-2, 0-1) = (-1,-1)
verts = rect(2, 1, 6, 4)
c.cmd(6, 0, [8, 0, 12])
c.cmd(9, 0, [12, 0, 0, 0, 0, 0, 0, 0, 48, 1, 1] + verts)
c.cmd(12, 0, [1, 0, (-1) & 0xffffffff, (-1) & 0xffffffff, 0, 0])
c.cmd(8, 0, [0, 6, 4, 0, 1, 0, 0, 0, 0, 0, 5, 0])
print('submit', c.flush())
out = C.create_string_buffer(256 * H)
box = Box(0, 0, 0, W, H, 1); iov = Iov(C.cast(out, ptr), 256 * H)
print('read', lib.virgl_renderer_transfer_read_iov(11, 1, 0, 256, 0, C.byref(box), 0, C.byref(iov), 1))
for y in range(H):
    print(' '.join(out.raw[y * 256 + 4 * x: y * 256 + 4 * x + 4].hex() for x in range(W)))
# R8 texture sampled into RGBA target
r8 = c.resource(20, 2, R8, SV, 64, 2, 256 * 2)
C.memmove(C.addressof(r8), bytes(range(64)), 64); C.memmove(C.addressof(r8) + 256, bytes(range(100, 164)), 64)
box = Box(0, 0, 0, 64, 2, 1); iov = Iov(C.cast(r8, ptr), 512)
print('write r8', lib.virgl_renderer_transfer_write_iov(20, 1, 0, 256, 0, C.byref(box), 0, C.byref(iov), 1))
c.resource(21, 2, RGBA, RT | SV, 4, 2, 512)
c.cmd(1, 8, [30, 21, RGBA, 0, 0]); c.cmd(5, 0, [1, 0, 30])
c.cmd(1, 6, [31, 20, R8, 0, 0, 0 | 1 << 3 | 2 << 6 | 3 << 9]); c.cmd(10, 0, [1, 0, 31])
c.cmd(9, 0, [12, 0, 0, 0, 0, 48, 0, 0, 48, 1, 1] + rect(0, 0, 4, 2))
c.cmd(12, 0, [1, 0, 60, 0, 0, 0])
c.cmd(8, 0, [6, 6, 4, 0, 1, 0, 0, 0, 0, 6, 11, 0])
# region copy within target 11: src (2,1,3x2) -> dst (0,4)
c.cmd(17, 0, [11, 0, 0, 4, 0, 11, 0, 2, 1, 0, 3, 2, 1])
print('submit2', c.flush())
for (h, w, ht) in ((21, 4, 2), (11, 8, 6)):
    out = C.create_string_buffer(256 * ht); box = Box(0, 0, 0, w, ht, 1); iov = Iov(C.cast(out, ptr), 256 * ht)
    print('read', h, lib.virgl_renderer_transfer_read_iov(h, 1, 0, 256, 0, C.byref(box), 0, C.byref(iov), 1))
    for y in range(ht): print(' '.join(out.raw[y * 256 + 4 * x: y * 256 + 4 * x + 4].hex() for x in range(w)))
# cross-context: ctx 2 samples texture 10 after ctx 1 re-uploads it
c2 = Ctx(2)
for h in (10, 11, 12): lib.virgl_renderer_ctx_attach_resource(2, h)
C.memmove(C.addressof(tex_mem), bytes([9, 9, 9, 9]) * TW, 4 * TW)
box = Box(0, 0, 0, TW, 1, 1); iov = Iov(C.cast(tex_mem, ptr), tex_pitch * TH)
print('write ctx1', lib.virgl_renderer_transfer_write_iov(10, 1, 0, tex_pitch, 0, C.byref(box), 0, C.byref(iov), 1))
c2.cmd(1, 1, [1, 0, 0] + [0xF << 27] * 8); c2.cmd(1, 3, [2, 0, 0, 0, 0]); c2.cmd(1, 2, [3, 1 << 29, f32(1), 0, 0, f32(1), 0, 0, 0])
c2.cmd(1, 4, shader_words(4, 0, VS, 11)); c2.cmd(1, 4, shader_words(5, 1, COPY, 39)); c2.cmd(1, 5, [6, 0, 0, 0, 29])
c2.cmd(1, 8, [7, 11, BGRA, 0, 0]); c2.cmd(1, 6, [8, 10, BGRA, 0, 0, 0 | 1 << 3 | 2 << 6 | 3 << 9])
c2.cmd(1, 7, [9, (2 | 2 << 3 | 2 << 6) | 2 << 11, 0, 0, 0, 0, 0, 0, 0])
c2.cmd(2, 1, [1]); c2.cmd(2, 3, [2]); c2.cmd(2, 2, [3]); c2.cmd(2, 5, [6]); c2.cmd(31, 0, [4, 0]); c2.cmd(31, 0, [5, 1])
c2.cmd(10, 0, [1, 0, 8]); c2.cmd(18, 0, [1, 0, 9]); c2.cmd(5, 0, [1, 0, 7])
c2.cmd(4, 0, [0, half, half, f32(0.5), half, half, f32(0.5)])
c2.cmd(6, 0, [8, 0, 12]); c2.cmd(9, 0, [12, 0, 0, 0, 0, 96, 0, 0, 48, 1, 1] + rect(6, 0, 8, 2))
c2.cmd(12, 0, [1, 0, (-6) & 0xffffffff, 0, 0, 0]); c2.cmd(8, 0, [12, 6, 4, 0, 1, 0, 0, 0, 0, 12, 17, 0])
print('submit ctx2', c2.flush())
out = C.create_string_buffer(256 * H); box = Box(0, 0, 0, W, H, 1); iov = Iov(C.cast(out, ptr), 256 * H)
print('read ctx2', lib.virgl_renderer_transfer_read_iov(11, 2, 0, 256, 0, C.byref(box), 0, C.byref(iov), 1))
for y in range(2): print(' '.join(out.raw[y * 256 + 4 * x: y * 256 + 4 * x + 4].hex() for x in range(W)))

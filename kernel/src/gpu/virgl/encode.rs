//! VirGL command buffers: the context's fixed state, the catalogue's programs, and the
//! translation of a validated BGL stream (`gpu_abi::stream`).
//!
//! Every command is encoded from the VirGL protocol header's layout for it (field
//! positions named in the comments). The state objects are Breenix's deterministic
//! rendering profile (`docs/gui/gpu.md` section 1.5): single sample, no blending, no
//! dithering, every colour channel written, no depth or stencil test, no culling, pixel
//! centres at half-integers. Programs are TGSI text (Gallium's TGSI language
//! documentation); each records how many tokens it translates to, counted from the
//! one-token-per-structure layout of TGSI's binary form (`p_shader_tokens.h`).

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use gpu_abi::float::{ratio_bits, scaled_int_bits};
use gpu_abi::stream::{self, cmd as bgl, TargetInfo, Words};

use super::wire::{self, ccmd, cmd0, obj, pipe};
use crate::gpu::dma::WordBuf;

/// The most words one SUBMIT_3D carries: 16 frames.
pub const CHUNK_WORDS: usize = 16 * 1024;

/// Rectangles per inline vertex write: six two-float vertices each, 48 KiB in all.
pub const RECTS_PER_WRITE: usize = 1024;
/// The vertex buffer's size in bytes.
pub const VERTEX_BUFFER_BYTES: u32 = (RECTS_PER_WRITE * 48) as u32;

/// Viewport half-size: the viewport spans [0, 16384) on both axes, so a vertex at
/// integer pixel coordinate `p` has the clip coordinate `(p - 8192) / 8192`, which
/// binary32 holds exactly, and the viewport maps back to exactly `p`.
const HALF: i64 = 8192;
const HALF_SHIFT: u32 = 13;

/// A VirGL command buffer split into SUBMIT_3D-sized chunks, never splitting a command.
pub struct Enc {
    chunks: Vec<WordBuf>,
    cur: WordBuf,
    oom: bool,
}

impl Enc {
    pub fn new() -> Enc {
        Enc { chunks: Vec::new(), cur: WordBuf::new(), oom: false }
    }

    /// Append one command: its header and `payload` (at most `CMD0_MAX_LEN` words).
    fn cmd(&mut self, command: u32, object: u32, payload: &[u32]) {
        self.cmd_with(command, object, payload, &[]);
    }

    /// Append one command whose payload is `head` then `tail`.
    fn cmd_with(&mut self, command: u32, object: u32, head: &[u32], tail: &[u32]) {
        let len = head.len() + tail.len();
        if self.cur.len() > 0 && self.cur.len() + 1 + len > CHUNK_WORDS {
            let full = core::mem::replace(&mut self.cur, WordBuf::new());
            self.chunks.push(full);
        }
        let mut ok = self.cur.push(cmd0(command, object, len as u32));
        for &w in head.iter().chain(tail.iter()) {
            ok &= self.cur.push(w);
        }
        if !ok {
            self.oom = true;
        }
    }

    /// The chunks, or `None` when a frame could not be allocated.
    pub fn finish(mut self) -> Option<Vec<WordBuf>> {
        if self.oom {
            return None;
        }
        if self.cur.len() > 0 {
            self.chunks.push(self.cur);
        }
        Some(self.chunks)
    }
}

/// A program's TGSI text and its token count.
pub struct Program {
    pub kind: u32,
    pub text: &'static [u8],
    pub tokens: u32,
}

/// `solid`'s vertex program: the position passes through.
///
/// Tokens: header and processor (2); `DCL IN[0]`, a declaration and its range (2);
/// `DCL OUT[0], POSITION`, a declaration, its range and its semantic (3); `MOV`, an
/// instruction with one destination and one source register (3); `END` (1). 11 in all.
pub const SOLID_VS: Program = Program {
    kind: pipe::SHADER_VERTEX,
    text: b"VERT\nDCL IN[0]\nDCL OUT[0], POSITION\n  0: MOV OUT[0], IN[0]\n  1: END\n\0",
    tokens: 11,
};

/// `solid`'s fragment program: every fragment takes constant 0, the draw's colour.
///
/// Tokens: header and processor (2); `DCL OUT[0], COLOR` (3); `DCL CONST[0]` (2); `MOV`
/// with one destination and one source (3); `END` (1). 11 in all.
pub const SOLID_FS: Program = Program {
    kind: pipe::SHADER_FRAGMENT,
    text: b"FRAG\nDCL OUT[0], COLOR\nDCL CONST[0]\n  0: MOV OUT[0], CONST[0]\n  1: END\n\0",
    tokens: 11,
};

/// How many catalogue programs a context compiles and warms (`solid`'s pair).
pub const CATALOGUE_PROGRAMS: u32 = 2;

/// The per-context VirGL object handles, from one allocator that never reuses a handle
/// across object types (`docs/gui/gpu.md` section 1.6, VirGL object handles).
pub struct Objects {
    next: u32,
    pub blend: u32,
    pub dsa: u32,
    pub rasterizer: u32,
    pub vs: u32,
    pub fs: u32,
    pub elements: u32,
    /// The surface object of each target resource this context has drawn into.
    pub surfaces: BTreeMap<u32, u32>,
}

impl Objects {
    pub fn new() -> Objects {
        Objects { next: 1, blend: 0, dsa: 0, rasterizer: 0, vs: 0, fs: 0, elements: 0, surfaces: BTreeMap::new() }
    }

    fn take(&mut self) -> u32 {
        let h = self.next;
        self.next += 1;
        h
    }
}

/// Create and bind the context's fixed state and programs, and its vertex buffer.
pub fn setup(enc: &mut Enc, o: &mut Objects, vertex_buffer: u32) {
    o.blend = o.take();
    // Blend (VIRGL_OBJ_BLEND_SIZE = 11): handle, S0 (no independent blend, no logic op,
    // no dither, no alpha-to-coverage), S1 (logic op unused), then one S2 per colour
    // buffer: blending off, all four channels written.
    let mut blend = [0u32; 11];
    blend[0] = o.blend;
    for s2 in blend[3..].iter_mut() {
        *s2 = wire::blend_s2_colormask(pipe::MASK_RGBA);
    }
    enc.cmd(ccmd::CREATE_OBJECT, obj::BLEND, &blend);
    o.dsa = o.take();
    // DSA (VIRGL_OBJ_DSA_SIZE = 5): handle, S0 (no depth test or write, no alpha test),
    // S1 and S2 (no stencil), alpha reference 0.
    enc.cmd(ccmd::CREATE_OBJECT, obj::DSA, &[o.dsa, 0, 0, 0, 0]);
    o.rasterizer = o.take();
    // Rasterizer (VIRGL_OBJ_RS_SIZE = 9): handle; S0 with only half-pixel centres (fill
    // both faces, cull none, no scissor, no multisample, no smoothing); point size 1.0;
    // no sprite coordinates; S3 (no stipple, no clip planes); line width 1.0; no offset.
    let one = 0x3F80_0000;
    enc.cmd(
        ccmd::CREATE_OBJECT,
        obj::RASTERIZER,
        &[o.rasterizer, wire::RS_S0_HALF_PIXEL_CENTER, one, 0, 0, one, 0, 0, 0],
    );
    o.vs = o.take();
    shader(enc, o.vs, &SOLID_VS);
    o.fs = o.take();
    shader(enc, o.fs, &SOLID_FS);
    o.elements = o.take();
    // Vertex elements (4 words per element after the handle): offset 0, no instance
    // divisor, vertex buffer 0, two 32-bit floats.
    enc.cmd(ccmd::CREATE_OBJECT, obj::VERTEX_ELEMENTS, &[o.elements, 0, 0, 0, wire::format::R32G32_FLOAT]);
    enc.cmd(ccmd::BIND_OBJECT, obj::BLEND, &[o.blend]);
    enc.cmd(ccmd::BIND_OBJECT, obj::DSA, &[o.dsa]);
    enc.cmd(ccmd::BIND_OBJECT, obj::RASTERIZER, &[o.rasterizer]);
    enc.cmd(ccmd::BIND_OBJECT, obj::VERTEX_ELEMENTS, &[o.elements]);
    // BIND_SHADER: handle, shader type.
    enc.cmd(ccmd::BIND_SHADER, 0, &[o.vs, pipe::SHADER_VERTEX]);
    enc.cmd(ccmd::BIND_SHADER, 0, &[o.fs, pipe::SHADER_FRAGMENT]);
    // SET_VERTEX_BUFFERS: stride, offset, resource, per buffer.
    enc.cmd(ccmd::SET_VERTEX_BUFFERS, 0, &[8, 0, vertex_buffer]);
}

/// CREATE_OBJECT SHADER: handle, type, offset word (its value the text's whole length in
/// bytes, no continuation), token count, no stream-output, then the NUL-terminated text.
fn shader(enc: &mut Enc, handle: u32, p: &Program) {
    let mut text: Vec<u32> = Vec::with_capacity(p.text.len().div_ceil(4));
    for chunk in p.text.chunks(4) {
        let mut b = [0u8; 4];
        b[..chunk.len()].copy_from_slice(chunk);
        text.push(u32::from_le_bytes(b));
    }
    enc.cmd_with(
        ccmd::CREATE_OBJECT,
        obj::SHADER,
        &[handle, p.kind, p.text.len() as u32 & 0x7FFF_FFFF, p.tokens, 0],
        &text,
    );
}

/// The four binary32 channel values of a BGRA-packed colour, in RGBA order.
fn rgba_floats(colour: u32) -> [u32; 4] {
    let b = colour & 0xFF;
    let g = (colour >> 8) & 0xFF;
    let r = (colour >> 16) & 0xFF;
    let a = colour >> 24;
    [ratio_bits(r, 255), ratio_bits(g, 255), ratio_bits(b, 255), ratio_bits(a, 255)]
}

/// The clip coordinate of integer pixel coordinate `p` (0..=16384).
fn clip_coord(p: u32) -> u32 {
    scaled_int_bits(p as i64 - HALF, HALF_SHIFT)
}

/// Draw into `surface`: framebuffer and viewport.
fn bind_target(enc: &mut Enc, surface: u32, flip_y: bool) {
    // SET_FRAMEBUFFER_STATE: colour buffer count, depth surface (none), colour surfaces.
    enc.cmd(ccmd::SET_FRAMEBUFFER_STATE, 0, &[1, 0, surface]);
    let half = gpu_abi::float::int_bits(HALF);
    let y_scale = if flip_y { half | (1 << 31) } else { half };
    let point_five = 0x3F00_0000;
    // SET_VIEWPORT_STATE: first slot, then scale x, y, z and translate x, y, z.
    enc.cmd(ccmd::SET_VIEWPORT_STATE, 0, &[0, half, y_scale, point_five, half, half, point_five]);
}

/// What translation needs of the device's view of a target.
pub struct TargetRes {
    pub id: u32,
    pub format: u32,
    pub info: TargetInfo,
}

/// The submitting context's resources, for translation.
pub trait Bindings {
    fn target(&self, handle: u64) -> Option<TargetRes>;
    fn buffer_rect(&self, handle: u64, offset: u64) -> [i32; 4];
}

/// Translate a validated stream into `enc`, writing every draw's vertices into its own
/// region of `vertex_buffer`, which the submission alone uses: the device has been seen
/// to let a later write to a region change what an earlier draw of the same submission
/// read from it, so no region is written twice. `flip_y` selects the viewport's vertical
/// direction so that row 0 is the target's top row on this device. Returns the device ids
/// of the targets the stream writes.
pub fn translate(
    enc: &mut Enc,
    o: &mut Objects,
    words: &(impl Words + ?Sized),
    b: &impl Bindings,
    vertex_buffer: u32,
    flip_y: bool,
) -> Vec<u32> {
    let mut written: Vec<u32> = Vec::new();
    let mut current: Option<TargetRes> = None;
    let mut at = 0usize;
    let mut verts: Vec<u32> = Vec::with_capacity(RECTS_PER_WRITE * 12);
    let mut offset = 0u32;
    let mut bound = false;
    while at < words.len() {
        let code = words.word(at);
        let len = words.word(at + 1) as usize;
        match code {
            bgl::BEGIN => {
                let handle = words.word(at + 2) as u64 | (words.word(at + 3) as u64) << 32;
                if let Some(t) = b.target(handle) {
                    let surface = match o.surfaces.get(&t.id) {
                        Some(&s) => s,
                        None => {
                            let s = o.take();
                            // CREATE_OBJECT SURFACE: handle, resource, format, level 0, layers 0.
                            enc.cmd(ccmd::CREATE_OBJECT, obj::SURFACE, &[s, t.id, t.format, 0, 0]);
                            o.surfaces.insert(t.id, s);
                            s
                        }
                    };
                    bind_target(enc, surface, flip_y);
                    if !written.contains(&t.id) {
                        written.push(t.id);
                    }
                    current = Some(t);
                }
            }
            bgl::CLEAR => {
                let f = rgba_floats(words.word(at + 2));
                // CLEAR: buffers, colour (four words), depth (a double), stencil.
                enc.cmd(ccmd::CLEAR, 0, &[pipe::CLEAR_COLOR0, f[0], f[1], f[2], f[3], 0, 0, 0]);
            }
            bgl::DRAW | bgl::DRAW_BUFFER => {
                let Some(t) = current.as_ref() else {
                    at += len;
                    continue;
                };
                let f = rgba_floats(words.word(at + 3));
                // SET_CONSTANT_BUFFER: shader type, index, then the data.
                enc.cmd(ccmd::SET_CONSTANT_BUFFER, 0, &[pipe::SHADER_FRAGMENT, 0, f[0], f[1], f[2], f[3]]);
                let count = if code == bgl::DRAW { words.word(at + 4) as usize } else { words.word(at + 7) as usize };
                let buffer = words.word(at + 4) as u64 | (words.word(at + 5) as u64) << 32;
                let source = words.word(at + 6) as u64;
                verts.clear();
                let mut rects = 0usize;
                for r in 0..count {
                    let rect = if code == bgl::DRAW {
                        let base = at + 5 + 4 * r;
                        [words.word(base) as i32, words.word(base + 1) as i32, words.word(base + 2) as i32, words.word(base + 3) as i32]
                    } else {
                        b.buffer_rect(buffer, source + 16 * r as u64)
                    };
                    let Some([x0, y0, x1, y1]) = stream::clip(rect, t.info.width, t.info.height) else {
                        continue;
                    };
                    let (cx0, cy0, cx1, cy1) = (clip_coord(x0), clip_coord(y0), clip_coord(x1), clip_coord(y1));
                    verts.extend_from_slice(&[cx0, cy0, cx1, cy0, cx0, cy1, cx1, cy0, cx1, cy1, cx0, cy1]);
                    rects += 1;
                    if rects == RECTS_PER_WRITE {
                        bind_vertices(enc, &mut bound, vertex_buffer);
                        offset = draw_vertices(enc, vertex_buffer, offset, &verts, rects);
                        verts.clear();
                        rects = 0;
                    }
                }
                if rects > 0 {
                    bind_vertices(enc, &mut bound, vertex_buffer);
                    offset = draw_vertices(enc, vertex_buffer, offset, &verts, rects);
                }
            }
            _ => {}
        }
        at += len;
    }
    written
}

/// Context creation's warm-up: the fixed state and programs, then one `solid` draw of
/// the 1x1 scratch target `scratch` (`docs/gui/gpu.md` section 1.4.1, program warm-up).
pub fn warm_up(enc: &mut Enc, o: &mut Objects, vertex_buffer: u32, scratch: u32, format: u32, flip_y: bool) {
    setup(enc, o, vertex_buffer);
    let s = o.take();
    enc.cmd(ccmd::CREATE_OBJECT, obj::SURFACE, &[s, scratch, format, 0, 0]);
    o.surfaces.insert(scratch, s);
    bind_target(enc, s, flip_y);
    let f = rgba_floats(0xFF00_0000);
    enc.cmd(ccmd::SET_CONSTANT_BUFFER, 0, &[pipe::SHADER_FRAGMENT, 0, f[0], f[1], f[2], f[3]]);
    let (c0, c1) = (clip_coord(0), clip_coord(1));
    draw_vertices(enc, vertex_buffer, 0, &[c0, c0, c1, c0, c0, c1, c1, c0, c1, c1, c0, c1], 1);
}

/// Bytes of vertex data a stream with `drawn` drawn rectangles needs.
pub fn vertex_bytes(drawn: u64) -> u64 {
    drawn * 48
}

/// Bind the submission's vertex buffer before its first draw.
fn bind_vertices(enc: &mut Enc, bound: &mut bool, vertex_buffer: u32) {
    if !*bound {
        // SET_VERTEX_BUFFERS: stride, offset, resource, per buffer.
        enc.cmd(ccmd::SET_VERTEX_BUFFERS, 0, &[8, 0, vertex_buffer]);
        *bound = true;
    }
}

/// Write `rects` rectangles' vertices into the vertex buffer at byte `offset` and draw
/// them; returns the offset after them.
fn draw_vertices(enc: &mut Enc, vertex_buffer: u32, offset: u32, verts: &[u32], rects: usize) -> u32 {
    let bytes = (verts.len() * 4) as u32;
    // RESOURCE_INLINE_WRITE: resource, level, usage, stride, layer stride, x, y, z, w, h,
    // d, then the data. A buffer's x and w are byte offsets and lengths.
    enc.cmd_with(
        ccmd::RESOURCE_INLINE_WRITE,
        0,
        &[vertex_buffer, 0, 0, 0, 0, offset, 0, 0, bytes, 1, 1],
        verts,
    );
    let count = (rects * 6) as u32;
    let start = offset / 8;
    // DRAW_VBO: start, count, mode, indexed, instance count, index bias, start instance,
    // primitive restart, restart index, min index, max index, count from stream output.
    enc.cmd(ccmd::DRAW_VBO, 0, &[start, count, pipe::PRIM_TRIANGLES, 0, 1, 0, 0, 0, 0, start, start + count - 1, 0]);
    offset + bytes
}

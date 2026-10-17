//! VirGL command buffers: the context's fixed state, the catalogue's programs, and the
//! translation of a validated BGL stream (`gpu_abi::stream`).
//!
//! Every command is encoded from the VirGL protocol header's layout for it (field
//! positions named in the comments). The state objects are Breenix's deterministic
//! rendering profile (`docs/gui/gpu.md` section 1.5): single sample, no blending, no
//! dithering, every colour channel written, no depth or stencil test, no culling, pixel
//! centres at half-integers. Programs are written from the TGSI documentation's
//! instruction, declaration and semantic definitions, in the text form the device was
//! seen to accept; each records how many tokens it translates to, counted from the
//! one-token-per-structure layout of TGSI's binary form (`p_shader_tokens.h`). Each
//! program's derivation and each grammar fact it relies on, with the experiment that
//! showed it, is in `docs/gui/provenance/gpu-core.md`.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use gpu_abi::float::{ratio_bits, scaled_int_bits};
use gpu_abi::stream::{self, cmd as bgl, TargetInfo, Words};

use super::wire::{self, ccmd, cmd0, obj, pipe};
#[path = "profile/mod.rs"]
mod profile;
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
        let Some(len) = head.len().checked_add(tail.len()).filter(|&len| len <= wire::CMD0_MAX_LEN as usize) else {
            self.oom = true;
            return;
        };
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

/// A program's TGSI text and token capacity (including optional flow-label tokens).
pub struct Program {
    pub kind: u32,
    pub text: &'static [u8],
    pub tokens: u32,
}

/// `solid`'s vertex program: input 0 moves to the POSITION output, then END.
///
/// Tokens: header and processor (2); `DCL IN[0]`, a declaration and its range (2);
/// `DCL OUT[0], POSITION`, a declaration, its range and its semantic (3); `MOV`, an
/// instruction with one destination and one source register (3); `END` (1). 11 in all.
pub const SOLID_VS: Program = Program {
    kind: pipe::SHADER_VERTEX,
    text: b"VERT\nDCL IN[0]\nDCL OUT[0], POSITION\nMOV OUT[0], IN[0]\nEND\n\0",
    tokens: 11,
};

/// `solid`'s fragment program: constant 0, the draw's colour, moves to the COLOR output,
/// then END.
///
/// Tokens: header and processor (2); `DCL OUT[0], COLOR` (3); `DCL CONST[0]` (2); `MOV`
/// with one destination and one source (3); `END` (1). 11 in all.
pub const SOLID_FS: Program = Program {
    kind: pipe::SHADER_FRAGMENT,
    text: b"FRAG\nDCL OUT[0], COLOR\nDCL CONST[0]\nMOV OUT[0], CONST[0]\nEND\n\0",
    tokens: 11,
};

/// `copy`'s vertex program: the same pass-through as `solid`'s, derived the same way; each
/// pipeline is its own pair of programs (`docs/gui/gpu.md` section 1.3). 11 tokens.
pub const COPY_VS: Program = Program {
    kind: pipe::SHADER_VERTEX,
    text: b"VERT\nDCL IN[0]\nDCL OUT[0], POSITION\nMOV OUT[0], IN[0]\nEND\n\0",
    tokens: 11,
};

/// `copy`'s fragment program: the fragment's window position, truncated to integers (its
/// pixel, since centres are at half-integers), plus the draw's integer offset in constant 0,
/// is the texel fetched without filtering from sampler view 0 and written as the colour.
///
/// Tokens: header and processor (2); `DCL IN[0], POSITION, LINEAR`, a declaration, its range,
/// its interpolation and its semantic (4); `DCL OUT[0], COLOR` (3); `DCL SAMP[0]` (2);
/// `DCL SVIEW[0], 2D, FLOAT`, a declaration, its range and its sampler-view token (3);
/// `DCL CONST[0]` (2); `DCL TEMP[0]` (2); `IMM[0] INT32 {0, 0, 0, 0}`, an immediate and four
/// values (5); `MOV` with a destination and a source (3); `F2I` (3); `UADD` with two
/// sources (4); `TXF`, an instruction, its texture token, a destination and two sources (5);
/// `END` (1). 39 tokens.
pub const COPY_FS: Program = Program {
    kind: pipe::SHADER_FRAGMENT,
    text: b"FRAG\nDCL IN[0], POSITION, LINEAR\nDCL OUT[0], COLOR\nDCL SAMP[0]\nDCL SVIEW[0], 2D, FLOAT\nDCL CONST[0]\nDCL TEMP[0]\nIMM[0] INT32 {0, 0, 0, 0}\nMOV TEMP[0], IMM[0]\nF2I TEMP[0].xy, IN[0]\nUADD TEMP[0].xy, TEMP[0], CONST[0]\nTXF OUT[0], TEMP[0], SAMP[0], 2D\nEND\n\0",
    tokens: 39,
};

/// Catalogue program pairs: solid, copy, and the integer probe.
pub const CATALOGUE_PROGRAMS: u32 = 6;

/// The per-context VirGL object handles, from one allocator that never reuses a handle
/// across object types (`docs/gui/gpu.md` section 1.6, VirGL object handles).
#[derive(Clone)]
pub struct Objects {
    next: u32,
    pub blend: u32,
    pub dsa: u32,
    pub rasterizer: u32,
    pub vs: u32,
    pub fs: u32,
    pub copy_vs: u32,
    pub copy_fs: u32,
    pub probe_vs: u32,
    pub probe_fs: u32,
    pub sampler: u32,
    pub elements: u32,
    /// The surface object of each target resource this context has drawn into.
    pub surfaces: BTreeMap<u32, u32>,
    /// The sampler view of each resource this context has sampled.
    pub views: BTreeMap<u32, u32>,
}

impl Objects {
    pub fn new() -> Objects {
        Objects {
            next: 1,
            blend: 0,
            dsa: 0,
            rasterizer: 0,
            vs: 0,
            fs: 0,
            copy_vs: 0,
            copy_fs: 0,
            probe_vs: 0,
            probe_fs: 0,
            sampler: 0,
            elements: 0,
            surfaces: BTreeMap::new(),
            views: BTreeMap::new(),
        }
    }

    /// The resources whose surfaces or views this context keeps.
    pub fn resources(&self) -> impl Iterator<Item = u32> + '_ {
        self.surfaces.keys().chain(self.views.keys()).copied()
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
    o.copy_vs = o.take();
    shader(enc, o.copy_vs, &COPY_VS);
    o.copy_fs = o.take();
    shader(enc, o.copy_fs, &COPY_FS);
    o.probe_vs = o.take();
    shader(enc, o.probe_vs, &SOLID_VS);
    o.probe_fs = o.take();
    shader(enc, o.probe_fs, &profile::FS);
    o.sampler = o.take();
    // Sampler state (VIRGL_OBJ_SAMPLER_STATE_SIZE = 9): handle, S0 (clamp to edge, nearest,
    // no mip filter, no compare), LOD bias, minimum and maximum LOD 0, border colour 0. `TXF`
    // ignores it (the TGSI documentation, TXF); the state names no filtering either.
    let s0 = wire::sampler_s0(pipe::TEX_WRAP_CLAMP_TO_EDGE, pipe::TEX_FILTER_NEAREST, pipe::TEX_MIPFILTER_NONE);
    enc.cmd(ccmd::CREATE_OBJECT, obj::SAMPLER_STATE, &[o.sampler, s0, 0, 0, 0, 0, 0, 0, 0]);
    o.elements = o.take();
    // Vertex elements (4 words per element after the handle): offset 0, no instance
    // divisor, vertex buffer 0, two 32-bit floats.
    enc.cmd(ccmd::CREATE_OBJECT, obj::VERTEX_ELEMENTS, &[o.elements, 0, 0, 0, wire::format::R32G32_FLOAT]);
    enc.cmd(ccmd::BIND_OBJECT, obj::BLEND, &[o.blend]);
    enc.cmd(ccmd::BIND_OBJECT, obj::DSA, &[o.dsa]);
    enc.cmd(ccmd::BIND_OBJECT, obj::RASTERIZER, &[o.rasterizer]);
    enc.cmd(ccmd::BIND_OBJECT, obj::VERTEX_ELEMENTS, &[o.elements]);
    // BIND_SAMPLER_STATES: shader type, start slot, then the states.
    enc.cmd(ccmd::BIND_SAMPLER_STATES, 0, &[pipe::SHADER_FRAGMENT, 0, o.sampler]);
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
    let terminated=p.text.ends_with(&[0]);
    if !terminated && p.text.len()%4==0 {text.push(0);}
    let bytes=p.text.len()+usize::from(!terminated);
    enc.cmd_with(
        ccmd::CREATE_OBJECT,
        obj::SHADER,
        &[handle, p.kind, bytes as u32 & 0x7FFF_FFFF, p.tokens, 0],
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

/// Destroy the surface and sampler-view objects of resources in `stale`, which no longer
/// exist, so a context caches them only for live resources.
pub fn forget_surfaces(enc: &mut Enc, o: &mut Objects, stale: &[u32]) {
    for id in stale {
        if let Some(surface) = o.surfaces.remove(id) {
            // DESTROY_OBJECT: the object's handle; the header names its type.
            enc.cmd(ccmd::DESTROY_OBJECT, obj::SURFACE, &[surface]);
        }
        if let Some(view) = o.views.remove(id) {
            enc.cmd(ccmd::DESTROY_OBJECT, obj::SAMPLER_VIEW, &[view]);
        }
    }
}

/// Which pipeline's programs a translation has bound.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Bound {
    None,
    Solid,
    Copy,
    Probe,
}

/// Bind `want`'s programs unless they are bound already.
fn bind_programs(enc: &mut Enc, o: &Objects, bound: &mut Bound, want: Bound) {
    if *bound == want {
        return;
    }
    let (vs, fs) = match want {
        Bound::Copy => (o.copy_vs, o.copy_fs),
        Bound::Probe => (o.probe_vs, o.probe_fs),
        _ => (o.vs, o.fs),
    };
    // BIND_SHADER: handle, shader type.
    enc.cmd(ccmd::BIND_SHADER, 0, &[vs, pipe::SHADER_VERTEX]);
    enc.cmd(ccmd::BIND_SHADER, 0, &[fs, pipe::SHADER_FRAGMENT]);
    *bound = want;
}

/// Sample `res` from fragment sampler view slot 0, creating its view on first use.
fn bind_view(enc: &mut Enc, o: &mut Objects, res: &SourceRes) {
    let view = match o.views.get(&res.id) {
        Some(&v) => v,
        None => {
            let v = o.take();
            // CREATE_OBJECT SAMPLER_VIEW: handle, resource, format, first and last layer 0,
            // first and last level 0, the identity swizzle.
            enc.cmd(ccmd::CREATE_OBJECT, obj::SAMPLER_VIEW, &[v, res.id, res.format, 0, 0, wire::SWIZZLE_IDENTITY]);
            o.views.insert(res.id, v);
            v
        }
    };
    // SET_SAMPLER_VIEWS: shader type, start slot, then the views.
    enc.cmd(ccmd::SET_SAMPLER_VIEWS, 0, &[pipe::SHADER_FRAGMENT, 0, view]);
}

/// RESOURCE_COPY_REGION of a `w` x `h` box at level 0, layer 0: destination resource,
/// level, x, y, z, then source resource, level, x, y, z, width, height and depth.
fn copy_region(enc: &mut Enc, dst: u32, dx: u32, dy: u32, src: u32, sx: u32, sy: u32, w: u32, h: u32) {
    enc.cmd(ccmd::RESOURCE_COPY_REGION, 0, &[dst, 0, dx, dy, 0, src, 0, sx, sy, 0, w, h, 1]);
}

/// What translation needs of the device's view of a target.
pub struct TargetRes {
    pub id: u32,
    pub format: u32,
    pub info: TargetInfo,
}

/// What translation needs of a resource a command reads.
#[derive(Clone, Copy)]
pub struct SourceRes {
    pub id: u32,
    /// Its VirGL format.
    pub format: u32,
    pub width: u32,
    pub height: u32,
}

/// The submitting context's resources, for translation.
pub trait Bindings {
    fn target(&self, handle: u64) -> Option<TargetRes>;
    fn source(&self, handle: u64) -> Option<SourceRes>;
    fn buffer_rect(&self, handle: u64, offset: u64) -> [i32; 4];
}

/// What a translation is given besides the stream: the submission's vertex buffer, its
/// scratch textures for staged copies (one per target format, B8G8R8A8 and R8G8B8A8; 0 when
/// the submission stages no copy in that format), and the test points reserved for it.
pub struct Extras {
    pub vertex_buffer: u32,
    pub scratch: [u32; 2],
    pub flip_y: bool,
    /// The target handle test point 22 fired for.
    pub corrupt: Option<u64>,
    /// The target handle whose next DRAW or DRAW_BUFFER test point 28 omits.
    pub suppress: Option<u64>,
    /// Test point 29: the word offset of the DRAW_SAMPLED that samples this resource
    /// instead of its own.
    pub swap: Option<(usize, SourceRes)>,
}

/// Translate a validated stream into `enc`, writing a draw's vertices into its own region
/// of the submission's vertex buffer, which the submission alone uses: the device has been
/// seen to let a later write to a region change what an earlier draw of the same
/// submission read from it, so no region is written twice. `flip_y` selects the viewport's
/// vertical direction so that row 0 is the target's top row on this device. The target
/// test point 22 fired for gets one extra draw of its pixel (0,0) after the stream's
/// commands for it; point 28 omits one draw and point 29 swaps one draw's texture, each
/// counted as fired only once the submission is queued. The programs, sampler view and
/// constants a draw uses are bound by the draw's own commands, so a draw does not depend on
/// what an earlier submission left bound. Returns the device ids of the targets the stream writes.
pub fn translate(enc: &mut Enc, o: &mut Objects, words: &(impl Words + ?Sized), b: &impl Bindings, x: Extras) -> Vec<u32> {
    let Extras { vertex_buffer, scratch, flip_y, corrupt, mut suppress, swap } = x;
    let mut written: Vec<u32> = Vec::new();
    let mut current: Option<TargetRes> = None;
    let mut current_handle = 0u64;
    let mut scissor = None;
    let mut corrupted = false;
    let mut at = 0usize;
    let mut verts: Vec<u32> = Vec::with_capacity(RECTS_PER_WRITE * 12);
    let mut offset = 0u32;
    let mut bound = false;
    let mut programs = Bound::None;
    while at < words.len() {
        let code = words.word(at);
        let len = words.word(at + 1) as usize;
        match code {
            bgl::BEGIN => {
                let handle = words.word(at + 2) as u64 | (words.word(at + 3) as u64) << 32;
                if !corrupted && current.is_some() && corrupt == Some(current_handle) {
                    offset = corrupt_pixel(enc, o, &mut programs, &mut bound, vertex_buffer, offset);
                    corrupted = true;
                }
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
                    current_handle = handle;
                scissor = None;
                }
            }
            bgl::CLEAR => {
                let f = rgba_floats(words.word(at + 2));
                // CLEAR: buffers, colour (four words), depth (a double), stencil.
                if scissor.is_none() {
                    enc.cmd(ccmd::CLEAR, 0, &[pipe::CLEAR_COLOR0, f[0], f[1], f[2], f[3], 0, 0, 0]);
                } else if let Some(t) = current.as_ref() {
                    if let Some(r) = stream::clipped([0,0,t.info.width as i32,t.info.height as i32], t.info.width,t.info.height,scissor) {
                        bind_programs(enc,o,&mut programs,Bound::Solid);
                        enc.cmd(ccmd::SET_CONSTANT_BUFFER,0,&[pipe::SHADER_FRAGMENT,0,f[0],f[1],f[2],f[3]]);
                        bind_vertices(enc,&mut bound,vertex_buffer);
                        offset=draw_vertices(enc,vertex_buffer,offset,&quad(r),1);
                    }
                }
            }
            bgl::DRAW | bgl::DRAW_BUFFER => {
                if current.is_some() && suppress == Some(current_handle) {
                    suppress = None;
                    at += len;
                    continue;
                }
                let Some(t) = current.as_ref() else {
                    at += len;
                    continue;
                };
                bind_programs(enc, o, &mut programs, Bound::Solid);
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
                    let Some(clipped) = stream::clipped(rect, t.info.width, t.info.height, scissor) else {
                        continue;
                    };
                    verts.extend_from_slice(&quad(clipped));
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
            bgl::COPY => {
                let source = words.word(at + 2) as u64 | (words.word(at + 3) as u64) << 32;
                let destination = words.word(at + 8) as u64 | (words.word(at + 9) as u64) << 32;
                let (Some(s), Some(d)) = (b.source(source), b.target(destination)) else {
                    at += len;
                    continue;
                };
                let r = [4, 5, 6, 7, 10, 11].map(|k| words.word(at + k));
                if let Some(c) = stream::copy_clip(r, [s.width, s.height], [d.info.width, d.info.height]) {
                    let [sx, sy, w, h, dx, dy] = c;
                    let via = scratch[usize::from(d.info.format == gpu_abi::format::R8G8B8A8)];
                    if source == destination && stream::copy_overlaps(c) && via != 0 {
                        // Through the scratch texture, so the destination receives the source
                        // as it was before the copy (`docs/gui/gpu.md` section 1.3).
                        copy_region(enc, via, 0, 0, s.id, sx, sy, w, h);
                        copy_region(enc, d.id, dx, dy, via, 0, 0, w, h);
                    } else {
                        copy_region(enc, d.id, dx, dy, s.id, sx, sy, w, h);
                    }
                    if !written.contains(&d.id) {
                        written.push(d.id);
                    }
                }
            }
            bgl::DRAW_SAMPLED => {
                let Some(t) = current.as_ref() else {
                    at += len;
                    continue;
                };
                let texture = words.word(at + 3) as u64 | (words.word(at + 4) as u64) << 32;
                let named = b.source(texture);
                let used = match swap {
                    Some((word, other)) if word == at => Some(other),
                    _ => named,
                };
                let (Some(tex), Some(own)) = (used, named) else {
                    at += len;
                    continue;
                };
                bind_programs(enc, o, &mut programs, Bound::Copy);
                bind_view(enc, o, &tex);
                let count = words.word(at + 5) as usize;
                let mut done = 0usize;
                while done < count {
                    // Up to a vertex write's worth of instances, then one draw per instance,
                    // each with its own texel offset.
                    let mut batch: Vec<(u32, u32)> = Vec::new();
                    verts.clear();
                    while done < count && batch.len() < RECTS_PER_WRITE {
                        let base = at + 6 + 6 * done;
                        let i = [0, 1, 2, 3, 4, 5].map(|k| words.word(base + k) as i32);
                        done += 1;
                        // Preserve validation's named bounds and also bound fetches in a substitute.
                        let Some(clipped) = stream::sampled_clip(i, t.info.width, t.info.height, own.width.min(tex.width), own.height.min(tex.height)).and_then(|r| stream::clipped(r.map(|v| v as i32),t.info.width,t.info.height,scissor)) else {
                            continue;
                        };
                        verts.extend_from_slice(&quad(clipped));
                        // The texel of pixel (x, y) is (x + sx - x0, y + sy - y0); the true
                        // texel is inside the texture, so 32-bit wrapping sums are exact.
                        batch.push((i[4].wrapping_sub(i[0]) as u32, i[5].wrapping_sub(i[1]) as u32));
                    }
                    if batch.is_empty() {
                        continue;
                    }
                    bind_vertices(enc, &mut bound, vertex_buffer);
                    offset = write_vertices(enc, vertex_buffer, offset, &verts);
                    let first = (offset - 48 * batch.len() as u32) / 8;
                    for (n, (ox, oy)) in batch.into_iter().enumerate() {
                        enc.cmd(ccmd::SET_CONSTANT_BUFFER, 0, &[pipe::SHADER_FRAGMENT, 0, ox, oy, 0, 0]);
                        draw_range(enc, first + 6 * n as u32, 6);
                    }
                }
            }
            bgl::SCISSOR => scissor=Some([2,3,4,5].map(|k|words.word(at+k) as i32)),
            bgl::PROBE => {
                let Some(t)=current.as_ref() else {at+=len;continue;};
                let r=[4,5,6,7].map(|k|words.word(at+k) as i32);
                if let Some(r)=stream::clipped(r,t.info.width,t.info.height,scissor) {
                    bind_programs(enc,o,&mut programs,Bound::Probe);
                    let mut constants=Vec::with_capacity(22);
                    constants.extend_from_slice(&[pipe::SHADER_FRAGMENT,0]);
                    constants.extend((8..24).map(|k|words.word(at+k)));
                    constants.extend_from_slice(&[words.word(at+3),words.word(at+2),0,0]);
                    enc.cmd(ccmd::SET_CONSTANT_BUFFER,0,&constants);
                    bind_vertices(enc,&mut bound,vertex_buffer);
                    offset=draw_vertices(enc,vertex_buffer,offset,&quad(r),1);
                }
            }
            _ => {}
        }
        at += len;
    }
    if !corrupted && current.is_some() && corrupt == Some(current_handle) {
        corrupt_pixel(enc, o, &mut programs, &mut bound, vertex_buffer, offset);
    }
    written
}

/// The two triangles of a clipped rectangle, as clip coordinates.
fn quad([x0, y0, x1, y1]: [u32; 4]) -> [u32; 12] {
    let (cx0, cy0, cx1, cy1) = (clip_coord(x0), clip_coord(y0), clip_coord(x1), clip_coord(y1));
    [cx0, cy0, cx1, cy0, cx0, cy1, cx1, cy0, cx1, cy1, cx0, cy1]
}

/// Point 22: an extra GPU draw of the current target's pixel (0,0) with the solid program.
/// No CPU write reaches device pixels. Returns the next free vertex offset.
fn corrupt_pixel(enc: &mut Enc, o: &Objects, programs: &mut Bound, bound: &mut bool, vertex_buffer: u32, offset: u32) -> u32 {
    bind_programs(enc, o, programs, Bound::Solid);
    let f = rgba_floats(!super::super::POISON);
    enc.cmd(ccmd::SET_CONSTANT_BUFFER, 0, &[pipe::SHADER_FRAGMENT, 0, f[0], f[1], f[2], f[3]]);
    bind_vertices(enc, bound, vertex_buffer);
    draw_vertices(enc, vertex_buffer, offset, &quad([0, 0, 1, 1]), 1)
}

/// Context creation's warm-up: the fixed state and programs, then one draw with each
/// pipeline into the 1x1 scratch target `scratch`, `copy` sampling the 1x1 texture
/// `scratch_texture` and the probe selecting integer addition (`docs/gui/gpu.md` section 1.4.1, program warm-up).
pub fn warm_up(enc: &mut Enc, o: &mut Objects, vertex_buffer: u32, scratch: u32, scratch_texture: u32, format: u32, flip_y: bool) {
    setup(enc, o, vertex_buffer);
    let s = o.take();
    enc.cmd(ccmd::CREATE_OBJECT, obj::SURFACE, &[s, scratch, format, 0, 0]);
    o.surfaces.insert(scratch, s);
    bind_target(enc, s, flip_y);
    let mut programs = Bound::None;
    bind_programs(enc, o, &mut programs, Bound::Solid);
    let f = rgba_floats(0xFF00_0000);
    enc.cmd(ccmd::SET_CONSTANT_BUFFER, 0, &[pipe::SHADER_FRAGMENT, 0, f[0], f[1], f[2], f[3]]);
    let next = draw_vertices(enc, vertex_buffer, 0, &quad([0, 0, 1, 1]), 1);
    bind_programs(enc, o, &mut programs, Bound::Copy);
    bind_view(enc, o, &SourceRes { id: scratch_texture, format, width: 1, height: 1 });
    enc.cmd(ccmd::SET_CONSTANT_BUFFER, 0, &[pipe::SHADER_FRAGMENT, 0, 0, 0, 0, 0]);
    let next=draw_vertices(enc, vertex_buffer, next, &quad([0, 0, 1, 1]), 1);
    bind_programs(enc,o,&mut programs,Bound::Probe);
    let mut constants=[0u32;22];constants[0]=pipe::SHADER_FRAGMENT;
    enc.cmd(ccmd::SET_CONSTANT_BUFFER,0,&constants);
    draw_vertices(enc,vertex_buffer,next,&quad([0,0,1,1]),1);
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
    let next = write_vertices(enc, vertex_buffer, offset, verts);
    draw_range(enc, offset / 8, (rects * 6) as u32);
    next
}

/// Write vertices into the vertex buffer at byte `offset`; returns the offset after them.
fn write_vertices(enc: &mut Enc, vertex_buffer: u32, offset: u32, verts: &[u32]) -> u32 {
    let bytes = (verts.len() * 4) as u32;
    // RESOURCE_INLINE_WRITE: resource, level, usage, stride, layer stride, x, y, z, w, h,
    // d, then the data. A buffer's x and w are byte offsets and lengths.
    enc.cmd_with(
        ccmd::RESOURCE_INLINE_WRITE,
        0,
        &[vertex_buffer, 0, 0, 0, 0, offset, 0, 0, bytes, 1, 1],
        verts,
    );
    offset + bytes
}

/// Draw `count` vertices from vertex `start` as triangles.
fn draw_range(enc: &mut Enc, start: u32, count: u32) {
    // DRAW_VBO: start, count, mode, indexed, instance count, index bias, start instance,
    // primitive restart, restart index, min index, max index, count from stream output.
    enc.cmd(ccmd::DRAW_VBO, 0, &[start, count, pipe::PRIM_TRIANGLES, 0, 1, 0, 0, 0, 0, start, start + count - 1, 0]);
}

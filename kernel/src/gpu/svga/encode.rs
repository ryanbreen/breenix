//! SVGA3D DX command buffers: a context's fixed state, the catalogue's programs, and the
//! translation of a validated BGL stream (`gpu_abi::stream`).
//!
//! The same deterministic rendering profile as the VirGL encoder (`docs/gui/gpu.md` section
//! 1.5): single sample, blending off with write mask 0xf, depth and stencil tests off,
//! no culling, pixel centres at half-integers. Each command's layout is the published
//! structure `svga3d_dx.h` defines for it; the programs are the catalogue's, translated to
//! SM4 tokens (`sm4`). The translation follows `virgl::encode::translate` command for
//! command, so both backends draw the same rectangles with the same programs and constants.
//!
//! A submission's vertices, constants and coverage-span tables are bytes the kernel writes
//! into frames behind MOBs of the submission's own buffer surfaces, which its first command
//! buffer defines and uploads and its last destroys; each draw reads its own region of them,
//! so no region is written twice. A draw's constants are one record of a per-instance vertex
//! stream (the device has no constant-buffer surfaces; `sm4`), which the draw reads as its
//! single instance. A first pass only counts what the second writes, so everything is
//! charged before anything is allocated (section 1.16): the command frames, and those
//! surfaces' bytes.

use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;

use gpu_abi::float::{int_bits, ratio_bits};
use gpu_abi::stream::{self, cmd as bgl, Words};

use super::{wire, Builder, What};
use crate::gpu::dma::{Pages, WordBuf, PAGE};
use crate::gpu::virgl::encode::{self as venc, Bindings, SourceRes, TargetRes};

/// Shader pairs: one per rendering pipeline, and `probe_int`'s.
pub const PAIRS: u32 = venc::RENDERING_PIPELINES + 1;
/// Shader ids a context's shader table holds: a vertex and a fragment program per pair.
pub const SHADER_IDS: u32 = 2 * PAIRS;
const PROBE_PAIR: u32 = venc::RENDERING_PIPELINES;

/// The SVGA shader types (`SVGA3dShaderType`).
const VS: u32 = 1;
const PS: u32 = 2;

/// Bytes of each draw's constant record: `sm4::CONSTANTS` vectors of four words.
const RECORD: usize = 16 * super::sm4::CONSTANTS as usize;
/// Bytes of one vertex: two binary32 coordinates.
const STRIDE: u32 = 8;
/// A command buffer is closed once it reaches this size, at a command boundary: sixteen
/// frames, as VirGL's chunks are.
const CHUNK_BYTES: usize = 64 * 1024;

/// The fragment programs' TGSI texts, in program-MOB order after the vertex program: the
/// rendering pipelines' (`solid`, `copy`, then the catalogue's), then `probe_int`'s.
pub fn fragment_texts() -> impl Iterator<Item = &'static [u8]> {
    [venc::SOLID_FS.text, venc::COPY_FS.text]
        .into_iter()
        .chain(venc::PIPELINE_PROGRAMS.iter().map(|(_, p)| p.text))
        .chain(core::iter::once(venc::PROFILE_FS.text))
}

/// A context's DX objects: the views it keeps for resources, which shader pairs it has
/// defined, and its DX context id.
#[derive(Clone)]
pub struct Objects {
    pub cid: u32,
    rtv: BTreeMap<u32, u32>,
    srv: BTreeMap<u32, u32>,
    free_rtv: Vec<u32>,
    free_srv: Vec<u32>,
    next_rtv: u32,
    next_srv: u32,
    shaders: u64,
}

impl Objects {
    pub fn new(cid: u32) -> Objects {
        Objects {
            cid,
            rtv: BTreeMap::new(),
            srv: BTreeMap::new(),
            free_rtv: Vec::new(),
            free_srv: Vec::new(),
            next_rtv: 0,
            next_srv: 0,
            shaders: 0,
        }
    }
    /// The resources whose views this context keeps.
    pub fn resources(&self) -> impl Iterator<Item = u32> + '_ {
        self.rtv.keys().chain(self.srv.keys()).copied()
    }
    /// Whether a PROBE has defined `probe_int`'s pair in this context.
    pub fn has_probe(&self) -> bool {
        self.shaders & (1 << PROBE_PAIR) != 0
    }
    fn take_rtv(&mut self) -> Option<u32> {
        take(&mut self.free_rtv, &mut self.next_rtv)
    }
    fn take_srv(&mut self) -> Option<u32> {
        take(&mut self.free_srv, &mut self.next_srv)
    }
}

fn take(free: &mut Vec<u32>, next: &mut u32) -> Option<u32> {
    if let Some(v) = free.pop() {
        return Some(v);
    }
    (*next < super::VIEWS).then(|| {
        *next += 1;
        *next - 1
    })
}

/// One command buffer of a translation, in frames. A chunk dropped before it becomes a
/// request gives back the surfaces it owns.
pub struct Chunk {
    pub words: WordBuf,
    pub effects: Vec<(u32, What)>,
    /// Transient surfaces this chunk's request owns (the last chunk of a translation).
    pub transient: Vec<u32>,
}

impl Drop for Chunk {
    fn drop(&mut self) {
        for sid in self.transient.drain(..) {
            super::forget_transient(sid);
        }
    }
}

/// A chunk of a builder's commands.
pub fn chunk(mut b: Builder) -> Option<Chunk> {
    let (words, effects) = b.take();
    let mut buf = WordBuf::new();
    for w in words {
        if !buf.push(w) {
            return None;
        }
    }
    Some(Chunk { words: buf, effects, transient: Vec::new() })
}

/// What a translation is given besides the stream: the overlapping-copy scratch sizes per
/// target format (B8G8R8A8, R8G8B8A8), the data texture's row width, and the test points
/// reserved for it.
pub struct Extras {
    pub scratch: [[u32; 2]; 2],
    pub data_width: u32,
    pub corrupt: Option<u64>,
    pub suppress: Option<u64>,
    pub swap: Option<(usize, SourceRes)>,
}

/// The bytes a translation's own surfaces hold: its vertices, its constant slots and its
/// coverage-span data texture, and the command frames it takes.
#[derive(Clone, Copy, Default, Debug)]
pub struct Sizes {
    pub vertices: usize,
    pub constants: usize,
    pub data: usize,
    pub frames: usize,
    pub transport_bytes: usize,
}

impl Sizes {
    /// The bytes of the frames the translation's pages take.
    pub fn bytes(&self) -> u64 {
        let surface = |n: usize| {
            let pages = n.div_ceil(PAGE);
            let tables = super::memory::PageTree::table_pages(pages);
            ((pages + tables) * PAGE) as u64
        };
        surface(self.vertices) + surface(self.constants) + surface(self.data)
            + (self.frames * PAGE + self.transport_bytes) as u64
    }
}

/// The frames a translation writes its surfaces' bytes into, sized by its first pass.
pub struct Backing {
    pub vertices: Option<Arc<Pages>>,
    pub constants: Option<Arc<Pages>>,
    pub data: Option<Arc<Pages>>,
}

impl Backing {
    pub fn allocate(s: &Sizes) -> Option<Backing> {
        let pages = |n: usize| -> Option<Option<Arc<Pages>>> {
            if n == 0 {
                Some(None)
            } else {
                Some(Some(Arc::new(Pages::new(n.div_ceil(PAGE))?)))
            }
        };
        Some(Backing { vertices: pages(s.vertices)?, constants: pages(s.constants)?, data: pages(s.data)? })
    }
}

/// The SVGA format of a VirGL-format source or target, as `Bindings` reports them.
fn svga_format(virgl: u32) -> u32 {
    use crate::gpu::virgl::wire::format as vf;
    match virgl {
        vf::R8G8B8A8_UNORM => wire::format::RGBA,
        vf::R8_UNORM => wire::format::R8,
        _ => wire::format::BGRA,
    }
}

/// The four binary32 channel values of a BGRA-packed colour, in RGBA order.
fn rgba_floats(colour: u32) -> [u32; 4] {
    let b = colour & 0xFF;
    let g = (colour >> 8) & 0xFF;
    let r = (colour >> 16) & 0xFF;
    let a = colour >> 24;
    [ratio_bits(r, 255), ratio_bits(g, 255), ratio_bits(b, 255), ratio_bits(a, 255)]
}

/// The binary32 nearest `num / den` for `|num| <= den`, signed.
fn signed_ratio(num: i64, den: u32) -> u32 {
    if num < 0 {
        (1 << 31) | ratio_bits(num.unsigned_abs() as u32, den)
    } else {
        ratio_bits(num as u32, den)
    }
}

/// The two triangles of a clipped rectangle of a `w` x `h` target, as clip coordinates for
/// a viewport covering the target: x `(2p - w) / w`, y `(h - 2p) / h` (Direct3D's viewport
/// puts clip y = 1 at row 0). The window position of an integer pixel coordinate is that
/// integer to within far less than the rasterizer's sub-pixel precision.
fn quad([x0, y0, x1, y1]: [u32; 4], w: u32, h: u32) -> [u32; 12] {
    let cx = |p: u32| signed_ratio(2 * p as i64 - w as i64, w);
    let cy = |p: u32| signed_ratio(h as i64 - 2 * p as i64, h);
    let (cx0, cy0, cx1, cy1) = (cx(x0), cy(y0), cx(x1), cy(y1));
    [cx0, cy0, cx1, cy0, cx0, cy1, cx1, cy0, cx1, cy1, cx0, cy1]
}

/// A translation in progress: the first pass (without `backing`) counts, the second writes.
struct Tx<'o> {
    o: &'o mut Objects,
    b: Builder,
    chunks: Vec<Chunk>,
    backing: Option<Backing>,
    layout: Option<Sizes>,
    sizes: Sizes,
    vb: u32,
    cb: u32,
    data: Option<(u32, u32)>,
    bound: Option<u32>,
    /// The current target: its size and view.
    target: Option<(u32, u32, u32)>,
    /// Views made for this translation's own surfaces, ended with it.
    own_srv: Vec<u32>,
    /// Transient sids this translation reserved.
    transient: Vec<u32>,
    /// Transients the body defines (copy scratch), destroyed in the last buffer.
    defined: Vec<u32>,
    vb_bound: bool,
    /// The views bound at the fragment stage's slots 0 to 2, when known.
    srv: Option<[u32; 3]>,
    failed: bool,
}

impl Tx<'_> {
    fn writing(&self) -> bool {
        self.backing.is_some()
    }

    /// Close the buffer once it is full; later commands go to the next buffer.
    fn boundary(&mut self) {
        if self.b.len_bytes() >= CHUNK_BYTES {
            self.close();
        }
    }

    fn close(&mut self) {
        let b = core::mem::take(&mut self.b);
        if !self.writing() {
            self.sizes.frames += b.len_bytes().div_ceil(PAGE);
            // buffer_of retains a contiguous payload copy and a separate header. Reserve
            // a fence tail for each chunk; only the last uses it. Page rounding covers
            // DmaBox's cache-line alignment without depending on the host cache line.
            self.sizes.transport_bytes += (b.len_bytes() + 24).div_ceil(PAGE) * PAGE + PAGE;
            return;
        }
        match chunk(b) {
            Some(c) => self.chunks.push(c),
            None => self.failed = true,
        }
    }

    /// Set pair `pair`'s programs, defining and binding them to the program MOB first when
    /// the context has not: a draw needs only its own pipeline's programs, whatever the
    /// catalogue's warm-up has reached (section 1.4.1, program warm-up).
    fn programs(&mut self, pair: u32) {
        if self.bound == Some(pair) {
            return;
        }
        let Some(p) = super::programs() else {
            self.failed = true;
            return;
        };
        let (vs, ps) = (2 * pair, 2 * pair + 1);
        if self.o.shaders & (1 << pair) == 0 {
            let (vs_at, vs_size) = p.at[0];
            let (ps_at, ps_size) = p.at[1 + pair as usize];
            self.b.c.dx_define_shader(vs, VS, vs_size);
            self.b.c.dx_bind_shader(self.o.cid, vs, p.mob, vs_at);
            self.b.c.dx_define_shader(ps, PS, ps_size);
            self.b.c.dx_bind_shader(self.o.cid, ps, p.mob, ps_at);
            self.o.shaders |= 1 << pair;
        }
        self.b.c.dx_set_shader(vs, VS);
        self.b.c.dx_set_shader(ps, PS);
        self.bound = Some(pair);
    }

    /// A new constant record holding `words`; returns its index.
    fn constants(&mut self, words: &[u32]) -> u32 {
        let at = self.sizes.constants;
        if let Some(p) = self.backing.as_ref().and_then(|b| b.constants.as_ref()) {
            for (i, w) in words.iter().enumerate() {
                p.write(at + 4 * i, &w.to_le_bytes());
            }
        }
        self.sizes.constants += RECORD;
        (at / RECORD) as u32
    }

    /// Draw the rectangles `rects` (already clipped) with the bound programs and the
    /// constants of record `record`.
    fn draw(&mut self, rects: &[[u32; 4]], record: u32) {
        let Some((w, h, _)) = self.target else { return };
        if rects.is_empty() {
            return;
        }
        if !self.vb_bound {
            self.b.c.dx_set_vertex_buffers(self.vb, STRIDE, self.cb, RECORD as u32);
            self.vb_bound = true;
        }
        let first = (self.sizes.vertices / STRIDE as usize) as u32;
        for &r in rects {
            let q = quad(r, w, h);
            if let Some(p) = self.backing.as_ref().and_then(|b| b.vertices.as_ref()) {
                let mut bytes = [0u8; 48];
                for (i, v) in q.iter().enumerate() {
                    bytes[4 * i..4 * i + 4].copy_from_slice(&v.to_le_bytes());
                }
                p.write(self.sizes.vertices, &bytes);
            }
            self.sizes.vertices += 48;
        }
        self.b.c.dx_draw_instanced(6 * rects.len() as u32, first, record);
        self.boundary();
    }

    /// The view through which programs sample resource `res`, made on first use.
    fn view(&mut self, res: &SourceRes) -> Option<u32> {
        if let Some(&v) = self.o.srv.get(&res.id) {
            return Some(v);
        }
        let sid = super::sid_of(res.id)?;
        let v = self.o.take_srv()?;
        self.b.c.dx_define_srv(v, sid, svga_format(res.format));
        self.o.srv.insert(res.id, v);
        Some(v)
    }

    /// A view of one of this translation's own surfaces, ended with the translation.
    fn own_view(&mut self, sid: u32, format: u32) -> Option<u32> {
        let v = self.o.take_srv()?;
        self.b.c.dx_define_srv(v, sid, format);
        self.own_srv.push(v);
        Some(v)
    }

    /// Bind `views` at slots 0 to 2 (`wire::INVALID` unbinds a slot): each draw command states
    /// its three slots, so a pipeline's draws see the same bound slots as its warm-up step.
    fn sample(&mut self, views: [u32; 3]) {
        for (slot, &v) in views.iter().enumerate() {
            if self.srv.is_none_or(|s| s[slot] != v) {
                self.b.c.dx_set_shader_resource(slot as u32, PS, v);
            }
        }
        self.srv = Some(views);
    }

    /// Draw into target `t`: its view, made on first use, and a viewport covering it. The
    /// fragment stage's views are cleared first, so no view of the new target stays bound
    /// for sampling while it is drawn into.
    fn begin(&mut self, t: &TargetRes) -> bool {
        let Some(sid) = super::sid_of(t.id) else { return false };
        let view = match self.o.rtv.get(&t.id) {
            Some(&v) => v,
            None => {
                let Some(v) = self.o.take_rtv() else {
                    self.failed = true;
                    return false;
                };
                self.b.c.dx_define_rtv(v, sid, svga_format(t.format));
                self.o.rtv.insert(t.id, v);
                v
            }
        };
        self.sample([wire::INVALID; 3]);
        self.b.c.dx_set_render_target(view);
        self.b.c.dx_set_viewport(0, 0, int_bits(t.info.width as i64), int_bits(t.info.height as i64));
        self.target = Some((t.info.width, t.info.height, view));
        true
    }

    /// The translation's own surfaces at the front of its first buffer: each defined on a
    /// MOB over its frames and uploaded.
    fn prelude(&mut self) {
        let layout = match (&self.backing, self.layout) {
            (Some(b), _) => Sizes {
                vertices: b.vertices.as_ref().map_or(0, |p| p.bytes()),
                constants: b.constants.as_ref().map_or(0, |p| p.bytes()),
                data: b.data.as_ref().map_or(0, |p| p.bytes()),
                ..Sizes::default()
            },
            (None, Some(s)) => s,
            (None, None) => return,
        };
        let mut surfaces = Vec::new();
        if layout.vertices > 0 {
            surfaces.push((self.vb, wire::bind::VERTEX, wire::format::BUFFER, layout.vertices.div_ceil(PAGE) * PAGE, 1, 0));
        }
        if layout.constants > 0 {
            surfaces.push((self.cb, wire::bind::VERTEX, wire::format::BUFFER, layout.constants.div_ceil(PAGE) * PAGE, 1, 1));
        }
        if let Some((sid, width)) = self.data {
            let bytes = layout.data.div_ceil(PAGE) * PAGE;
            surfaces.push((sid, wire::bind::SAMPLE, wire::format::RGBA, width as usize, bytes / 4 / width as usize, 2));
        }
        for (sid, flags, format, width, height, slot) in surfaces {
            if let Some(backing) = &self.backing {
                let pages = match slot { 0 => &backing.vertices, 1 => &backing.constants, _ => &backing.data };
                if pages.as_ref().and_then(|p| super::transient_pages(sid, p.clone())).is_none() {
                    self.failed = true;
                    return;
                }
                super::define_transient(&mut self.b, sid, flags, format, width as u32, height as u32);
            } else {
                // Same packets as define_transient, with placeholder addresses and ids:
                // the sizing pass must not allocate a page tree or device payload.
                self.b.c.mob_address(0, wire::PT64_1, 0, 0);
                self.b.c.surface(sid, flags, format, width as u32, height as u32);
                self.b.c.bind_surface(sid, 0);
            }
            self.b.c.update_surface(sid);
            self.defined.push(sid);
        }
    }

    /// End the translation: its own views and surfaces destroyed at the end of its last
    /// buffer, which owns the surfaces the translation reserved.
    fn finish(mut self) -> Option<Output> {
        for v in core::mem::take(&mut self.own_srv) {
            self.b.c.dx_destroy_srv(v);
            self.o.free_srv.push(v);
        }
        for sid in core::mem::take(&mut self.defined).into_iter().rev() {
            let mob = !self.writing() && self.layout.is_some()
                && (sid == self.vb || sid == self.cb || self.data.is_some_and(|(s, _)| s == sid));
            if mob { self.b.c.bind_surface(sid, wire::INVALID); }
            super::destroy_transient(&mut self.b, sid);
            if mob { self.b.c.destroy_mob(0); }
        }
        self.close();
        if self.failed {
            return self.abandon();
        }
        let transient = core::mem::take(&mut self.transient);
        if !self.writing() {
            for sid in transient {
                super::forget_transient(sid);
            }
            return Some(Output { chunks: Vec::new(), sizes: self.sizes });
        }
        let mut chunks = core::mem::take(&mut self.chunks);
        match chunks.last_mut() {
            Some(last) => last.transient = transient,
            None => {
                for sid in transient {
                    super::forget_transient(sid);
                }
            }
        }
        Some(Output { chunks, sizes: self.sizes })
    }

    /// Give back what a translation that will not be sent reserved.
    fn abandon(mut self) -> Option<Output> {
        self.chunks.clear();
        for sid in &self.transient {
            super::forget_transient(*sid);
        }
        None
    }
}

/// Destroy the views of resources in `stale`, which no longer exist, so a context keeps
/// views only of live resources.
pub fn forget_surfaces(b: &mut Builder, o: &mut Objects, stale: &[u32]) {
    for id in stale {
        if let Some(v) = o.rtv.remove(id) {
            b.c.dx_destroy_rtv(v);
            o.free_rtv.push(v);
        }
        if let Some(v) = o.srv.remove(id) {
            b.c.dx_destroy_srv(v);
            o.free_srv.push(v);
        }
    }
}

/// The fixed state the context's draws use, defined once after its creation: the input
/// layout, blending off, depth and stencil tests off, solid fill without culling.
pub fn warm_base() -> Builder {
    let mut b = Builder::new();
    b.c.dx_define_element_layout(0, super::sm4::CONSTANTS);
    b.c.dx_define_blend_state(0);
    b.c.dx_define_depth_stencil_state(0);
    b.c.dx_define_rasterizer_state(0);
    fixed_state(&mut b);
    b
}

fn fixed_state(b: &mut Builder) {
    b.c.dx_set_input_layout(0);
    b.c.dx_set_topology(wire::value::TRIANGLELIST);
    b.c.dx_set_blend_state(0);
    b.c.dx_set_depth_stencil_state(0);
    b.c.dx_set_rasterizer_state(0);
}

/// A translation's command buffers, and what its surfaces hold.
pub struct Output {
    pub chunks: Vec<Chunk>,
    pub sizes: Sizes,
}

fn start<'o>(o: &'o mut Objects, backing: Option<Backing>, layout: Option<Sizes>, data: bool) -> Option<Tx<'o>> {
    let mut transient = Vec::new();
    let mut reserve = || -> Option<u32> {
        let (sid, _) = super::transient(None)?;
        transient.push(sid);
        Some(sid)
    };
    let ids = (|| Some((reserve()?, reserve()?, if data { Some(reserve()?) } else { None })))();
    let Some((vb, cb, data_sid)) = ids else {
        for sid in transient {
            super::forget_transient(sid);
        }
        return None;
    };
    Some(Tx {
        o,
        b: Builder::new(),
        chunks: Vec::new(),
        backing,
        layout,
        sizes: Sizes::default(),
        vb,
        cb,
        data: data_sid.map(|s| (s, 0)),
        bound: None,
        target: None,
        own_srv: Vec::new(),
        transient,
        defined: Vec::new(),
        vb_bound: false,
        srv: None,
        failed: false,
    })
}

/// Translate a validated stream for context objects `o`, as `virgl::encode::translate`
/// does for VirGL, writing the surfaces' bytes into `backing`; without backing, count what a
/// translation needs. Returns the output with the device ids of the targets the stream
/// writes, and fails when ids, views or memory run out.
pub fn translate(
    o: &mut Objects,
    stale: &[u32],
    words: &(impl Words + ?Sized),
    b: &impl Bindings,
    x: Extras,
    backing: Option<Backing>,
    layout: Option<Sizes>,
) -> Option<(Output, Vec<u32>)> {
    let Extras { scratch, data_width, corrupt, mut suppress, swap } = x;
    // Pack each distinct crossing buffer once, before any draw can sample it.
    let mut tables = [(0u64, 0u32, 0u32); stream::MAX_NAMED_BUFFERS];
    let mut table_count = 0;
    let mut scan = 0;
    while scan < words.len() {
        if words.word(scan) == gpu_abi::draw::COMMAND && words.word(scan + 2) == gpu_abi::pipeline::COVERAGE_SPANS {
            let h = words.word(scan + 6) as u64 | (words.word(scan + 7) as u64) << 32;
            let bytes = words.word(scan + 8);
            let i = tables[..table_count].iter().position(|&(v, _, _)| v == h).unwrap_or_else(|| {
                let i = table_count;
                table_count += 1;
                tables[i].0 = h;
                i
            });
            tables[i].1 = tables[i].1.max(bytes);
        }
        scan += words.word(scan + 1) as usize;
    }
    let mut data_words = 0u32;
    for entry in &mut tables[..table_count] {
        entry.2 = data_words;
        data_words += entry.1 / 4;
    }
    let width = data_width.max(1);
    let mut tx = start(o, backing, layout, data_words > 0)?;
    if let Some((sid, _)) = tx.data {
        tx.data = Some((sid, width));
        tx.sizes.data = (data_words.div_ceil(width) * width * 4) as usize;
    }
    if let Some(p) = tx.backing.as_ref().and_then(|bk| bk.data.as_ref()) {
        for &(h, bytes, base) in &tables[..table_count] {
            for i in 0..bytes / 4 {
                p.write(4 * (base + i) as usize, &b.buffer_word(h, (i * 4) as u64).to_le_bytes());
            }
        }
    }
    tx.prelude();
    forget_surfaces(&mut tx.b, tx.o, stale);
    fixed_state(&mut tx.b);
    let data_view = match tx.data {
        Some((sid, _)) => match tx.own_view(sid, wire::format::RGBA) {
            Some(v) => Some(v),
            None => return tx.abandon().map(|o| (o, Vec::new())),
        },
        None => None,
    };
    // The overlapping-copy and destination-copy scratch surfaces, made on first use.
    let mut scratch_sid: [Option<(u32, u32)>; 2] = [None, None];
    let mut get_scratch = |tx: &mut Tx<'_>, format: u32| -> Option<(u32, u32)> {
        let i = usize::from(format == gpu_abi::format::R8G8B8A8);
        if let Some(s) = scratch_sid[i] {
            return Some(s);
        }
        let [w, h] = scratch[i];
        if w == 0 || h == 0 {
            return None;
        }
        let (sid, _) = super::transient(None)?;
        tx.transient.push(sid);
        tx.defined.push(sid);
        let f = if i == 1 { wire::format::RGBA } else { wire::format::BGRA };
        super::define_transient(&mut tx.b, sid, wire::bind::RENDER | wire::bind::SAMPLE, f, w, h);
        let view = tx.own_view(sid, f)?;
        scratch_sid[i] = Some((sid, view));
        Some((sid, view))
    };
    let mut written: Vec<u32> = Vec::new();
    let mut current: Option<TargetRes> = None;
    let mut current_handle = 0u64;
    let mut scissor = None;
    let mut corrupted = false;
    let mut rects: Vec<[u32; 4]> = Vec::new();
    let mut at = 0usize;
    while at < words.len() {
        let code = words.word(at);
        let len = words.word(at + 1) as usize;
        match code {
            bgl::BEGIN => {
                let handle = words.word(at + 2) as u64 | (words.word(at + 3) as u64) << 32;
                if !corrupted && current.is_some() && corrupt == Some(current_handle) {
                    corrupt_pixel(&mut tx);
                    corrupted = true;
                }
                if let Some(t) = b.target(handle) {
                    if tx.begin(&t) {
                        if !written.contains(&t.id) {
                            written.push(t.id);
                        }
                        current = Some(t);
                        current_handle = handle;
                        scissor = None;
                    }
                }
            }
            bgl::CLEAR => {
                let f = rgba_floats(words.word(at + 2));
                if let Some(t) = current.as_ref() {
                    if scissor.is_none() {
                        if let Some((_, _, view)) = tx.target {
                            tx.b.c.dx_clear_rtv(view, f);
                            tx.boundary();
                        }
                    } else if let Some(r) = stream::clipped([0, 0, t.info.width as i32, t.info.height as i32], t.info.width, t.info.height, scissor) {
                        tx.programs(gpu_abi::pipeline::SOLID);
                        tx.sample([wire::INVALID; 3]);
                        let record = tx.constants(&f);
                        tx.draw(&[r], record);
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
                let (tw, th) = (t.info.width, t.info.height);
                tx.programs(gpu_abi::pipeline::SOLID);
                tx.sample([wire::INVALID; 3]);
                let record = tx.constants(&rgba_floats(words.word(at + 3)));
                let count = if code == bgl::DRAW { words.word(at + 4) as usize } else { words.word(at + 7) as usize };
                let buffer = words.word(at + 4) as u64 | (words.word(at + 5) as u64) << 32;
                let source = words.word(at + 6) as u64;
                rects.clear();
                for r in 0..count {
                    let rect = if code == bgl::DRAW {
                        let base = at + 5 + 4 * r;
                        [words.word(base) as i32, words.word(base + 1) as i32, words.word(base + 2) as i32, words.word(base + 3) as i32]
                    } else {
                        b.buffer_rect(buffer, source + 16 * r as u64)
                    };
                    if let Some(c) = stream::clipped(rect, tw, th, scissor) {
                        rects.push(c);
                    }
                    if rects.len() == venc::RECTS_PER_WRITE {
                        tx.draw(&rects, record);
                        rects.clear();
                    }
                }
                tx.draw(&rects, record);
            }
            bgl::COPY => {
                let source = words.word(at + 2) as u64 | (words.word(at + 3) as u64) << 32;
                let destination = words.word(at + 8) as u64 | (words.word(at + 9) as u64) << 32;
                let (Some(s), Some(d)) = (b.source(source), b.target(destination)) else {
                    at += len;
                    continue;
                };
                let (Some(ss), Some(ds)) = (super::sid_of(s.id), super::sid_of(d.id)) else {
                    at += len;
                    continue;
                };
                let r = [4, 5, 6, 7, 10, 11].map(|k| words.word(at + k));
                if let Some(c) = stream::copy_clip(r, [s.width, s.height], [d.info.width, d.info.height]) {
                    let [sx, sy, w, h, dx, dy] = c;
                    let via = if source == destination && stream::copy_overlaps(c) { get_scratch(&mut tx, d.info.format) } else { None };
                    match via {
                        // Through the scratch surface, so the destination receives the source as
                        // it was before the copy (`docs/gui/gpu.md` section 1.3).
                        Some((v, _)) => {
                            tx.b.c.dx_copy_region(v, 0, 0, ss, sx, sy, w, h);
                            tx.b.c.dx_copy_region(ds, dx, dy, v, 0, 0, w, h);
                        }
                        None => tx.b.c.dx_copy_region(ds, dx, dy, ss, sx, sy, w, h),
                    }
                    if !written.contains(&d.id) {
                        written.push(d.id);
                    }
                    tx.boundary();
                }
            }
            bgl::DRAW_SAMPLED => {
                let Some(t) = current.as_ref() else {
                    at += len;
                    continue;
                };
                let (tw, th) = (t.info.width, t.info.height);
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
                tx.programs(gpu_abi::pipeline::COPY);
                let Some(view) = tx.view(&tex) else {
                    at += len;
                    continue;
                };
                tx.sample([view, wire::INVALID, wire::INVALID]);
                let count = words.word(at + 5) as usize;
                for n in 0..count {
                    let base = at + 6 + 6 * n;
                    let i = [0, 1, 2, 3, 4, 5].map(|k| words.word(base + k) as i32);
                    // Preserve validation's named bounds and also bound fetches in a substitute.
                    let Some(clipped) = stream::sampled_clip(i, tw, th, own.width.min(tex.width), own.height.min(tex.height))
                        .and_then(|r| stream::clipped(r.map(|v| v as i32), tw, th, scissor))
                    else {
                        continue;
                    };
                    // The texel of pixel (x, y) is (x + sx - x0, y + sy - y0); the true texel
                    // is inside the texture, so 32-bit wrapping sums are exact.
                    let offset = [i[4].wrapping_sub(i[0]) as u32, i[5].wrapping_sub(i[1]) as u32, 0, 0];
                    let record = tx.constants(&offset);
                    tx.draw(&[clipped], record);
                }
            }
            gpu_abi::draw::COMMAND => {
                use gpu_abi::draw as d;
                let Some(t) = current.as_ref() else {
                    at += len;
                    continue;
                };
                let (tw, th, tf) = (t.info.width, t.info.height, t.info.format);
                let target_id = t.id;
                let p = words.word(at + 2);
                let texture = words.word(at + 4) as u64 | (words.word(at + 5) as u64) << 32;
                let buffer = words.word(at + 6) as u64 | (words.word(at + 7) as u64) << 32;
                let bytes = words.word(at + 8);
                let count = words.word(at + 9) as usize;
                tx.programs(p);
                let tex = b.source(texture);
                let mut views = [wire::INVALID; 3];
                if let Some(tex) = tex {
                    if let Some(v) = tx.view(&tex) {
                        views[0] = v;
                    }
                }
                let source_info = tex.map(|s| stream::SourceInfo { width: s.width, height: s.height, format: 0, invalid: false });
                let instance = |n: usize| {
                    let base = at + d::HEADER + d::INSTANCE * n;
                    d::Instance {
                        rect: core::array::from_fn(|k| words.word(base + k) as i32),
                        params: core::array::from_fn(|k| words.word(base + 4 + k)),
                    }
                };
                if d::blended(p) {
                    let mut union: Option<[u32; 4]> = None;
                    for n in 0..count {
                        if let Some(r) = d::source_clip(p, instance(n), source_info).and_then(|i| d::clip(p, i, tw, th, scissor)) {
                            union = Some(match union {
                                None => r,
                                Some(u) => [u[0].min(r[0]), u[1].min(r[1]), u[2].max(r[2]), u[3].max(r[3])],
                            });
                        }
                    }
                    if let Some(u) = union {
                        // The destination's pixels under the draw, copied first, so the blend
                        // reads the destination as it was (section 1.3, `over`).
                        if let (Some((via, view)), Some(ts)) = (get_scratch(&mut tx, tf), super::sid_of(target_id)) {
                            tx.b.c.dx_copy_region(via, u[0], u[1], ts, u[0], u[1], u[2] - u[0], u[3] - u[1]);
                            views[1] = view;
                        }
                    }
                }
                if p == gpu_abi::pipeline::COVERAGE_SPANS && bytes != 0 {
                    if let Some(v) = data_view {
                        views[2] = v;
                    }
                }
                tx.sample(views);
                let table = tables[..table_count].iter().find(|&&(h, _, _)| h == buffer).map_or(0, |e| e.2);
                for n in 0..count {
                    let i = instance(n);
                    let Some(r) = d::source_clip(p, i, source_info).and_then(|i| d::clip(p, i, tw, th, scissor)) else { continue };
                    let mut constants = [0u32; 32];
                    constants[..16].copy_from_slice(&i.params);
                    if p >= 10 && p != gpu_abi::pipeline::GRADIENT_V {
                        constants[20..24].copy_from_slice(&rgba_floats(words.word(at + 3)));
                    } else {
                        constants[20] = words.word(at + 3);
                    }
                    constants[24..28].copy_from_slice(&[tex.map_or(0, |x| x.width), tex.map_or(0, |x| x.height), table, width.trailing_zeros()]);
                    let record = tx.constants(&constants);
                    tx.draw(&[r], record);
                }
            }
            bgl::SCISSOR => scissor = Some([2, 3, 4, 5].map(|k| words.word(at + k) as i32)),
            bgl::PROBE => {
                let Some(t) = current.as_ref() else {
                    at += len;
                    continue;
                };
                let r = [4, 5, 6, 7].map(|k| words.word(at + k) as i32);
                if let Some(r) = stream::clipped(r, t.info.width, t.info.height, scissor) {
                    // The context's first PROBE defines `probe_int`'s pair; this draw warms it.
                    // The objects are kept only if the submission is sent.
                    tx.programs(PROBE_PAIR);
                    tx.sample([wire::INVALID; 3]);
                    let mut constants = [0u32; 20];
                    for (k, c) in constants[..16].iter_mut().enumerate() {
                        *c = words.word(at + 8 + k);
                    }
                    constants[16] = words.word(at + 3);
                    constants[17] = words.word(at + 2);
                    let record = tx.constants(&constants);
                    tx.draw(&[r], record);
                }
            }
            _ => {}
        }
        at += len;
    }
    if !corrupted && current.is_some() && corrupt == Some(current_handle) {
        corrupt_pixel(&mut tx);
    }
    tx.finish().map(|o| (o, written))
}

/// Point 22: an extra GPU draw of the current target's pixel (0,0) with the solid program.
/// No CPU write reaches device pixels.
fn corrupt_pixel(tx: &mut Tx<'_>) {
    tx.programs(gpu_abi::pipeline::SOLID);
    tx.sample([wire::INVALID; 3]);
    let record = tx.constants(&rgba_floats(!crate::gpu::POISON));
    tx.draw(&[[0, 0, 1, 1]], record);
}

/// The bytes one warm-up step's surfaces hold: one quad's vertices and one constant record.
pub const WARM_SIZES: Sizes = Sizes { vertices: 48, constants: RECORD, data: 0, frames: 0, transport_bytes: 0 };

/// One step of the catalogue's asynchronous warm-up: rendering pipeline `p`'s programs,
/// defined unless a submission already did, and one draw with them into the 1x1 scratch
/// target `scratch`, sampling slots 0 to 2 reading the 1x1 texture `scratch_texture`.
pub fn warm_pipeline(o: &mut Objects, scratch: &TargetRes, scratch_texture: &SourceRes, p: u32) -> Option<Vec<Chunk>> {
    let backing = Backing::allocate(&WARM_SIZES)?;
    let mut tx = start(o, Some(backing), None, false)?;
    tx.prelude();
    fixed_state(&mut tx.b);
    if !tx.begin(scratch) {
        tx.abandon();
        return None;
    }
    tx.programs(p);
    // The slots the pipeline's draws bind: its texture, the destination copy of a blending
    // pipeline, and coverage_spans' data, each here the 1x1 texture.
    let v = tx.view(scratch_texture).unwrap_or(wire::INVALID);
    let used = |on: bool| if on { v } else { wire::INVALID };
    let sampled = gpu_abi::draw::sampled(p) || p == gpu_abi::pipeline::COPY;
    tx.sample([used(sampled), used(gpu_abi::draw::blended(p)), used(p == gpu_abi::pipeline::COVERAGE_SPANS)]);
    let record = match p {
        gpu_abi::pipeline::SOLID => tx.constants(&rgba_floats(0xFF00_0000)),
        gpu_abi::pipeline::COPY => tx.constants(&[0, 0, 0, 0]),
        _ => {
            // Valid, minimal constants exercise the program on a one-pixel quad.
            let mut constants = [0u32; 32];
            let params: &[u32] = match p {
                2 => &[0, 0, 0, 0, 1],
                3 | 4 => &[0, 0, 1, 1, 0, 0, 1, 1],
                // This off-glyph fragment reaches the coverage predicate's discard before the
                // dummy data-view fetch.
                8 => &[2, 2, 1, 1, 0],
                9 => &[0, 0, 0, 0, 1, 1, 1, 1],
                13 => &[0, 0, 1, 1, 0],
                14 => &[0, 1, 0],
                15 => &[0, 0, 1, 0, 0, 1],
                _ => &[],
            };
            constants[..params.len()].copy_from_slice(params);
            constants[24] = 1;
            constants[25] = 1;
            tx.constants(&constants)
        }
    };
    tx.draw(&[[0, 0, 1, 1]], record);
    tx.finish().map(|o| o.chunks)
}

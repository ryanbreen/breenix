/*
 * rnndb copyright 2006
 * Artur Huillet
 * Ben Skeggs
 * B. R.
 * Carlos Martin
 * Christoph Bumiller
 * Dawid Gajownik
 * Dmitry Baryshkov
 * Dmitry Eremin-Solenikov
 * EdB
 * Erik Waling
 * Francisco Jerez
 * Ilia Mirkin
 * jb17bsome
 * Jeremy Kolb
 * Laurent Carlier
 * Luca Barbieri
 * Maarten Maathuis
 * Marcelina Kościelnicka
 * Mark Carey
 * Matthieu Castet
 * nvidiaman
 * Patrice Mandin
 * Pekka Paalanen
 * Peter Popov
 * Richard Hughes
 * Rudi Cilibrasi
 * Serge Martin
 * Simon Raffeiner
 * Stephane Loeuillet
 * Stephane Marchesin
 * sturmflut
 * Sylvain Munaut
 * Victor Stinner
 * Wladmir van der Laan
 * Younes Manton
 *
 * Permission is hereby granted, free of charge, to any person obtaining
 * a copy of this software and associated documentation files (the
 * "Software"), to deal in the Software without restriction, including
 * without limitation the rights to use, copy, modify, merge, publish,
 * distribute, sublicense, and/or sell copies of the Software, and to
 * permit persons to whom the Software is furnished to do so, subject to
 * the following conditions:
 *
 * The above copyright notice and this permission notice (including the
 * next paragraph) shall be included in all copies or substantial
 * portions of the Software.
 *
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
 * EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
 * MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.
 * IN NO EVENT SHALL THE COPYRIGHT OWNER(S) AND/OR ITS SUPPLIERS BE
 * LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
 * OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION
 * WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
 */

/*******************************************************************************
    Copyright (c) 2020, NVIDIA CORPORATION. All rights reserved.

    Permission is hereby granted, free of charge, to any person obtaining a
    copy of this software and associated documentation files (the "Software"),
    to deal in the Software without restriction, including without limitation
    the rights to use, copy, modify, merge, publish, distribute, sublicense,
    and/or sell copies of the Software, and to permit persons to whom the
    Software is furnished to do so, subject to the following conditions:

    The above copyright notice and this permission notice shall be included in
    all copies or substantial portions of the Software.

    THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
    IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
    FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.  IN NO EVENT SHALL
    THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
    LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
    FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
    DEALINGS IN THE SOFTWARE.

*******************************************************************************/

/*
 * Copyright 2010 Christoph Bumiller
 *
 * Permission is hereby granted, free of charge, to any person obtaining a
 * copy of this software and associated documentation files (the "Software"),
 * to deal in the Software without restriction, including without limitation
 * the rights to use, copy, modify, merge, publish, distribute, sublicense,
 * and/or sell copies of the Software, and to permit persons to whom the
 * Software is furnished to do so, subject to the following conditions:
 *
 * The above copyright notice and this permission notice shall be included in
 * all copies or substantial portions of the Software.
 *
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
 * IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
 * FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.  IN NO EVENT SHALL
 * THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR
 * OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE,
 * ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR
 * OTHER DEALINGS IN THE SOFTWARE.
 */

/*
 * Copyright 2008 Ben Skeggs
 *
 * Permission is hereby granted, free of charge, to any person obtaining a
 * copy of this software and associated documentation files (the "Software"),
 * to deal in the Software without restriction, including without limitation
 * the rights to use, copy, modify, merge, publish, distribute, sublicense,
 * and/or sell copies of the Software, and to permit persons to whom the
 * Software is furnished to do so, subject to the following conditions:
 *
 * The above copyright notice and this permission notice shall be included in
 * all copies or substantial portions of the Software.
 *
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
 * IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
 * FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.  IN NO EVENT SHALL
 * THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR
 * OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE,
 * ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR
 * OTHER DEALINGS IN THE SOFTWARE.
 */

//! Validated BGL solid and sampled draws for A197. Methods: MIT NVIDIA cla197.h;
//! fixed raster state: MIT Mesa 24.3.4 nvc0_{screen,state,vbo,surface}.c.
//! Coordinates and colours retain the shared stream semantics. No pixels are
//! generated here: data contains shader code, constants and vertex inputs.
use super::shaders;
use alloc::{vec, vec::Vec};
use gpu_abi::{
    draw, format, pipeline,
    stream::{self, cmd, Resolver, Words},
};

/// VAB scratch belongs to the channel, beyond a frame's draw release. Mesa's
/// poly_cache is screen-owned (nvc0_screen.c:1274-1283); A197 0x17bc-0x17c4
/// selects that area. Frame backing still owns shader and vertex inputs.
pub fn channel_words(input: &[u32], quarantine: u64) -> Result<Vec<u32>, &'static str> {
    if quarantine == 0 || quarantine & ((1 << 17) - 1) != 0
        || quarantine.checked_add(1 << 20).is_none_or(|end| end > 1 << 40) {
        return Err("invalid channel VAB storage");
    }
    // Frames with no draw packet still receive the publisher's engine release;
    // they do not bind or consume VAB storage.
    if input.is_empty() { return Ok(Vec::new()); }
    let mut words = input.to_vec();
    let (mut at, mut found) = (0, false);
    while at < words.len() {
        let header = words[at];
        let count = ((header >> 16) & 0x1fff) as usize;
        if header & 0xe0000000 != 0x20000000 || count == 0 || at + 1 + count > words.len() {
            return Err("invalid native draw packet extent");
        }
        if (header & 0x1fff) * 4 == 0x17bc {
            if found || count != 3 || words[at + 3] != 3 {
                return Err("invalid native VAB binding");
            }
            words[at + 1] = (quarantine >> 32) as u32;
            words[at + 2] = quarantine as u32;
            found = true;
        }
        at += count + 1;
    }
    if !found { return Err("native draw lacks VAB binding"); }
    Ok(words)
}

pub const DATA_START: usize = (1 << 20) + 8192;
const CODE: usize = 1 << 20;
#[derive(Clone, Copy)]
pub struct Target {
    pub id: u32,
    pub address: u64,
    pub width: u32,
    pub height: u32,
    pub format: u32,
    pub tile_mode: u32,
}
pub trait Bindings: Resolver {
    fn render_target(&self, handle: u64) -> Option<Target>;
    fn sampled_source(&self, handle: u64) -> Option<Target>;
}

/// Warm the installed basic catalogue through the same encoder as client work.
/// Completion is still the native draw release; compiling an asset is not warm-up.
pub fn warm_frame(target: Target, source: Target, pipe: u32) -> Option<Frame> {
    struct View(Target, Target);
    impl Resolver for View {
        fn buffer_size(&self, _: u64) -> Option<u64> { None }
        fn buffer_rect(&self, _: u64, _: u64) -> [i32; 4] { [0; 4] }
        fn target(&self, h: u64) -> Option<stream::TargetInfo> {
            (h == 1).then_some(stream::TargetInfo { width: self.0.width, height: self.0.height,
                format: self.0.format, invalid: false })
        }
        fn source(&self, h: u64) -> Option<stream::SourceInfo> {
            (h == 2).then_some(stream::SourceInfo { width: self.1.width, height: self.1.height,
                format: self.1.format, invalid: false })
        }
    }
    impl Bindings for View {
        fn render_target(&self, h: u64) -> Option<Target> { (h == 1).then_some(self.0) }
        fn sampled_source(&self, h: u64) -> Option<Target> { (h == 2).then_some(self.1) }
    }
    let mut words = Vec::new();
    stream::encode::begin(&mut words, 1);
    if pipe == pipeline::SOLID {
        stream::encode::draw(&mut words, pipe, 0xff123456, &[[0, 0, 1, 1]]);
    } else {
        let mut params = [0; 16];
        match pipe {
            pipeline::COPY => params[2] = 1,
            pipeline::COPY_SCALED => { params[2] = 1; params[3] = 1; params[4] = 1; }
            pipeline::NEAREST_SCALED | pipeline::BOX_FILTER => {
                params[2] = 1; params[3] = 1; params[6] = 1; params[7] = 1;
            }
            _ => return None,
        }
        draw::encode(&mut words, pipe, 0xffffffff, 2, 0, 0,
            &[draw::Instance { rect: [0, 0, 1, 1], params }]);
    }
    translate(words.as_slice(), &View(target, source)).ok()
}

/// Build an inert channel state from the sampled encoder. Its
/// code, constants, vertices and descriptors occupy retained bootstrap VRAM;
/// its two images are separate retained pages. No draw launch is published.
/// VAB remains in the channel's separate 1 MiB scratch allocation.
pub fn parking_frame(target: Target, source: Target, address: u64, quarantine: u64)
    -> Result<(Vec<u8>, Vec<u32>), &'static str> {
    let mut frame = warm_frame(target, source, pipeline::COPY)
        .ok_or("native parking frame refused")?;
    // CODE is a 128 KiB multiple; omit the obsolete per-frame VAB reservation.
    // channel_words redirects that one binding to channel-owned VAB storage.
    frame.relocate(address.checked_sub(CODE as u64).ok_or("native parking VA underflow")?)?;
    let mut parked = Vec::new();
    for chunk in &frame.chunks {
        let words = channel_words(&chunk.words, quarantine)?;
        let mut at = 0;
        while at < words.len() {
            let count = ((words[at] >> 16) & 0x1fff) as usize;
            // A197 BEGIN/END and the vertex array start/count packet launch work in
            // quad(). Keep the complete state, including both program offsets.
            if !matches!((words[at] & 0x1fff)*4, 0x1618 | 0xd74 | 0x1614) {
                parked.extend_from_slice(&words[at..at+count+1]);
            }
            at += count+1;
        }
    }
    Ok((frame.data.split_off(CODE), parked))
}

#[cfg(target_os = "none")]
pub(crate) fn warm(target_id: u32, source_id: u32, pipe: u32) -> Option<Vec<super::adapter::EncodedDraw>> {
    let frame = warm_frame(target(target_id, format::B8G8R8A8)?,
        target(source_id, format::B8G8R8A8)?, pipe)?;
    let backing = alloc::sync::Arc::new(crate::gpu::dma::Pages::new(frame.data.len().div_ceil(4096))?);
    backing.write(0, &frame.data);
    backing.maintain(0, frame.data.len(), false);
    prepare(frame, backing).ok()
}
/// The shared core reserves these controls until device work is queued.
/// Their effects are confined to this frame, as in the SVGA encoder.
#[derive(Clone, Copy, Default)]
pub struct Extras {
    pub corrupt: Option<u64>,
    pub suppress: Option<u64>,
    pub swap: Option<(usize, Target)>,
}
pub struct Chunk {
    pub command: u32,
    pub target: u32,
    pub words: Vec<u32>,
    relocations: Vec<usize>,
    pub source: Option<u32>,
    /// Byte-preserving CE work, resolved and retained during prepare().
    pub copy: Option<(u32, u32, [u32; 6])>,
}
pub struct Frame {
    pub chunks: Vec<Chunk>,
    pub data: Vec<u8>,
}
impl Frame {
    /// Pack adjacent draws from this frame into bounded channel submissions.
    /// Each draw retains its state and relocations; only repeated channel VAB
    /// selection is omitted. Transfers remain separate and ordered.
    pub fn batch_draws(&mut self, limit: usize) {
        let mut batches: Vec<Chunk> = Vec::new();
        for mut chunk in core::mem::take(&mut self.chunks) {
            let prefix = chunk.words.first() == Some(&0x200305ef);
            if let Some(last) = batches.last_mut().filter(|last|
                prefix && chunk.copy.is_none() && last.copy.is_none()
                    && last.target == chunk.target && last.source == chunk.source
                    && last.words.len() + chunk.words.len() - 4 <= limit) {
                let shift = last.words.len() - 4;
                last.relocations.extend(chunk.relocations.drain(..).filter(|&i| i >= 4).map(|i| i + shift));
                last.words.extend_from_slice(&chunk.words[4..]);
            } else {
                batches.push(chunk);
            }
        }
        self.chunks = batches;
    }

    /// Addresses in a fresh frame are relative to one retained system buffer.
    /// Relocation is called once, after its complete mapping has succeeded.
    pub fn relocate(&mut self, base: u64) -> Result<(), &'static str> {
        if base == 0
            || base & ((1 << 17) - 1) != 0
            || base
                .checked_add(self.data.len() as u64)
                .is_none_or(|e| e > 1 << 40)
        {
            return Err("invalid native draw buffer address");
        }
        for c in &mut self.chunks {
            for &i in &c.relocations {
                let offset = (u64::from(c.words[i]) << 32) | u64::from(c.words[i + 1]);
                let address = base.checked_add(offset).ok_or("draw relocation overflow")?;
                c.words[i] = (address >> 32) as u32;
                c.words[i + 1] = address as u32;
            }
            c.relocations.clear();
        }
        Ok(())
    }
}
impl Chunk {
    fn emit(&mut self, method: u32, data: &[u32]) {
        self.words
            .push(0x20000000 | ((data.len() as u32) << 16) | (method / 4));
        self.words.extend_from_slice(data);
    }
    fn address(&mut self, method: u32, offset: usize, tail: &[u32]) {
        self.relocations.push(self.words.len() + 1);
        let mut data = vec![(offset as u64 >> 32) as u32, offset as u32];
        data.extend_from_slice(tail);
        self.emit(method, &data);
    }
}
fn handle(w: &(impl Words + ?Sized), at: usize) -> u64 {
    u64::from(w.word(at)) | u64::from(w.word(at + 1)) << 32
}
/// Catalogue preflight for a stream already accepted by shared validation.
/// Return the ABI refusal at the unsupported command before reserving payloads.
pub fn pipeline_refusal(w: &(impl Words + ?Sized)) -> Option<stream::Refusal> {
    let mut at = 0;
    let mut index = 0;
    while at < w.len() {
        match w.word(at) {
            cmd::BEGIN | cmd::CLEAR | cmd::SCISSOR | cmd::COPY => (),
            cmd::DRAW | cmd::DRAW_BUFFER if w.word(at + 2) == pipeline::SOLID => (),
            cmd::DRAW_SAMPLED => (),
            draw::COMMAND
                if matches!(
                    w.word(at + 2),
                    pipeline::COPY
                        | pipeline::COPY_SCALED
                        | pipeline::NEAREST_SCALED
                        | pipeline::BOX_FILTER
                ) => {}
            _ => return Some(stream::Refusal { index, error: stream::error::BAD_PIPELINE }),
        }
        at += w.word(at + 1) as usize;
        index += 1;
    }
    None
}
/// Full validation precedes allocation or output; unsupported catalogue entries
/// refuse the whole stream as well. Later units extend this same encoder.
pub fn translate(w: &(impl Words + ?Sized), b: &impl Bindings) -> Result<Frame, &'static str> {
    translate_with_extras(w, b, Extras::default())
}
pub fn translate_with_extras(w: &(impl Words + ?Sized), b: &impl Bindings, x: Extras) -> Result<Frame, &'static str> {
    let Extras { mut corrupt, mut suppress, swap } = x;
    stream::validate(w, b).map_err(|_| "invalid BGL draw stream")?;
    if pipeline_refusal(w).is_some() { return Err("native pipeline not implemented"); }
    let mut f = Frame {
        chunks: Vec::new(),
        data: vec![0; DATA_START],
    };
    let segment = shaders::triangle_segment();
    f.data[CODE..CODE + segment.len()].copy_from_slice(&segment);
    for pipe in [
        pipeline::SOLID,
        pipeline::COPY,
        pipeline::COPY_SCALED,
        pipeline::NEAREST_SCALED,
        pipeline::BOX_FILTER,
    ] {
        let (offset, header, code, _) = program(pipe);
        let fs = CODE + offset;
        f.data[fs..fs + 80].copy_from_slice(header);
        f.data[fs + 80..fs + 80 + code.len()].copy_from_slice(code);
    }
    let mut target = None;
    let mut current_handle = 0;
    let mut scissor = None;
    let mut at = 0;
    let mut command = 0;
    while at < w.len() {
        let first_chunk = f.chunks.len();
        let code = w.word(at);
        match code {
            cmd::BEGIN => {
                if let Some(t) = target {
                    if corrupt == Some(current_handle) {
                        corrupt_pixel(&mut f, t);
                        corrupt = None;
                    }
                }
                current_handle = handle(w, at + 2);
                let t = b
                    .render_target(current_handle)
                    .ok_or("native target absent")?;
                if t.id == 0
                    || t.address == 0
                    || t.address & 4095 != 0
                    || t.address >= 1 << 40
                    || t.width == 0
                    || t.height == 0
                    || t.width > 16384
                    || t.height > 16384
                    || !matches!(t.format, format::B8G8R8A8 | format::R8G8B8A8)
                    || t.tile_mode & !0x70 != 0
                    || t.tile_mode > 0x40
                {
                    return Err("invalid native render target");
                }
                target = Some(t);
                scissor = None;
            }
            cmd::SCISSOR => scissor = Some([2, 3, 4, 5].map(|k| w.word(at + k) as i32)),
            cmd::CLEAR | cmd::DRAW | cmd::DRAW_BUFFER => {
                if code != cmd::CLEAR && suppress == Some(current_handle) {
                    suppress = None;
                    at += w.word(at + 1) as usize;
                    command += 1;
                    continue;
                }
                let t = target.ok_or("native target absent")?;
                let clip = stream::clipped(
                    [0, 0, t.width as i32, t.height as i32],
                    t.width,
                    t.height,
                    scissor,
                );
                let colour = w.word(at + if code == cmd::CLEAR { 2 } else { 3 });
                let n = if code == cmd::CLEAR {
                    1
                } else {
                    w.word(at + if code == cmd::DRAW { 4 } else { 7 })
                };
                for i in 0..n as usize {
                    let r = match code {
                        cmd::CLEAR => [0, 0, t.width as i32, t.height as i32],
                        cmd::DRAW => core::array::from_fn(|k| w.word(at + 5 + 4 * i + k) as i32),
                        _ => b.buffer_rect(
                            handle(w, at + 4),
                            u64::from(w.word(at + 6)) + 16 * i as u64,
                        ),
                    };
                    if let (Some(r), Some(s)) =
                        (stream::clipped(r, t.width, t.height, scissor), clip)
                    {
                        solid(&mut f, t, r, s, colour);
                    }
                }
            }
            cmd::COPY => {
                let source = b.sampled_source(handle(w, at + 2)).ok_or("native copy source absent")?;
                let destination = b.render_target(handle(w, at + 8)).ok_or("native copy destination absent")?;
                let rectangle = stream::copy_clip(
                    [4, 5, 6, 7, 10, 11].map(|k| w.word(at + k)),
                    [source.width, source.height], [destination.width, destination.height]);
                // COPY names its own destination and is independent of BEGIN's
                // target and scissor. Empty copies still publish a fenced no-op.
                f.chunks.push(Chunk { command, target: destination.id, source: Some(source.id),
                    words: Vec::new(), relocations: Vec::new(),
                    copy: rectangle.map(|r| (source.id, destination.id, r)) });
            }
            cmd::DRAW_SAMPLED | draw::COMMAND => {
                let t = target.ok_or("native target absent")?;
                let legacy = code == cmd::DRAW_SAMPLED;
                let pipe = w.word(at + 2);
                let h = handle(w, at + if legacy { 3 } else { 4 });
                let info = b.source(h).ok_or("native source absent")?;
                let source = b.sampled_source(h).ok_or("native source storage absent")?;
                texture_image_control(source)?;
                if source.id == t.id
                    || source.width != info.width
                    || source.height != info.height
                    || source.format != info.format
                {
                    return Err("invalid native sampled source");
                }
                let source = match swap {
                    Some((word, other)) if word == at => other,
                    _ => source,
                };
                texture_image_control(source)?;
                if source.id == t.id { return Err("native sampled source aliases target"); }
                // Bound the substitute as well as the validated named texture.
                let clipped_info = stream::SourceInfo {
                    width: info.width.min(source.width),
                    height: info.height.min(source.height),
                    ..info
                };
                let Some(clip) = stream::clipped(
                    [0, 0, t.width as i32, t.height as i32],
                    t.width,
                    t.height,
                    scissor,
                ) else {
                    at += w.word(at + 1) as usize;
                    command += 1;
                    continue;
                };
                let count = w.word(at + if legacy { 5 } else { 9 }) as usize;
                for n in 0..count {
                    let mut constants = [0; 32];
                    let r = if legacy {
                        let base = at + 6 + n * 6;
                        let i = core::array::from_fn(|k| w.word(base + k) as i32);
                        constants[0] = (i[4] as u32).wrapping_sub(i[0] as u32);
                        constants[1] = (i[5] as u32).wrapping_sub(i[1] as u32);
                        stream::sampled_clip(i, t.width, t.height, clipped_info.width, clipped_info.height)
                            .and_then(|r| {
                                stream::clipped(r.map(|v| v as i32), t.width, t.height, scissor)
                            })
                    } else {
                        let base = at + draw::HEADER + n * draw::INSTANCE;
                        let i = draw::Instance {
                            rect: core::array::from_fn(|k| w.word(base + k) as i32),
                            params: core::array::from_fn(|k| w.word(base + 4 + k)),
                        };
                        constants[..16].copy_from_slice(&i.params);
                        constants[20] = w.word(at + 3);
                        draw::source_clip(pipe, i, Some(clipped_info))
                            .and_then(|i| draw::clip(pipe, i, t.width, t.height, scissor))
                    };
                    constants[24] = source.width;
                    constants[25] = source.height;
                    if let Some(r) = r {
                        quad(&mut f, t, r, clip, &constants, pipe, Some(source));
                    }
                }
            }
            _ => unreachable!(),
        }
        for chunk in &mut f.chunks[first_chunk..] { chunk.command = command; }
        command += 1;
        at += w.word(at + 1) as usize;
    }
    if let Some(t) = target {
        if corrupt == Some(current_handle) { corrupt_pixel(&mut f, t); }
    }
    if f.chunks.is_empty() {
        let t = target.ok_or("native target absent")?;
        f.chunks.push(Chunk {
            command,
            target: t.id,
            words: Vec::new(),
            relocations: Vec::new(),
            source: None,
            copy: None,
        });
    }
    Ok(f)
}
/// TIC v2, single-level block-linear 2D. MIT rnndb g80_texture.xml,
/// g80_defs.xml and Mesa nvc0_tex.c:309-436. TXF consumes integer coordinates;
/// swizzles expose logical RGBA, including R8 -> (r, 0, 0, 1).
fn texture_image_control(t: Target) -> Result<[u32; 8], &'static str> {
    if t.id == 0
        || t.address == 0
        || t.address & 4095 != 0
        || t.address >= 1 << 40
        || t.width == 0
        || t.height == 0
        || t.width > 16384
        || t.height > 16384
        || t.tile_mode & !0x70 != 0
        || t.tile_mode > 0x40
    {
        return Err("invalid native texture layout");
    }
    let (size, swizzle) = match t.format {
        format::B8G8R8A8 => (8, [4, 3, 2, 5]),
        format::R8G8B8A8 => (8, [2, 3, 4, 5]),
        format::R8 => (0x1d, [2, 0, 0, 7]),
        _ => return Err("invalid native texture format"),
    };
    let types = (2 << 6) | (2 << 9) | (2 << 12) | (2 << 15); // UNORM
    Ok([
        size | types | swizzle[0] << 18 | swizzle[1] << 21 | swizzle[2] << 24 | swizzle[3] << 27,
        t.address as u32,
        (t.address >> 32) as u32 | 0x50005000 | (t.tile_mode << 18),
        0x00300000,
        0x80000000 | t.width,
        0x10000 | t.height,
        0x03000000,
        0,
    ])
}
const PROBE_INTEGER: u32 = u32::MAX - 1;
const PROBE_TEXEL: u32 = u32::MAX;

fn program(pipe: u32) -> (usize, &'static [u8; 80], &'static [u8], u32) {
    match pipe {
        pipeline::SOLID => (
            shaders::FRAGMENT_OFFSET,
            shaders::PIPELINE_SOLID_HEADER,
            shaders::PIPELINE_SOLID_CODE,
            shaders::PIPELINE_SOLID_GPRS,
        ),
        pipeline::COPY => (
            0x2b0,
            shaders::PIPELINE_COPY_HEADER,
            shaders::PIPELINE_COPY_CODE,
            shaders::PIPELINE_COPY_GPRS,
        ),
        pipeline::COPY_SCALED => (
            0x430,
            shaders::PIPELINE_COPY_SCALED_HEADER,
            shaders::PIPELINE_COPY_SCALED_CODE,
            shaders::PIPELINE_COPY_SCALED_GPRS,
        ),
        pipeline::NEAREST_SCALED => (
            0xa30,
            shaders::PIPELINE_NEAREST_SCALED_HEADER,
            shaders::PIPELINE_NEAREST_SCALED_CODE,
            shaders::PIPELINE_NEAREST_SCALED_GPRS,
        ),
        pipeline::BOX_FILTER => (
            0xf30,
            shaders::PIPELINE_BOX_FILTER_HEADER,
            shaders::PIPELINE_BOX_FILTER_CODE,
            shaders::PIPELINE_BOX_FILTER_GPRS,
        ),
        // Mesa nvc0_program.c:775-799: first instruction follows the 80-byte
        // SPH at a 128-byte boundary, where Kepler expects scheduling words.
        PROBE_INTEGER => (
            0x2030,
            shaders::PROBE_INTEGER_OPS_HEADER,
            shaders::PROBE_INTEGER_OPS_CODE,
            shaders::PROBE_INTEGER_OPS_GPRS,
        ),
        PROBE_TEXEL => (
            0x2430,
            shaders::PROBE_TEXEL_FETCH_HEADER,
            shaders::PROBE_TEXEL_FETCH_CODE,
            shaders::PROBE_TEXEL_FETCH_GPRS,
        ),
        _ => unreachable!(),
    }
}
/// Admission-only programs reuse the same u4/u5 quad, raster, TIC/TSC and
/// relocation builders. They are not new public catalogue pipeline IDs.
pub fn probe_frame(t: Target, source: Option<Target>) -> Frame {
    use super::caps::{PROBE_HEIGHT as H, PROBE_WIDTH as W};
    let pipe = if source.is_some() {
        PROBE_TEXEL
    } else {
        PROBE_INTEGER
    };
    let mut f = Frame {
        chunks: Vec::new(),
        data: vec![0; DATA_START],
    };
    let segment = shaders::triangle_segment();
    f.data[CODE..CODE + segment.len()].copy_from_slice(&segment);
    let (offset, header, code, _) = program(pipe);
    f.data
        .resize(DATA_START.max(CODE + offset + 80 + code.len()), 0);
    f.data[CODE + offset..CODE + offset + 80].copy_from_slice(header);
    f.data[CODE + offset + 80..CODE + offset + 80 + code.len()].copy_from_slice(code);
    if let Some(source) = source {
        quad(
            &mut f,
            t,
            [0, 0, W, H],
            [0, 0, W, H],
            &[0; 4],
            pipe,
            Some(source),
        );
    } else {
        let inputs = include_bytes!("caps/integer.inputs");
        for (n, input) in inputs.chunks_exact(16).enumerate() {
            let values: [u32; 4] = core::array::from_fn(|k| {
                u32::from_le_bytes(input[k * 4..k * 4 + 4].try_into().unwrap())
            });
            let (x, y) = (n as u32 % W, n as u32 / W);
            quad(
                &mut f,
                t,
                [x, y, x + 1, y + 1],
                [0, 0, W, H],
                &values,
                pipe,
                None,
            );
        }
    }
    f
}
/// Point 22 uses the solid shader at (0,0), outside the suite's draw/scissor.
fn corrupt_pixel(f: &mut Frame, t: Target) {
    solid(f, t, [0, 0, 1, 1], [0, 0, 1, 1], !0x3c5a_a5c3);
}
fn solid(f: &mut Frame, t: Target, r: [u32; 4], scissor: [u32; 4], colour: u32) {
    let constants =
        [16, 8, 0, 24].map(|shift| gpu_abi::float::ratio_bits(colour >> shift & 255, 255));
    quad(f, t, r, scissor, &constants, pipeline::SOLID, None);
}
fn quad(
    f: &mut Frame,
    t: Target,
    r: [u32; 4],
    scissor: [u32; 4],
    constants: &[u32],
    pipe: u32,
    source: Option<Target>,
) {
    let uniform = f.data.len().next_multiple_of(256);
    let vertex = uniform + 256;
    let tic = vertex + 256;
    let tsc = tic + 256;
    let handles = tsc + 256;
    f.data.resize(
        if source.is_some() {
            handles + 256
        } else {
            vertex + 256
        },
        0,
    );
    for (k, v) in constants.iter().enumerate() {
        f.data[uniform + k * 4..uniform + k * 4 + 4].copy_from_slice(&v.to_le_bytes());
    }
    if let Some(source) = source {
        // Validation above established this descriptor before any draw chunks.
        for (offset, words) in [
            (tic, texture_image_control(source).unwrap()),
            (tsc, [0x92, 0x51, 0, 0, 0, 0, 0, 0]),
        ] {
            for (k, v) in words.iter().enumerate() {
                f.data[offset + k * 4..offset + k * 4 + 4].copy_from_slice(&v.to_le_bytes());
            }
        }
        // The compiler emits handle 0 in the texture CB. TIC 0 | TSC 0 is 0.
    }
    let [x0, y0, x1, y1] = r;
    for (i, [x, y]) in [[x0, y0], [x1, y0], [x1, y1], [x0, y0], [x1, y1], [x0, y1]]
        .into_iter()
        .enumerate()
    {
        let coord = |p: u32, size: u32| {
            let n = i64::from(p) * 2 - i64::from(size);
            gpu_abi::float::ratio_bits(n.unsigned_abs() as u32, size)
                | if n < 0 { 1 << 31 } else { 0 }
        };
        for (k, v) in [
            coord(x, t.width),
            coord(y, t.height),
            0,
            0x3f800000,
            0,
            0,
            0,
            0,
        ]
        .into_iter()
        .enumerate()
        {
            f.data[vertex + i * 32 + k * 4..vertex + i * 32 + k * 4 + 4]
                .copy_from_slice(&v.to_le_bytes());
        }
    }
    let mut c = Chunk {
        command: 0,
        target: t.id,
        words: Vec::new(),
        relocations: Vec::new(),
        source: source.map(|s| s.id),
        copy: None,
    };
    // IDXFETCH quarantine: 1 MiB, 128 KiB aligned (Mesa screen:1274-1283).
    c.address(0x17bc, 0, &[3]);
    c.emit(
        0x800,
        &[
            (t.address >> 32) as u32,
            t.address as u32,
            t.width,
            t.height,
            if t.format == format::B8G8R8A8 {
                0xcf
            } else {
                0xd5
            },
            t.tile_mode,
            1,
            0,
            0,
        ],
    );
    c.emit(0x121c, &[1]);
    for method in [0x1538, 0x12cc, 0x12e8, 0x1380, 0x1594, 0x19bc] {
        c.emit(method, &[0]);
    }
    let halfw = gpu_abi::float::scaled_int_bits(i64::from(t.width), 1);
    let halfh = gpu_abi::float::scaled_int_bits(i64::from(t.height), 1);
    c.emit(0xa00, &[halfw, halfh, 0x3f800000, halfw, halfh, 0]);
    c.emit(0xc00, &[t.width << 16, t.height << 16, 0, 0x3f800000]);
    let [sx0, sy0, sx1, sy1] = scissor;
    c.emit(0xe00, &[1, sx0 | sx1 << 16, sy0 | sy1 << 16]);
    c.emit(0xff4, &[t.width << 16, t.height << 16]);
    c.emit(0xd00, &[t.width << 16, t.height << 16]);
    c.emit(0x194c, &[1]);
    c.emit(0x1950, &[0]);
    c.emit(0x0df8, &[0, 0]);
    c.emit(0x13ac, &[0]);
    c.emit(0x1924, &[0]);
    c.emit(0x192c, &[1]);
    c.emit(0x193c, &[2]);
    c.emit(0x1510, &[0]);
    c.emit(0x1918, &[0]);
    c.emit(0x0dac, &[0x1b02, 0x1b02]);
    c.emit(0x1360, &[0]);
    c.emit(0x135c, &[0]);
    c.emit(0x19c4, &[0]);
    c.emit(0x0f90, &[1]);
    c.emit(0x1a00, &[0x1111]);
    for method in [0x1534, 0x15d0, 0x153c] {
        c.emit(method, &[0]);
    }
    // MIT envytools rnndb graph/gf100_3d.xml:217-222: one pass, sample shading disabled.
    c.emit(0x0754, &[1]);
    c.emit(0x0fbc, &[0xffff; 4]);
    c.emit(0x15b8, &[0]);
    c.emit(0x19e0, &[0]);
    c.emit(0x12d4, &[0x1d01]);
    c.emit(0x12ec, &[0]);
    c.emit(0x037c, &[1]);
    c.emit(0x0fac, &[1]);
    c.emit(0x1558, &[1]);
    c.emit(0x1644, &[0]);
    c.emit(0x15e4, &[1]);
    // Mesa screen:688-761, Kepler branch; no MME macros are installed or used.
    c.emit(0x10cc, &[0xff]);
    c.emit(0x10e0, &[0xff, 0xff]);
    c.emit(0x10ec, &[0xff, 0xff]);
    c.emit(0x074c, &[0x3f]);
    c.emit(0x16a8, &[0x30003]);
    c.emit(0x1794, &[0x20002]);
    c.emit(0x12ac, &[0]);
    c.emit(0x0218, &[0x10]);
    c.emit(0x10fc, &[0x10]);
    c.emit(0x1290, &[0x10]);
    c.emit(0x12d8, &[0x10, 0x10]);
    c.emit(0x1140, &[0x10]);
    c.emit(0x1610, &[0xe]);
    c.emit(0x161c, &[0]);
    c.emit(0x030c, &[0]);
    c.emit(0x0300, &[3]);
    c.emit(0x02d0, &[0x3fffff]);
    c.emit(0x0fdc, &[1]);
    c.emit(0x19c0, &[1]);
    c.emit(0x075c, &[3]);
    c.emit(0x07fc, &[1]);

    c.address(0x1608, CODE, &[]);
    c.emit(0x021c, &[0x1011]); // invalidate instruction cache, all graphics stages
    for slot in [0, 2, 3, 4] {
        c.emit(0x2000 + slot * 64, &[slot << 4]);
    }
    c.emit(0x2040, &[0x11, shaders::VERTEX_OFFSET as u32]);
    c.emit(0x204c, &[shaders::VERTEX_GPRS]);
    let (offset, _, _, gprs) = program(pipe);
    c.emit(0x2140, &[0x51, offset as u32]);
    c.emit(0x214c, &[gprs]);
    c.emit(0x2380, &[256, 0, uniform as u32]);
    c.relocations.push(c.words.len() - 2);
    c.emit(0x2490, &[1]);
    if source.is_some() {
        c.address(0x1574, tic, &[0]);
        c.address(0x155c, tsc, &[0]);
        c.emit(0x1334, &[0]);
        c.emit(0x1330, &[0]);
        c.emit(0x1338, &[0]);
        c.emit(0x2608, &[15]);
        c.emit(0x2380, &[256, 0, handles as u32]);
        c.relocations.push(c.words.len() - 2);
        c.emit(0x2490, &[0xf1]);
    }
    c.emit(0x1930, &[1]);
    c.emit(0x1880, &[0]);
    c.emit(0x1c00, &[0x1020, 0, vertex as u32, 0]);
    c.relocations.push(c.words.len() - 3);
    c.address(0x1f00, vertex + 191, &[]);
    c.emit(0x1160, &[0x38200000, 0x38200800]);
    c.emit(0x142c, &[0]);
    c.emit(0x21c, &[1]);
    c.emit(0x1618, &[4]);
    c.emit(0xd74, &[0, 6]);
    c.emit(0x1614, &[0]);
    c.emit(0x110, &[0]);
    f.chunks.push(c);
}

#[cfg(target_os = "none")]
pub(in crate::gk208) struct Storage {
    _backing: alloc::sync::Arc<crate::gpu::dma::Pages>,
    holds: Vec<super::resource::Hold>,
    pub(in crate::gk208) resource: u32,
    pub(in crate::gk208) address: u64,
}
#[cfg(target_os = "none")]
impl Drop for Storage {
    fn drop(&mut self) {
        self.holds.clear();
        let _ = super::ce::with_resources(|r, a, io| r.collect(a, io));
    }
}
#[cfg(target_os = "none")]
pub(crate) fn prepare(
    mut frame: Frame,
    backing: alloc::sync::Arc<crate::gpu::dma::Pages>,
) -> Result<Vec<super::adapter::EncodedDraw>, &'static str> {
    use alloc::sync::Arc;
    let pages: Vec<_> = (0..backing.count()).map(|i| backing.page_addr(i)).collect();
    let mut regions = vec![
        super::resource::FrameRegion { start: 0, end: CODE as u64, name: "frame-reserved", command: 0 },
        super::resource::FrameRegion { start: CODE as u64, end: DATA_START as u64, name: "code", command: 0 },
    ];
    for c in &frame.chunks {
        let mut at = 0;
        while at < c.words.len() {
            let count = ((c.words[at] >> 16) & 0x1fff) as usize;
            let method = (c.words[at] & 0x1fff) * 4;
            let region = match method {
                0x2380 => Some((u64::from(c.words[at+3]), 256, "constants")),
                0x1c00 => Some((u64::from(c.words[at+3]), 256, "vertex")),
                0x1574 => Some((u64::from(c.words[at+2]), 32, "texture-descriptor")),
                0x155c => Some((u64::from(c.words[at+2]), 32, "sampler-descriptor")),
                _ => None,
            };
            if let Some((start, bytes, name)) = region {
                regions.push(super::resource::FrameRegion { start, end: start+bytes, name, command: c.command });
            }
            at += count+1;
        }
    }
    let (id, address, holds, transfers) = super::ce::with_resources(|r, a, io| {
        let mut holds = Vec::new();
        let mut transfers = Vec::new();
        for c in &frame.chunks {
            holds.push(r.hold(c.target)?);
            if let Some(source) = c.source {
                holds.push(r.hold(source)?);
            }
            transfers.push(if let Some((source, destination, rectangle)) = c.copy {
                let (plan, retained) = super::ce::readback::region_plan(r, a, io, source, destination, rectangle)?;
                holds.extend(retained);
                Some(plan)
            } else { None });
        }
        let id = r.draw_buffer(a, io, backing.bytes() as u32)?;
        r.attach(a, io, id, backing.clone(), &pages, backing.bytes() as u64)?;
        r.describe_frame(id, regions)?;
        let address = r.address(id).ok_or("native draw upload absent")?;
        holds.push(r.hold(id)?);
        r.unref(id)?;
        Ok((id, address, holds, transfers))
    })?;
    let storage = Arc::new(Storage {
        _backing: backing,
        holds,
        resource: id,
        address,
    });
    frame.relocate(address)?;
    Ok(frame
        .chunks
        .into_iter()
        .zip(transfers)
        .map(|(c, transfer)| super::adapter::EncodedDraw {
            target: c.target,
            command: c.command,
            words: c.words,
            transfer,
            _storage: storage.clone(),
        })
        .collect())
}

#[cfg(target_os = "none")]
pub(crate) fn target(id: u32, format: u32) -> Option<Target> {
    super::ce::with_resources(|r, _, _| {
        let l = r.layout(id).ok_or("native target layout absent")?;
        Ok(Target {
            id,
            address: r.address(id).ok_or("native target storage absent")?,
            width: l.width,
            height: l.height,
            format,
            tile_mode: l.tile_mode,
        })
    })
    .ok()
}

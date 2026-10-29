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

//! Validated BGL solid draws for A197. Methods: MIT NVIDIA cla197.h;
//! fixed raster state: MIT Mesa 24.3.4 nvc0_{screen,state,vbo,surface}.c.
//! Coordinates and colours retain the shared stream semantics. No pixels are
//! generated here: data contains shader code, constants and vertex inputs.
use super::shaders;
use alloc::{vec, vec::Vec};
use gpu_abi::{
    format, pipeline,
    stream::{self, cmd, Resolver, Words},
};

pub const DATA_START: usize = (1 << 20) + 4096;
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
}
pub struct Chunk {
    pub target: u32,
    pub words: Vec<u32>,
    relocations: Vec<usize>,
}
pub struct Frame {
    pub chunks: Vec<Chunk>,
    pub data: Vec<u8>,
}
impl Frame {
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
/// Full validation precedes allocation or output; unsupported catalogue entries
/// refuse the whole stream as well. Later units extend this same encoder.
pub fn translate(w: &(impl Words + ?Sized), b: &impl Bindings) -> Result<Frame, &'static str> {
    stream::validate(w, b).map_err(|_| "invalid BGL draw stream")?;
    let mut at = 0;
    while at < w.len() {
        match w.word(at) {
            cmd::BEGIN | cmd::CLEAR | cmd::SCISSOR => (),
            cmd::DRAW | cmd::DRAW_BUFFER if w.word(at + 2) == pipeline::SOLID => (),
            _ => return Err("native pipeline not implemented"),
        }
        at += w.word(at + 1) as usize;
    }
    let mut f = Frame {
        chunks: Vec::new(),
        data: vec![0; DATA_START],
    };
    let segment = shaders::triangle_segment();
    f.data[CODE..CODE + segment.len()].copy_from_slice(&segment);
    let fs = CODE + shaders::FRAGMENT_OFFSET;
    f.data[fs..fs + 80].copy_from_slice(shaders::PIPELINE_SOLID_HEADER);
    f.data[fs + 80..fs + 80 + shaders::PIPELINE_SOLID_CODE.len()]
        .copy_from_slice(shaders::PIPELINE_SOLID_CODE);
    let mut target = None;
    let mut scissor = None;
    at = 0;
    while at < w.len() {
        let code = w.word(at);
        match code {
            cmd::BEGIN => {
                let t = b
                    .render_target(handle(w, at + 2))
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
            _ => unreachable!(),
        }
        at += w.word(at + 1) as usize;
    }
    if f.chunks.is_empty() {
        let t = target.ok_or("native target absent")?;
        f.chunks.push(Chunk {
            target: t.id,
            words: Vec::new(),
            relocations: Vec::new(),
        });
    }
    Ok(f)
}
fn solid(f: &mut Frame, t: Target, r: [u32; 4], scissor: [u32; 4], colour: u32) {
    let uniform = f.data.len().next_multiple_of(256);
    let vertex = uniform + 256;
    f.data.resize(vertex + 256, 0);
    for (k, shift) in [16, 8, 0, 24].into_iter().enumerate() {
        let v = gpu_abi::float::ratio_bits(colour >> shift & 255, 255).to_le_bytes();
        f.data[uniform + k * 4..uniform + k * 4 + 4].copy_from_slice(&v);
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
        target: t.id,
        words: Vec::new(),
        relocations: Vec::new(),
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
    c.emit(0x2140, &[0x51, shaders::FRAGMENT_OFFSET as u32]);
    c.emit(0x214c, &[shaders::PIPELINE_SOLID_GPRS]);
    c.emit(0x2380, &[256, 0, uniform as u32]);
    c.relocations.push(c.words.len() - 2);
    c.emit(0x2490, &[1]);
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
    let (address, holds) = super::ce::with_resources(|r, a, io| {
        let mut holds = Vec::new();
        for c in &frame.chunks {
            holds.push(r.hold(c.target)?);
        }
        let id = r.draw_buffer(a, io, backing.bytes() as u32)?;
        r.attach(a, io, id, backing.clone(), &pages, backing.bytes() as u64)?;
        let address = r.address(id).ok_or("native draw upload absent")?;
        holds.push(r.hold(id)?);
        r.unref(id)?;
        Ok((address, holds))
    })?;
    let storage = Arc::new(Storage {
        _backing: backing,
        holds,
    });
    frame.relocate(address)?;
    Ok(frame
        .chunks
        .into_iter()
        .map(|c| super::adapter::EncodedDraw {
            target: c.target,
            words: c.words,
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

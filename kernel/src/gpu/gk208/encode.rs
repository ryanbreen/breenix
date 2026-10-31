/*
 * SPDX-FileCopyrightText: Copyright (c) 2001-2010 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
 * SPDX-License-Identifier: MIT
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
 * FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL
 * THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
 * LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
 * FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
 * DEALINGS IN THE SOFTWARE.
 */

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
//! Profile references: open-gpu-doc 9fdf5c4062007929d9f4e6cbad9c9771fe61b880;
//! Mesa mesa-24.3.4 769e51468b49b2a42f0a0eaf71cf9eed5ff4e5de (MIT).
use super::shaders;
use alloc::vec::Vec;
use gpu_abi::{
    draw, format, pipeline,
    stream::{self, cmd, Resolver, Words},
};

pub const NO_MEMORY: &str = "native encoder allocation failed";
#[cfg(target_os = "none")]
type FrameBytes = crate::gpu::byte_buffer::Bytes;
#[cfg(not(target_os = "none"))]
type FrameBytes = Vec<u8>;
fn new_data(len: usize) -> Result<FrameBytes, &'static str> {
    #[cfg(target_os = "none")]
    { FrameBytes::new(len).map_err(|_| NO_MEMORY) }
    #[cfg(not(target_os = "none"))]
    {
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(len).map_err(|_| NO_MEMORY)?;
        bytes.resize(len, 0);
        Ok(bytes)
    }
}
fn resize_data(bytes: &mut FrameBytes, len: usize) -> Result<(), &'static str> {
    #[cfg(target_os = "none")]
    { bytes.resize(len).map_err(|_| NO_MEMORY) }
    #[cfg(not(target_os = "none"))]
    {
        bytes.try_reserve(len.saturating_sub(bytes.len())).map_err(|_| NO_MEMORY)?;
        bytes.resize(len, 0);
        Ok(())
    }
}
fn copy_bytes(bytes: &[u8]) -> Result<Vec<u8>, &'static str> {
    let mut copy = Vec::new();
    copy.try_reserve_exact(bytes.len()).map_err(|_| NO_MEMORY)?;
    copy.extend_from_slice(bytes);
    Ok(copy)
}

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
    let mut words = Vec::new();
    words.try_reserve_exact(input.len()).map_err(|_| NO_MEMORY)?;
    words.extend_from_slice(input);
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

/// Bind each draw's unchanged program offsets to the catalogue retained by the
/// channel's parking state. Frame retirement cannot revoke this code storage.
pub fn channel_programs(input: &mut [u32], parking: &[u32]) -> Result<(), &'static str> {
    let mut code = None;
    let mut at = 0;
    while at < parking.len() {
        let header = parking[at];
        let count = ((header >> 16) & 0x1fff) as usize;
        if header & 0xe0000000 != 0x20000000 || count == 0 || at + count >= parking.len() {
            return Err("invalid native parking packet extent");
        }
        if (header & 0x1fff) * 4 == 0x1608 {
            if count != 2 || code.is_some() { return Err("invalid native parked code binding"); }
            let address = u64::from(parking[at+1]) << 32 | u64::from(parking[at+2]);
            if address == 0 || address & ((1 << 17) - 1) != 0
                || address.checked_add(shaders::CATALOGUE_CODE_SIZE as u64).is_none_or(|end| end > 1 << 40) {
                return Err("invalid native parked code address");
            }
            code = Some([parking[at+1], parking[at+2]]);
        }
        at += count + 1;
    }
    let code = code.ok_or("native channel catalogue absent")?;
    at = 0;
    while at < input.len() {
        let header = input[at];
        let count = ((header >> 16) & 0x1fff) as usize;
        if header & 0xe0000000 != 0x20000000 || count == 0 || at + count >= input.len() {
            return Err("invalid native draw packet extent");
        }
        if (header & 0x1fff) * 4 == 0x1608 {
            if count != 2 { return Err("invalid native frame code binding"); }
            input[at+1..at+3].copy_from_slice(&code);
        }
        at += count + 1;
    }
    Ok(())
}

// Reserve the complete committed shader catalogue before per-draw data.
pub const DATA_START: usize = (1 << 20) + shaders::CATALOGUE_CODE_SIZE;
const CODE: usize = 1 << 20;
/// Bound native payload and method storage after shared validation. `drawn`
/// counts clipped rectangles, not empty DRAW declarations or clipped instances.
/// Shared CLEAR uses a virtual clear packet and only counts its scissored form
/// in `drawn`; native CLEAR needs a quad for a nonempty clip. COPY emits a chunk
/// even when empty, so include both commands conservatively.
/// Each blended command adds at most one snapshot, and only if it draws.
pub fn reservation_bytes(w: &(impl Words + ?Sized), plan: &stream::Plan, corrupt: bool) -> Option<u64> {
    let mut fixed = 0u64;
    let mut snapshots = 0u64;
    let mut at = 0;
    while at < w.len() {
        match w.word(at) {
            cmd::CLEAR | cmd::COPY => fixed += 1,
            cmd::DRAW_SAMPLED | draw::COMMAND if draw::blended(w.word(at + 2)) => snapshots += 1,
            _ => (),
        }
        at += w.word(at + 1) as usize;
    }
    // A 4 KiB allowance covers a quad's aligned inputs and bounded methods,
    // or a copy chunk. Keep one for a fenced no-op, plus the temporary shader
    // prefix and page padding. Scratch pixels are reserved separately.
    plan.drawn.checked_add(fixed)?
        .checked_add(plan.drawn.min(snapshots))?
        .checked_add(u64::from(corrupt))?.max(1)
        .checked_mul(4096)?.checked_add(DATA_START as u64 + 4096)
}
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
    /// Caller-owned destination snapshot for this format, retaining pixel
    /// coordinates. Blended commands refuse if the required scratch is absent.
    fn blend_scratch(&self, _format: u32) -> Option<Target> { None }
    /// Frame-owned RGBA8 crossing tables, uploaded before preparation.
    fn span_texture(&self) -> Option<Target> { None }
}

/// Shared coverage-spans words in little-endian RGBA8 texels with 0-filled row padding.
pub struct Spans {
    pub width: u32,
    pub height: u32,
    pub bytes: FrameBytes,
}
/// The input has passed shared stream validation. Match VirGL's first-use order
/// and maximum referenced extent per handle, including later commands' extents.
fn span_tables(w: &(impl Words + ?Sized)) -> Result<Vec<(u64, u32, u32)>, &'static str> {
    let mut tables: Vec<(u64, u32, u32)> = Vec::new();
    let mut at = 0;
    while at < w.len() {
        if w.word(at) == draw::COMMAND && w.word(at + 2) == pipeline::COVERAGE_SPANS {
            let h = handle(w, at + 6);
            let bytes = w.word(at + 8);
            if let Some(entry) = tables.iter_mut().find(|e| e.0 == h) {
                entry.1 = entry.1.max(bytes);
            } else {
                tables.try_reserve(1).map_err(|_| NO_MEMORY)?;
                tables.push((h, bytes, 0));
            }
        }
        at += w.word(at + 1) as usize;
    }
    let mut base = 0;
    for entry in &mut tables { entry.2 = base; base += entry.1 / 4; }
    Ok(tables)
}
/// Pack each named crossing buffer exactly once, from the validated snapshot.
/// The supplied power-of-two limit bounds the row; short tables use only the
/// next power of two covering their words. The shader receives the actual width.
pub fn pack_spans(w: &(impl Words + ?Sized), b: &impl Bindings, width: u32)
    -> Result<Option<Spans>, &'static str> {
    if !width.is_power_of_two() || width > gpu_abi::gk208::MAX_2D_SIZE {
        return Err("invalid native crossing texture width");
    }
    let tables = span_tables(w)?;
    let Some(&(_, bytes, base)) = tables.last() else { return Ok(None); };
    let words = base + bytes / 4;
    let width = width.min(words.checked_next_power_of_two().ok_or("native crossing size overflow")?);
    let height = words.div_ceil(width);
    if height == 0 || height > gpu_abi::gk208::MAX_2D_SIZE {
        return Err("invalid native crossing texture height");
    }
    let size = width as usize * height as usize * 4;
    let bytes = new_data(size).map_err(|_| "native crossing allocation")?;
    let mut data = Spans { width, height, bytes };
    for (h, bytes, base) in tables {
        for word in 0..bytes / 4 {
            let at = (base + word) as usize * 4;
            data.bytes[at..at + 4].copy_from_slice(&b.buffer_word(h, u64::from(word) * 4).to_le_bytes());
        }
    }
    Ok(Some(data))
}

// One covered pixel: eight subpixel rows, each with crossings at x=0 and x=1.
// These are inputs to the same coverage-spans encoder and shader as client data.
const WARM_SPANS: [u32; 41] = [
    0, 2, 4, 6, 8, 10, 12, 14, 16,
    0, 1, 0x3f800000, u32::MAX, 0, 1, 0x3f800000, u32::MAX,
    0, 1, 0x3f800000, u32::MAX, 0, 1, 0x3f800000, u32::MAX,
    0, 1, 0x3f800000, u32::MAX, 0, 1, 0x3f800000, u32::MAX,
    0, 1, 0x3f800000, u32::MAX, 0, 1, 0x3f800000, u32::MAX,
];

/// The crossing texture is uploaded through CE before its warm draw is prepared.
pub fn warm_spans() -> Result<Spans, &'static str> {
    let mut bytes = new_data(64 * 4)?;
    for (word, value) in WARM_SPANS.iter().enumerate() {
        bytes[word * 4..word * 4 + 4].copy_from_slice(&value.to_le_bytes());
    }
    Ok(Spans { width: 64, height: 1, bytes })
}

/// Warm each rendering pipeline through validation and the client encoder.
/// Completion is still the native draw release; compiling an asset is not warm-up.
/// `source` is R8 for BITMAP_TEXT, BGRA8 otherwise. Blends require a separate
/// destination snapshot, and COVERAGE_SPANS additionally requires uploaded data.
pub fn warm_frame(target: Target, source: Target, snapshot: Option<Target>,
    spans: Option<Target>, pipe: u32) -> Option<Frame> {
    struct View(Target, Target, Option<Target>, Option<Target>);
    impl Resolver for View {
        fn buffer_size(&self, h: u64) -> Option<u64> {
            (h == 3).then_some((WARM_SPANS.len() * 4) as u64)
        }
        fn buffer_rect(&self, _: u64, _: u64) -> [i32; 4] { [0; 4] }
        fn buffer_word(&self, h: u64, offset: u64) -> u32 {
            if h == 3 { WARM_SPANS[offset as usize / 4] } else { 0 }
        }
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
        fn blend_scratch(&self, format: u32) -> Option<Target> {
            self.2.filter(|s| s.format == format)
        }
        fn span_texture(&self) -> Option<Target> { self.3 }
    }
    let mut words = Vec::new();
    stream::encode::begin(&mut words, 1);
    if pipe == pipeline::SOLID {
        stream::encode::draw(&mut words, pipe, 0xff123456, &[[0, 0, 1, 1]]);
    } else {
        let mut params = [0; 16];
        let inputs: &[u32] = match pipe {
            pipeline::COPY => &[0, 0, 1],
            pipeline::COPY_SCALED => &[0, 0, 0, 0, 1],
            pipeline::NEAREST_SCALED | pipeline::BOX_FILTER => &[0, 0, 1, 1, 0, 0, 1, 1],
            pipeline::OVER | pipeline::OVER_STRAIGHT | pipeline::LINE
                | pipeline::CIRCLE_FILL | pipeline::CIRCLE_OUTLINE => &[],
            pipeline::COVERAGE => &[0, 0, 0, 0, 1],
            pipeline::BITMAP_TEXT => &[0, 0, 0, 0, 1, 1, 1, 1],
            pipeline::COVERAGE_SPANS => &[0, 0, 1, 1, 0],
            pipeline::ROUNDED_RECT => &[0, 0, 1, 1, 0],
            pipeline::GRADIENT_V => &[0, 1, 0xff654321],
            pipeline::TRIANGLE => &[0, 0, 1, 0, 0, 1],
            _ => return None, // PROBE_INT remains first-PROBE work.
        };
        params[..inputs.len()].copy_from_slice(inputs);
        let (buffer, bytes) = if pipe == pipeline::COVERAGE_SPANS {
            (3, (WARM_SPANS.len() * 4) as u32)
        } else { (0, 0) };
        draw::encode(&mut words, pipe, 0xffffffff, if draw::sampled(pipe) { 2 } else { 0 }, buffer, bytes,
            &[draw::Instance { rect: [0, 0, 1, 1], params }]);
    }
    translate(words.as_slice(), &View(target, source, snapshot, spans)).ok()
}

/// K1 C1 fixed ALU workload: 16 full 512-square TRIANGLE profile draws.
/// The integer triangle shader evaluates its 96-bit edges for each fragment.
#[cfg(feature = "gt710_clock_probe")]
pub fn clock_frame(target: Target) -> Result<Frame, &'static str> {
    struct View(Target);
    impl Resolver for View {
        fn buffer_size(&self,_:u64)->Option<u64>{None}
        fn buffer_rect(&self,_:u64,_:u64)->[i32;4]{[0;4]}
        fn target(&self,h:u64)->Option<stream::TargetInfo>{(h==1).then_some(stream::TargetInfo {
            width:512,height:512,format:self.0.format,invalid:false})}
        fn source(&self,_:u64)->Option<stream::SourceInfo>{None}
    }
    impl Bindings for View {
        fn render_target(&self,h:u64)->Option<Target>{(h==1).then_some(self.0)}
        fn sampled_source(&self,_:u64)->Option<Target>{None}
    }
    let mut words=Vec::new();stream::encode::begin(&mut words,1);
    let mut params=[0;16];params[..6].copy_from_slice(&[0,0,512,0,0,512]);
    draw::encode(&mut words,pipeline::TRIANGLE,0xff123456,0,0,0,
        &[draw::Instance{rect:[0,0,512,512],params}]);
    let mut frame=translate(words.as_slice(),&View(target))?;
    // One draw is below the unchanged BGL work cap. The benchmark issues
    // 16 separate submissions, reusing its immutable validated inputs.
    if frame.chunks.len()!=1 {return Err("C1 expected one validated draw");}
    for _ in 1..16 {
        let c=&frame.chunks[0];
        let chunk=Chunk {command:c.command,target:c.target,words:c.words.clone(),
            relocations:c.relocations.clone(),source:c.source,copy:c.copy};
        frame.chunks.push(chunk);
    }
    Ok(frame)
}

/// Exact bootstrap allocation for the COPY parking payload, including the
/// committed catalogue and its per-draw constants, descriptors and vertices.
pub fn parking_bytes(target: Target, source: Target) -> Result<usize, &'static str> {
    warm_frame(target, source, None, None, pipeline::COPY)
        .ok_or("native parking frame refused")?
        .data.len().checked_sub(CODE).ok_or("native parking code extent")
}

/// Preserve the bootstrap channel's bounded upload contract for a catalogue
/// spanning several calls. Validate the complete address span before writing.
pub fn upload_parking(address: u64, data: &[u8],
    mut upload: impl FnMut(u64, &[u8]) -> Result<(), &'static str>,
) -> Result<(), &'static str> {
    let bytes = u64::try_from(data.len()).map_err(|_| "native parking upload extent")?;
    if bytes == 0 || address.checked_add(bytes).is_none() {
        return Err("native parking upload extent");
    }
    let mut next = address;
    for chunk in data.chunks(16384) {
        upload(next, chunk)?;
        next += chunk.len() as u64;
    }
    Ok(())
}

/// Build an inert channel state from the sampled encoder. Its
/// code, constants, vertices and descriptors occupy retained bootstrap VRAM;
/// its two images are separate retained pages. No draw launch is published.
/// VAB remains in the channel's separate 1 MiB scratch allocation.
pub fn parking_frame(target: Target, source: Target, address: u64, quarantine: u64)
    -> Result<(Vec<u8>, Vec<u32>), &'static str> {
    let mut frame = warm_frame(target, source, None, None, pipeline::COPY)
        .ok_or("native parking frame refused")?;
    // Runtime frames use this retained code base for both rendering and the
    // 25 integer probe variants. The admission programs share that same layout.
    for pipe in [PROBE_INTEGER, PROBE_TEXEL].into_iter()
        .chain(PROBE_ROUTINE_BASE..PROBE_ROUTINE_BASE + 25) {
        let (offset, header, code, _) = program(pipe);
        let at = CODE + offset;
        frame.data[at..at + 80].copy_from_slice(header);
        frame.data[at + 80..at + 80 + code.len()].copy_from_slice(code);
    }
    // CODE is a 128 KiB multiple; omit the obsolete per-frame VAB reservation.
    // channel_words redirects that one binding to channel-owned VAB storage.
    frame.relocate(address.checked_sub(CODE as u64).ok_or("native parking VA underflow")?)?;
    // Mesa nvc0_screen.c:920-925 serializes before changing live CB state.
    // The publisher appends this retained state after a bounded draw batch.
    // Drain that batch before replacing any of its input bindings.
    let mut parked = Vec::new();
    parked.try_reserve(2).map_err(|_| NO_MEMORY)?;
    parked.extend([0x20010044, 0]); // SERIALIZE / WAIT_FOR_IDLE (0x110)
    for chunk in &frame.chunks {
        let words = channel_words(&chunk.words, quarantine)?;
        let mut at = 0;
        while at < words.len() {
            let count = ((words[at] >> 16) & 0x1fff) as usize;
            // A197 BEGIN/END and the vertex array start/count packet launch work in
            // quad(). Keep the complete state, including both program offsets.
            if !matches!((words[at] & 0x1fff)*4, 0x1618 | 0xd74 | 0x1614) {
                parked.try_reserve(count+1).map_err(|_| NO_MEMORY)?;
                parked.extend_from_slice(&words[at..at+count+1]);
            }
            at += count+1;
        }
    }
    // Drain the parked state before the publisher unbinds CBs and releases.
    parked.try_reserve(2).map_err(|_| NO_MEMORY)?;
    parked.extend([0x20010044, 0]);
    Ok((copy_bytes(&frame.data[CODE..])?, parked))
}

#[cfg(target_os = "none")]
pub(crate) fn warm(target: Target, source: Target, snapshot: Option<Target>,
    spans: Option<Target>, pipe: u32) -> Option<Vec<super::adapter::EncodedDraw>> {
    let mut frame = warm_frame(target, source, snapshot, spans, pipe)?;
    frame.compact().ok()?;
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
    /// Routines with an actual PROBE draw, for completion-based readiness.
    pub probe_mask: u32,
    pub chunks: Vec<Chunk>,
    pub data: FrameBytes,
    removed_prefix: usize,
}
impl Frame {
    /// VAB and the immutable catalogue are channel-owned. Keep only frame
    /// inputs; publication supplies both retained bindings before any launch.
    /// Program offsets remain relative to the unchanged channel catalogue.
    pub fn compact(&mut self) -> Result<(), &'static str> {
        if self.removed_prefix == DATA_START { return Ok(()); }
        if self.data.len() < DATA_START { return Err("native frame prefix absent"); }
        for c in &mut self.chunks {
            let mut kept = 0;
            for n in 0..c.relocations.len() {
                let i = c.relocations[n];
                if i > 0 && matches!(c.words[i-1], 0x200305ef | 0x20020582) {
                    c.words[i] = 0;
                    c.words[i+1] = 0;
                    continue;
                }
                let offset = (u64::from(c.words[i]) << 32) | u64::from(c.words[i+1]);
                let offset = offset.checked_sub(DATA_START as u64).ok_or("native frame address in removed prefix")?;
                c.words[i] = (offset >> 32) as u32;
                c.words[i+1] = offset as u32;
                c.relocations[kept] = i;
                kept += 1;
            }
            c.relocations.truncate(kept);
        }
        #[cfg(target_os = "none")]
        self.data.remove_prefix(DATA_START);
        #[cfg(not(target_os = "none"))]
        { self.data.drain(..DATA_START); }
        if self.data.is_empty() { resize_data(&mut self.data, 1)?; }
        self.removed_prefix = DATA_START;
        Ok(())
    }
    /// Pack adjacent draws from this frame into bounded channel submissions.
    /// Each draw retains its state and relocations; only repeated channel VAB
    /// selection is omitted. Transfers remain separate and ordered.
    pub fn batch_draws(&mut self, limit: usize) -> Result<(), &'static str> {
        let mut batches: Vec<Chunk> = Vec::new();
        batches.try_reserve_exact(self.chunks.len()).map_err(|_| NO_MEMORY)?;
        for mut chunk in core::mem::take(&mut self.chunks) {
            let prefix = chunk.words.first() == Some(&0x200305ef);
            if let Some(last) = batches.last_mut().filter(|last|
                prefix && chunk.copy.is_none() && last.copy.is_none()
                    && last.target == chunk.target && last.source == chunk.source
                    && last.words.len() + chunk.words.len() - 4 <= limit) {
                let shift = last.words.len() - 4;
                last.relocations.try_reserve(chunk.relocations.len()).map_err(|_| NO_MEMORY)?;
                last.words.try_reserve(chunk.words.len() - 4).map_err(|_| NO_MEMORY)?;
                last.relocations.extend(chunk.relocations.drain(..).filter(|&i| i >= 4).map(|i| i + shift));
                last.words.extend_from_slice(&chunk.words[4..]);
            } else {
                batches.push(chunk);
            }
        }
        self.chunks = batches;
        Ok(())
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
    fn emit(&mut self, method: u32, data: &[u32]) -> Result<(), &'static str> {
        self.words.try_reserve(data.len() + 1).map_err(|_| NO_MEMORY)?;
        self.words
            .push(0x20000000 | ((data.len() as u32) << 16) | (method / 4));
        self.words.extend_from_slice(data);
        Ok(())
    }
    fn address(&mut self, method: u32, offset: usize, tail: &[u32]) -> Result<(), &'static str> {
        self.relocations.try_reserve(1).map_err(|_| NO_MEMORY)?;
        self.relocations.push(self.words.len() + 1);
        let mut data = Vec::new();
        data.try_reserve_exact(tail.len() + 2).map_err(|_| NO_MEMORY)?;
        data.extend_from_slice(&[(offset as u64 >> 32) as u32, offset as u32]);
        data.extend_from_slice(tail);
        self.emit(method, &data)
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
            cmd::PROBE if matches!(w.word(at + 2), 0..=24) => (),
            draw::COMMAND
                if matches!(
                    w.word(at + 2),
                    pipeline::COPY
                        | pipeline::COPY_SCALED
                        | pipeline::NEAREST_SCALED
                        | pipeline::BOX_FILTER
                        | pipeline::OVER
                        | pipeline::OVER_STRAIGHT
                        | pipeline::COVERAGE
                        | pipeline::BITMAP_TEXT
                        | pipeline::LINE
                        | pipeline::CIRCLE_FILL
                        | pipeline::CIRCLE_OUTLINE
                        | pipeline::ROUNDED_RECT
                        | pipeline::GRADIENT_V
                        | pipeline::TRIANGLE
                        | pipeline::COVERAGE_SPANS
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
    translate_inner(w, b, x, false)
}
/// Runtime draws use the catalogue retained by NativeChannel::publish_draw.
/// Build only their mutable inputs; bootstrap and diagnostic encoders still
/// produce a complete image through translate().
pub fn translate_resident(w: &(impl Words + ?Sized), b: &impl Bindings) -> Result<Frame, &'static str> {
    translate_resident_with_extras(w, b, Extras::default())
}
pub fn translate_resident_with_extras(w: &(impl Words + ?Sized), b: &impl Bindings, x: Extras) -> Result<Frame, &'static str> {
    translate_inner(w, b, x, true)
}
fn translate_inner(w: &(impl Words + ?Sized), b: &impl Bindings, x: Extras, resident: bool) -> Result<Frame, &'static str> {
    let Extras { mut corrupt, mut suppress, swap } = x;
    stream::validate(w, b).map_err(|_| "invalid BGL draw stream")?;
    if pipeline_refusal(w).is_some() { return Err("native pipeline not implemented"); }
    let mut f = Frame {
        probe_mask: 0,
        chunks: Vec::new(),
        data: new_data(if resident { 0 } else { DATA_START })?,
        removed_prefix: if resident { DATA_START } else { 0 },
    };
    if !resident {
        let segment = shaders::triangle_segment();
        f.data[CODE..CODE + segment.len()].copy_from_slice(&segment);
        for pipe in [
            pipeline::SOLID,
            pipeline::COPY,
            pipeline::COPY_SCALED,
            pipeline::NEAREST_SCALED,
            pipeline::BOX_FILTER,
            pipeline::OVER,
            pipeline::OVER_STRAIGHT,
            pipeline::COVERAGE,
            pipeline::BITMAP_TEXT,
        ] {
            let (offset, header, code, _) = program(pipe);
            let fs = CODE + offset;
            f.data[fs..fs + 80].copy_from_slice(header);
            f.data[fs + 80..fs + 80 + code.len()].copy_from_slice(code);
        }
        for &(offset, header, code, _) in shaders::SHAPE_PROGRAMS {
            let fs = CODE + offset;
            f.data[fs..fs + 80].copy_from_slice(header);
            f.data[fs + 80..fs + 80 + code.len()].copy_from_slice(code);
        }
    }
    let tables = span_tables(w)?;
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
                        corrupt_pixel(&mut f, t)?;
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
                let rectangles = (0..n as usize).filter_map(|i| {
                    let r = match code {
                        cmd::CLEAR => [0, 0, t.width as i32, t.height as i32],
                        cmd::DRAW => core::array::from_fn(|k| w.word(at + 5 + 4 * i + k) as i32),
                        _ => b.buffer_rect(
                            handle(w, at + 4),
                            u64::from(w.word(at + 6)) + 16 * i as u64,
                        ),
                    };
                    stream::clipped(r, t.width, t.height, scissor)
                });
                if let Some(clip) = clip {
                    let constants = [16, 8, 0, 24]
                        .map(|shift| gpu_abi::float::ratio_bits(colour >> shift & 255, 255));
                    quads(&mut f, t, rectangles, clip, &constants, pipeline::SOLID, None, None, None)?;
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
                f.chunks.try_reserve(1).map_err(|_| NO_MEMORY)?;
                f.chunks.push(Chunk { command, target: destination.id, source: Some(source.id),
                    words: Vec::new(), relocations: Vec::new(),
                    copy: rectangle.map(|r| (source.id, destination.id, r)) });
            }
            cmd::PROBE => {
                let t = target.ok_or("native target absent")?;
                let r = [4, 5, 6, 7].map(|k| w.word(at + k) as i32);
                let clip = stream::clipped([0, 0, t.width as i32, t.height as i32],
                    t.width, t.height, scissor);
                if let (Some(r), Some(clip)) = (stream::clipped(r, t.width, t.height, scissor), clip) {
                    let routine = w.word(at + 2);
                    let pipe = PROBE_ROUTINE_BASE + routine;
                    let (offset, header, code, _) = program(pipe);
                    let fs = CODE + offset;
                    if !resident {
                        f.data[fs..fs + 80].copy_from_slice(header);
                        f.data[fs + 80..fs + 80 + code.len()].copy_from_slice(code);
                    }
                    let mut constants = [0; 20];
                    for (i, value) in constants[..16].iter_mut().enumerate() {
                        *value = w.word(at + 8 + i);
                    }
                    constants[16] = w.word(at + 3);
                    constants[17] = routine;
                    quad(&mut f, t, r, clip, &constants, pipe, None, None, None)?;
                    f.probe_mask |= 1 << routine;
                }
            }
            draw::COMMAND if !draw::sampled(w.word(at + 2)) => {
                let t = target.ok_or("native target absent")?;
                let pipe = w.word(at + 2);
                let Some(clip) = stream::clipped([0, 0, t.width as i32, t.height as i32],
                    t.width, t.height, scissor) else {
                    at += w.word(at + 1) as usize;
                    command += 1;
                    continue;
                };
                let instance = |n| {
                    let base = at + draw::HEADER + n * draw::INSTANCE;
                    draw::Instance {
                        rect: core::array::from_fn(|k| w.word(base + k) as i32),
                        params: core::array::from_fn(|k| w.word(base + 4 + k)),
                    }
                };
                let count = w.word(at + 9) as usize;
                let data = if pipe == pipeline::COVERAGE_SPANS {
                    let data = b.span_texture().ok_or("native crossing texture absent")?;
                    texture_image_control(data)?;
                    let words = tables.last().map_or(0, |e| e.2 + e.1 / 4);
                    if data.format != format::R8G8B8A8 || !data.width.is_power_of_two()
                        || data.height != words.div_ceil(data.width)
                        || data.id == t.id || data.address == t.address {
                        return Err("invalid native crossing texture");
                    }
                    Some(data)
                } else { None };
                let scratch = if let Some(data) = data {
                    let union = (0..count).filter_map(|n| draw::source_clip(pipe, instance(n), None)
                        .and_then(|i| draw::clip(pipe, i, t.width, t.height, scissor)))
                        .fold(None, |u: Option<[u32; 4]>, r| Some(match u {
                            None => r,
                            Some(u) => [u[0].min(r[0]), u[1].min(r[1]), u[2].max(r[2]), u[3].max(r[3])],
                        }));
                    if let Some([x0, y0, x1, y1]) = union {
                        let scratch = b.blend_scratch(t.format).ok_or("native blend scratch absent")?;
                        texture_image_control(scratch)?;
                        if scratch.format != t.format || scratch.width < x1 || scratch.height < y1
                            || scratch.id == t.id || scratch.id == data.id
                            || scratch.address == t.address || scratch.address == data.address {
                            return Err("invalid native blend scratch");
                        }
                        f.chunks.try_reserve(1).map_err(|_| NO_MEMORY)?;
                f.chunks.push(Chunk { command, target: scratch.id, source: Some(t.id),
                            words: Vec::new(), relocations: Vec::new(),
                            copy: Some((t.id, scratch.id, [x0,y0,x1-x0,y1-y0,x0,y0])) });
                        Some(scratch)
                    } else { None }
                } else { None };
                for n in 0..count {
                    let i = instance(n);
                    if let Some(r) = draw::source_clip(pipe, i, None)
                        .and_then(|i| draw::clip(pipe, i, t.width, t.height, scissor)) {
                        let mut constants = [0; 32];
                        constants[..16].copy_from_slice(&i.params);
                        constants[26] = tables.iter().find(|e| e.0 == handle(w, at + 6)).map_or(0, |e| e.2);
                        constants[27] = data.map_or(gpu_abi::gk208::MAX_2D_SIZE.trailing_zeros(), |d| d.width.trailing_zeros());
                        let colour = w.word(at + 3);
                        if matches!(pipe, pipeline::GRADIENT_V | pipeline::COVERAGE_SPANS) { constants[20] = colour; }
                        else { constants[20..24].copy_from_slice(&[16, 8, 0, 24]
                            .map(|shift| gpu_abi::float::ratio_bits(colour >> shift & 255, 255))); }
                        quad(&mut f, t, r, clip, &constants, pipe, None, scratch, data)?;
                    }
                }
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
                let instance = |n| {
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
                    r.map(|r| (constants, r))
                };
                let scratch = if draw::blended(pipe) {
                    let union = (0..count).filter_map(|n| instance(n).map(|(_, r)| r))
                        .fold(None, |u: Option<[u32; 4]>, r| Some(match u {
                            None => r,
                            Some(u) => [u[0].min(r[0]), u[1].min(r[1]),
                                u[2].max(r[2]), u[3].max(r[3])],
                        }));
                    if let Some([x0, y0, x1, y1]) = union {
                        let scratch = b.blend_scratch(t.format).ok_or("native blend scratch absent")?;
                        texture_image_control(scratch)?;
                        if scratch.format != t.format || scratch.width < x1 || scratch.height < y1
                            || scratch.id == t.id || scratch.id == source.id
                            || scratch.address == t.address || scratch.address == source.address {
                            return Err("invalid native blend scratch");
                        }
                        // gpu.md 1.5 item 7: snapshot once per blended command,
                        // after prior draws, at the same pixel coordinates.
                        // Shared validation already refused intersecting writes.
                        f.chunks.try_reserve(1).map_err(|_| NO_MEMORY)?;
                f.chunks.push(Chunk { command, target: scratch.id, source: Some(t.id),
                            words: Vec::new(), relocations: Vec::new(),
                            copy: Some((t.id, scratch.id, [x0, y0, x1-x0, y1-y0, x0, y0])) });
                        Some(scratch)
                    } else { None }
                } else { None };
                for n in 0..count {
                    if let Some((constants, r)) = instance(n) {
                        quad(&mut f, t, r, clip, &constants, pipe, Some(source), scratch, None)?;
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
        if corrupt == Some(current_handle) { corrupt_pixel(&mut f, t)?; }
    }
    // Mesa nvc0_vbo.c:592-597 ends ordinary draws without SERIALIZE.
    // Keep it at a graphics-to-CE dependency (including blend snapshots),
    // rather than between rectangles. Native publication also drains before
    // parking/releasing this frame's bindings in parking_frame().
    for i in 1..f.chunks.len() {
        if f.chunks[i].copy.is_some() && !f.chunks[i - 1].words.is_empty() {
            f.chunks[i - 1].emit(0x110, &[0])?;
        }
    }
    if f.chunks.is_empty() {
        let t = target.ok_or("native target absent")?;
        f.chunks.try_reserve(1).map_err(|_| NO_MEMORY)?;
        f.chunks.push(Chunk {
            command,
            target: t.id,
            words: Vec::new(),
            relocations: Vec::new(),
            source: None,
            copy: None,
        });
    }
    if resident && f.data.is_empty() { resize_data(&mut f.data, 1)?; }
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
const PROBE_ROUTINE_BASE: u32 = 0x10000;
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
        pipeline::OVER => (
            shaders::PIPELINE_OVER_OFFSET,
            shaders::PIPELINE_OVER_HEADER,
            shaders::PIPELINE_OVER_CODE,
            shaders::PIPELINE_OVER_GPRS,
        ),
        pipeline::OVER_STRAIGHT => (
            shaders::PIPELINE_OVER_STRAIGHT_OFFSET,
            shaders::PIPELINE_OVER_STRAIGHT_HEADER,
            shaders::PIPELINE_OVER_STRAIGHT_CODE,
            shaders::PIPELINE_OVER_STRAIGHT_GPRS,
        ),
        pipeline::COVERAGE => (
            shaders::PIPELINE_COVERAGE_OFFSET,
            shaders::PIPELINE_COVERAGE_HEADER,
            shaders::PIPELINE_COVERAGE_CODE,
            shaders::PIPELINE_COVERAGE_GPRS,
        ),
        pipeline::BITMAP_TEXT => (
            shaders::PIPELINE_BITMAP_TEXT_OFFSET,
            shaders::PIPELINE_BITMAP_TEXT_HEADER,
            shaders::PIPELINE_BITMAP_TEXT_CODE,
            shaders::PIPELINE_BITMAP_TEXT_GPRS,
        ),
        pipeline::COVERAGE_SPANS => (shaders::PIPELINE_COVERAGE_SPANS_OFFSET, shaders::PIPELINE_COVERAGE_SPANS_HEADER, shaders::PIPELINE_COVERAGE_SPANS_CODE, shaders::PIPELINE_COVERAGE_SPANS_GPRS),
        pipeline::LINE => (shaders::PIPELINE_LINE_OFFSET, shaders::PIPELINE_LINE_HEADER, shaders::PIPELINE_LINE_CODE, shaders::PIPELINE_LINE_GPRS),
        pipeline::CIRCLE_FILL => (shaders::PIPELINE_CIRCLE_FILL_OFFSET, shaders::PIPELINE_CIRCLE_FILL_HEADER, shaders::PIPELINE_CIRCLE_FILL_CODE, shaders::PIPELINE_CIRCLE_FILL_GPRS),
        pipeline::CIRCLE_OUTLINE => (shaders::PIPELINE_CIRCLE_OUTLINE_OFFSET, shaders::PIPELINE_CIRCLE_OUTLINE_HEADER, shaders::PIPELINE_CIRCLE_OUTLINE_CODE, shaders::PIPELINE_CIRCLE_OUTLINE_GPRS),
        pipeline::ROUNDED_RECT => (shaders::PIPELINE_ROUNDED_RECT_OFFSET, shaders::PIPELINE_ROUNDED_RECT_HEADER, shaders::PIPELINE_ROUNDED_RECT_CODE, shaders::PIPELINE_ROUNDED_RECT_GPRS),
        pipeline::GRADIENT_V => (shaders::PIPELINE_GRADIENT_V_OFFSET, shaders::PIPELINE_GRADIENT_V_HEADER, shaders::PIPELINE_GRADIENT_V_CODE, shaders::PIPELINE_GRADIENT_V_GPRS),
        pipeline::TRIANGLE => (shaders::PIPELINE_TRIANGLE_OFFSET, shaders::PIPELINE_TRIANGLE_HEADER, shaders::PIPELINE_TRIANGLE_CODE, shaders::PIPELINE_TRIANGLE_GPRS),
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
        p if (PROBE_ROUTINE_BASE..PROBE_ROUTINE_BASE + 25).contains(&p) => {
            match p - PROBE_ROUTINE_BASE {
                0 => (shaders::PROBE_INT_00_OFFSET, shaders::PROBE_INT_00_HEADER, shaders::PROBE_INT_00_CODE, shaders::PROBE_INT_00_GPRS),
                1 => (shaders::PROBE_INT_01_OFFSET, shaders::PROBE_INT_01_HEADER, shaders::PROBE_INT_01_CODE, shaders::PROBE_INT_01_GPRS),
                2 => (shaders::PROBE_INT_02_OFFSET, shaders::PROBE_INT_02_HEADER, shaders::PROBE_INT_02_CODE, shaders::PROBE_INT_02_GPRS),
                3 => (shaders::PROBE_INT_03_OFFSET, shaders::PROBE_INT_03_HEADER, shaders::PROBE_INT_03_CODE, shaders::PROBE_INT_03_GPRS),
                4 => (shaders::PROBE_INT_04_OFFSET, shaders::PROBE_INT_04_HEADER, shaders::PROBE_INT_04_CODE, shaders::PROBE_INT_04_GPRS),
                5 => (shaders::PROBE_INT_05_OFFSET, shaders::PROBE_INT_05_HEADER, shaders::PROBE_INT_05_CODE, shaders::PROBE_INT_05_GPRS),
                6 => (shaders::PROBE_INT_06_OFFSET, shaders::PROBE_INT_06_HEADER, shaders::PROBE_INT_06_CODE, shaders::PROBE_INT_06_GPRS),
                7 => (shaders::PROBE_INT_07_OFFSET, shaders::PROBE_INT_07_HEADER, shaders::PROBE_INT_07_CODE, shaders::PROBE_INT_07_GPRS),
                8 => (shaders::PROBE_INT_08_OFFSET, shaders::PROBE_INT_08_HEADER, shaders::PROBE_INT_08_CODE, shaders::PROBE_INT_08_GPRS),
                9 => (shaders::PROBE_INT_09_OFFSET, shaders::PROBE_INT_09_HEADER, shaders::PROBE_INT_09_CODE, shaders::PROBE_INT_09_GPRS),
                10 => (shaders::PROBE_INT_10_OFFSET, shaders::PROBE_INT_10_HEADER, shaders::PROBE_INT_10_CODE, shaders::PROBE_INT_10_GPRS),
                11 => (shaders::PROBE_INT_11_OFFSET, shaders::PROBE_INT_11_HEADER, shaders::PROBE_INT_11_CODE, shaders::PROBE_INT_11_GPRS),
                12 => (shaders::PROBE_INT_12_OFFSET, shaders::PROBE_INT_12_HEADER, shaders::PROBE_INT_12_CODE, shaders::PROBE_INT_12_GPRS),
                13 => (shaders::PROBE_INT_13_OFFSET, shaders::PROBE_INT_13_HEADER, shaders::PROBE_INT_13_CODE, shaders::PROBE_INT_13_GPRS),
                14 => (shaders::PROBE_INT_14_OFFSET, shaders::PROBE_INT_14_HEADER, shaders::PROBE_INT_14_CODE, shaders::PROBE_INT_14_GPRS),
                15 => (shaders::PROBE_INT_15_OFFSET, shaders::PROBE_INT_15_HEADER, shaders::PROBE_INT_15_CODE, shaders::PROBE_INT_15_GPRS),
                16 => (shaders::PROBE_INT_16_OFFSET, shaders::PROBE_INT_16_HEADER, shaders::PROBE_INT_16_CODE, shaders::PROBE_INT_16_GPRS),
                17 => (shaders::PROBE_INT_17_OFFSET, shaders::PROBE_INT_17_HEADER, shaders::PROBE_INT_17_CODE, shaders::PROBE_INT_17_GPRS),
                20 => (shaders::PROBE_INT_20_OFFSET, shaders::PROBE_INT_20_HEADER, shaders::PROBE_INT_20_CODE, shaders::PROBE_INT_20_GPRS),
                21 => (shaders::PROBE_INT_21_OFFSET, shaders::PROBE_INT_21_HEADER, shaders::PROBE_INT_21_CODE, shaders::PROBE_INT_21_GPRS),
                22 => (shaders::PROBE_INT_22_OFFSET, shaders::PROBE_INT_22_HEADER, shaders::PROBE_INT_22_CODE, shaders::PROBE_INT_22_GPRS),
                23 => (shaders::PROBE_INT_23_OFFSET, shaders::PROBE_INT_23_HEADER, shaders::PROBE_INT_23_CODE, shaders::PROBE_INT_23_GPRS),
                24 => (shaders::PROBE_INT_24_OFFSET, shaders::PROBE_INT_24_HEADER, shaders::PROBE_INT_24_CODE, shaders::PROBE_INT_24_GPRS),
                18 => (shaders::PROBE_INT_18_OFFSET, shaders::PROBE_INT_18_HEADER, shaders::PROBE_INT_18_CODE, shaders::PROBE_INT_18_GPRS),
                19 => (shaders::PROBE_INT_19_OFFSET, shaders::PROBE_INT_19_HEADER, shaders::PROBE_INT_19_CODE, shaders::PROBE_INT_19_GPRS),
                _ => unreachable!(),
            }
        }
        _ => unreachable!(),
    }
}
/// Admission-only programs reuse the same u4/u5 quad, raster, TIC/TSC and
/// relocation builders. They are not new public catalogue pipeline IDs.
pub fn probe_frame(t: Target, source: Option<Target>) -> Result<Frame, &'static str> {
    use super::caps::{PROBE_HEIGHT as H, PROBE_WIDTH as W};
    let pipe = if source.is_some() {
        PROBE_TEXEL
    } else {
        PROBE_INTEGER
    };
    let mut f = Frame {
        probe_mask: 0,
        chunks: Vec::new(),
        data: new_data(DATA_START)?,
        removed_prefix: 0,
    };
    let segment = shaders::triangle_segment();
    f.data[CODE..CODE + segment.len()].copy_from_slice(&segment);
    let (offset, header, code, _) = program(pipe);
    resize_data(&mut f.data, DATA_START.max(CODE + offset + 80 + code.len()))?;
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
            None,
            None,
        )?;
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
                None,
                None,
            )?;
        }
    }
    Ok(f)
}
/// Point 22 uses the solid shader at (0,0), outside the suite's draw/scissor.
fn corrupt_pixel(f: &mut Frame, t: Target) -> Result<(), &'static str> {
    solid(f, t, [0, 0, 1, 1], [0, 0, 1, 1], !0x3c5a_a5c3)
}
fn solid(f: &mut Frame, t: Target, r: [u32; 4], scissor: [u32; 4], colour: u32) -> Result<(), &'static str> {
    let constants =
        [16, 8, 0, 24].map(|shift| gpu_abi::float::ratio_bits(colour >> shift & 255, 255));
    quad(f, t, r, scissor, &constants, pipeline::SOLID, None, None, None)
}
fn quad(
    f: &mut Frame,
    t: Target,
    r: [u32; 4],
    scissor: [u32; 4],
    constants: &[u32],
    pipe: u32,
    source: Option<Target>,
    scratch: Option<Target>,
    data: Option<Target>,
) -> Result<(), &'static str> {
    quads(f, t, core::iter::once(r), scissor, constants, pipe, source, scratch, data)
}
/// One BGL command's rectangles share uniforms and state. Preserve rectangle
/// order and six vertices per rectangle; no instancing or shader change.
fn quads(
    f: &mut Frame,
    t: Target,
    rectangles: impl Iterator<Item = [u32; 4]> + Clone,
    scissor: [u32; 4],
    constants: &[u32],
    pipe: u32,
    source: Option<Target>,
    scratch: Option<Target>,
    data: Option<Target>,
) -> Result<(), &'static str> {
    let count = rectangles.clone().count();
    if count == 0 { return Ok(()); }
    let vertices = count.checked_mul(6).ok_or("native vertex count overflow")?;
    let vertex_bytes = vertices.checked_mul(32).ok_or("native vertex extent overflow")?;
    let vertices = u32::try_from(vertices).map_err(|_| "native vertex count overflow")?;
    let sampled = source.is_some() || data.is_some();
    let uniform = f.data.len().next_multiple_of(256);
    let vertex = uniform + 256;
    let end = vertex.checked_add(vertex_bytes).ok_or("native vertex extent overflow")?;
    let tic = end.next_multiple_of(256);
    let tsc = tic + 256;
    let handles = tsc + 256;
    resize_data(&mut f.data,
        if sampled {
            handles + 256
        } else {
            tic
        },
    )?;
    for (k, v) in constants.iter().enumerate() {
        f.data[uniform + k * 4..uniform + k * 4 + 4].copy_from_slice(&v.to_le_bytes());
    }
    if sampled {
        // MIT cla197tex.h:509-536,547-574: clamp U/V/P to edge, no sRGB or
        // anisotropy; POINT mag/min, MIP_NONE. Integer texel fetches need no
        // hardware filtering, including the BOX_FILTER shader's explicit sum.
        // Mesa nvc0_tex.c:743-751 uploads a full eight-word TSC; unlike its
        // generic TSC0, this exact profile keeps sRGB conversion disabled.
        let sampler = [2 | (2 << 3) | (2 << 6), 1 | (1 << 4) | (1 << 6), 0, 0, 0, 0, 0, 0];
        // Validation above established this descriptor before any draw chunks.
        for (slot, texture) in [source, scratch, data].into_iter().enumerate() {
            let Some(texture) = texture else { continue; };
            for (offset, words) in [
                (tic + slot * 32, texture_image_control(texture).unwrap()),
                (tsc + slot * 32, sampler),
            ] {
                for (k, v) in words.iter().enumerate() {
                    f.data[offset + k * 4..offset + k * 4 + 4].copy_from_slice(&v.to_le_bytes());
                }
            }
            // Mesa nvc0_tex.c:708: TIC | TSC << 20. The compiler selects
            // CB15[slot] (texBindBase=0); u1's alpha assets use slots 0 and 1.
            let handle = slot as u32 | (slot as u32) << 20;
            f.data[handles + slot * 4..handles + slot * 4 + 4].copy_from_slice(&handle.to_le_bytes());
        }
    }
    for (rectangle, [x0, y0, x1, y1]) in rectangles.enumerate() {
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
                let at = vertex + rectangle * 192 + i * 32 + k * 4;
                f.data[at..at + 4]
                    .copy_from_slice(&v.to_le_bytes());
            }
        }
    }
    let mut c = Chunk {
        command: 0,
        target: t.id,
        words: Vec::new(),
        relocations: Vec::new(),
        source: source.or(data).map(|s| s.id),
        copy: None,
    };
    // IDXFETCH quarantine: 1 MiB, 128 KiB aligned (Mesa screen:1274-1283).
    if f.removed_prefix == DATA_START { c.emit(0x17bc, &[0, 0, 3])?; }
    else { c.address(0x17bc, 0, &[3])?; }
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
    )?;
    c.emit(0x121c, &[1])?;
    for method in [0x1538, 0x12cc, 0x12e8, 0x1380, 0x1594, 0x19bc] {
        c.emit(method, &[0])?;
    }
    let halfw = gpu_abi::float::scaled_int_bits(i64::from(t.width), 1);
    let halfh = gpu_abi::float::scaled_int_bits(i64::from(t.height), 1);
    c.emit(0xa00, &[halfw, halfh, 0x3f800000, halfw, halfh, 0])?;
    c.emit(0xc00, &[t.width << 16, t.height << 16, 0, 0x3f800000])?;
    let [sx0, sy0, sx1, sy1] = scissor;
    c.emit(0xe00, &[1, sx0 | sx1 << 16, sy0 | sy1 << 16])?;
    c.emit(0xff4, &[t.width << 16, t.height << 16])?;
    c.emit(0xd00, &[t.width << 16, t.height << 16])?;
    c.emit(0x194c, &[1])?;
    c.emit(0x1950, &[0])?;
    c.emit(0x0df8, &[0, 0])?;
    c.emit(0x13ac, &[0])?;
    c.emit(0x1924, &[0])?;
    c.emit(0x192c, &[1])?;
    c.emit(0x193c, &[2])?;
    c.emit(0x1510, &[0])?;
    c.emit(0x1918, &[0])?;
    c.emit(0x0dac, &[0x1b02, 0x1b02])?;
    // Exact profile is frame-owned, including after a context switch.
    // MIT cla197.h:1813-1816; Mesa nvc0_state.c:143-146: common blend state.
    c.emit(0x12e4, &[0])?;
    // MIT cla197.h:2119-2122; Mesa nvc0_state.c:130-146: SET_BLEND(0)=FALSE.
    // Use the direct method; no Mesa MME blend-enable macro is installed.
    c.emit(0x1360, &[0])?;
    c.emit(0x135c, &[0])?;
    c.emit(0x19c4, &[0])?;
    c.emit(0x0f90, &[1])?;
    c.emit(0x1a00, &[0x1111])?;
    // MIT cla197.h:1807-1811: 1x1 alpha-to-coverage dither footprint.
    // This is a footprint selector, not a color-dither enable. Disable its
    // alpha-to-coverage path explicitly below rather than inherit it.
    c.emit(0x12e0, &[0])?;
    // MIT cla197.h:2381-2384; Mesa nvc0_state.c:235: anti-alias disabled.
    c.emit(0x1534, &[0])?;
    // MIT cla197.h:2551-2553: MODE_1X1 (one sample).
    c.emit(0x15d0, &[0])?;
    // MIT cla197.h:2389-2395; Mesa nvc0_state.c:182-189: neither alpha
    // to coverage (including dithering) nor alpha to one may alter pixels.
    c.emit(0x153c, &[0])?;
    // MIT envytools rnndb graph/gf100_3d.xml:217-222: one pass, sample shading disabled.
    c.emit(0x0754, &[1])?;
    c.emit(0x0fbc, &[0xffff; 4])?;
    // MIT cla197.h:2521-2524: SRGB_WRITE=FALSE. Sampled images likewise
    // omit the TIC sRGB bit (Mesa nvc0_tex.c:335-338), and TSCs omit bit 13.
    c.emit(0x15b8, &[0])?;
    c.emit(0x19e0, &[0])?;
    c.emit(0x12d4, &[0x1d01])?;
    c.emit(0x12ec, &[0])?;
    c.emit(0x037c, &[1])?;
    c.emit(0x0fac, &[1])?;
    c.emit(0x1558, &[1])?;
    c.emit(0x1644, &[0])?;
    c.emit(0x15e4, &[1])?;
    // Mesa screen:688-761, Kepler branch; no MME macros are installed or used.
    c.emit(0x10cc, &[0xff])?;
    c.emit(0x10e0, &[0xff, 0xff])?;
    c.emit(0x10ec, &[0xff, 0xff])?;
    c.emit(0x074c, &[0x3f])?;
    c.emit(0x16a8, &[0x30003])?;
    c.emit(0x1794, &[0x20002])?;
    c.emit(0x12ac, &[0])?;
    c.emit(0x0218, &[0x10])?;
    c.emit(0x10fc, &[0x10])?;
    c.emit(0x1290, &[0x10])?;
    c.emit(0x12d8, &[0x10, 0x10])?;
    c.emit(0x1140, &[0x10])?;
    c.emit(0x1610, &[0xe])?;
    c.emit(0x161c, &[0])?;
    c.emit(0x030c, &[0])?;
    c.emit(0x0300, &[3])?;
    c.emit(0x02d0, &[0x3fffff])?;
    c.emit(0x0fdc, &[1])?;
    c.emit(0x19c0, &[1])?;
    c.emit(0x075c, &[3])?;
    c.emit(0x07fc, &[1])?;

    if f.removed_prefix == DATA_START { c.emit(0x1608, &[0, 0])?; }
    else { c.address(0x1608, CODE, &[])?; }
    c.emit(0x021c, &[0x1011])?; // invalidate instruction cache, all graphics stages
    for slot in [0, 2, 3, 4] {
        c.emit(0x2000 + slot * 64, &[slot << 4])?;
    }
    c.emit(0x2040, &[0x11, shaders::VERTEX_OFFSET as u32])?;
    c.emit(0x204c, &[shaders::VERTEX_GPRS])?;
    let (offset, _, _, gprs) = program(pipe);
    c.emit(0x2140, &[0x51, offset as u32])?;
    c.emit(0x214c, &[gprs])?;
    // MIT Mesa nvc0_screen.c:933-939: CB_SIZE, CB_ADDRESS_HIGH/LOW,
    // CB_BIND(stage). One immutable block supplies this command's colour.
    c.emit(0x2380, &[256, 0, uniform as u32])?;
    c.relocations.try_reserve(1).map_err(|_| NO_MEMORY)?;
    c.relocations.push(c.words.len() - 2);
    c.emit(0x2490, &[1])?;
    if sampled {
        let maximum_index = if data.is_some() { 2 } else { u32::from(scratch.is_some()) };
        c.address(0x1574, tic, &[maximum_index])?;
        c.address(0x155c, tsc, &[maximum_index])?;
        c.emit(0x1334, &[0])?;
        c.emit(0x1330, &[0])?;
        c.emit(0x1338, &[0])?;
        c.emit(0x2608, &[15])?;
        c.emit(0x2380, &[256, 0, handles as u32])?;
        c.relocations.try_reserve(1).map_err(|_| NO_MEMORY)?;
    c.relocations.push(c.words.len() - 2);
        c.emit(0x2490, &[0xf1])?;
    }
    c.emit(0x1930, &[1])?;
    c.emit(0x1880, &[0])?;
    // MIT Mesa nvc0_vbo.c:358-376: VERTEX_ARRAY_FETCH and the inclusive
    // VERTEX_ARRAY_LIMIT_HIGH/LOW bound the complete 32-byte-stride buffer.
    c.emit(0x1c00, &[0x1020, 0, vertex as u32, 0])?;
    c.relocations.try_reserve(1).map_err(|_| NO_MEMORY)?;
    c.relocations.push(c.words.len() - 3);
    c.address(0x1f00, end - 1, &[])?;
    c.emit(0x1160, &[0x38200000, 0x38200800])?;
    c.emit(0x142c, &[0])?;
    c.emit(0x21c, &[1])?;
    // MIT Mesa nvc0_vbo.c:592-597: VERTEX_BEGIN_GL,
    // VERTEX_BUFFER_FIRST/COUNT, VERTEX_END_GL. A single triangle-list draw
    // consumes 6 * count vertices; ordinary draws need no trailing SERIALIZE.
    c.emit(0x1618, &[4])?;
    c.emit(0xd74, &[0, vertices])?;
    c.emit(0x1614, &[0])?;
    f.chunks.try_reserve(1).map_err(|_| NO_MEMORY)?;
    f.chunks.push(c);
    Ok(())
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
    let mut pages = Vec::new();
    pages.try_reserve_exact(backing.count()).map_err(|_| NO_MEMORY)?;
    pages.extend((0..backing.count()).map(|i| backing.page_addr(i)));
    let mut regions = Vec::new();
    regions.try_reserve(2).map_err(|_| NO_MEMORY)?;
    let mut draws = Vec::new();
    draws.try_reserve_exact(frame.chunks.len()).map_err(|_| NO_MEMORY)?;
    if frame.removed_prefix == 0 {
        regions.push(super::resource::FrameRegion {
            start: CODE as u64, end: DATA_START as u64, name: "code", command: 0,
        });
        regions.push(super::resource::FrameRegion { start: 0, end: CODE as u64,
            name: "frame-reserved", command: 0 });
    }
    for c in &frame.chunks {
        let mut at = 0;
        let mut vertex_start = None;
        while at < c.words.len() {
            let count = ((c.words[at] >> 16) & 0x1fff) as usize;
            let method = (c.words[at] & 0x1fff) * 4;
            let region = match method {
                0x2380 => Some((u64::from(c.words[at+3]), 256, "constants")),
                0x1c00 => {
                    vertex_start = Some(u64::from(c.words[at+2]) << 32 | u64::from(c.words[at+3]));
                    None
                }
                0x1f00 => {
                    let start = vertex_start.take().ok_or("native vertex start absent")?;
                    let end = (u64::from(c.words[at+1]) << 32 | u64::from(c.words[at+2]))
                        .checked_add(1).ok_or("native vertex extent overflow")?;
                    let bytes = end.checked_sub(start).ok_or("native vertex limit before start")?;
                    Some((start, bytes, "vertex"))
                }
                0x1574 => Some((u64::from(c.words[at+2]), (u64::from(c.words[at+3])+1)*32, "texture-descriptor")),
                0x155c => Some((u64::from(c.words[at+2]), (u64::from(c.words[at+3])+1)*32, "sampler-descriptor")),
                _ => None,
            };
            if let Some((start, bytes, name)) = region {
                regions.try_reserve(1).map_err(|_| NO_MEMORY)?;
                regions.push(super::resource::FrameRegion { start, end: start+bytes, name, command: c.command });
            }
            at += count+1;
        }
    }
    let (id, address, holds, transfers) = super::ce::with_resources(|r, a, io| {
        let mut holds = Vec::new();
        let mut transfers = Vec::new();
        holds.try_reserve_exact(frame.chunks.len().checked_mul(4).and_then(|n|n.checked_add(1)).ok_or(NO_MEMORY)?)
            .map_err(|_| NO_MEMORY)?;
        transfers.try_reserve_exact(frame.chunks.len()).map_err(|_| NO_MEMORY)?;
        for c in &frame.chunks {
            holds.push(r.hold(c.target)?);
            if let Some(source) = c.source {
                holds.push(r.hold(source)?);
            }
            transfers.push(if let Some((source, destination, rectangle)) = c.copy {
                let (plan, retained) = super::ce::readback::region_plan(r, a, io, source, destination, rectangle)?;
                holds.try_reserve(retained.len()).map_err(|_| NO_MEMORY)?;
                holds.extend(retained);
                Some(plan)
            } else { None });
        }
        let id = r.draw_buffer(a, io, backing.bytes() as u32)?;
        backing.identify("native-frame", id, r.channel() as u32);
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
    draws.extend(frame
        .chunks
        .into_iter()
        .zip(transfers)
        .map(|(c, transfer)| super::adapter::EncodedDraw {
            target: c.target,
            command: c.command,
            words: c.words,
            transfer,
            _storage: storage.clone(),
        }));
    Ok(draws)
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

/// Native device-error control: validate logical context residency before the
/// worker assigns a sequence and publishes the A197 unmapped-address write.
#[cfg(target_os = "none")]
pub(crate) fn undefined_command(tag: crate::gpu::dev::Tag, ctx: u32, fence: u64)
    -> Option<super::adapter::Request> {
    super::resource::context_id(ctx)?;
    super::adapter::Request::undefined_command(tag, ctx, fence)
}

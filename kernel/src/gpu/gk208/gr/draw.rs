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

//! Pure A197 diagnostic-triangle encoder. The caller has bound A197 on
//! `channel::SUBCHANNEL` and set CODE_ADDRESS with the committed 7.1 assets.
//! This unit constructs commands and vertex input; the acceptance path owns
//! uploads, submission, read-back, presentation and the hardware verdict.
//!
//! Hardware state adapted from MIT open-gpu-doc cla197.h at
//! 9fdf5c4062007929d9f4e6cbad9c9771fe61b880 and MIT Mesa 24.3.4
//! 769e51468b49b2a42f0a0eaf71cf9eed5ff4e5de nvc0_{screen,state,vbo,surface}.c.
//! The literal host check cites each packet's source lines.
use alloc::vec::Vec;
use super::{channel, shaders};

/// Three interleaved float4 clip positions and float4 RGBA colours (32 bytes
/// per vertex), bit-identical to docs/gui/provenance/gt710-triangle.json.
/// No arithmetic or pixel generation takes place here.
pub const VERTEX_WORDS: [u32; 24] = [
    0xbf7e0000, 0xbf7e0000, 0, 0x3f800000, 0x3f600000, 0x3e000000, 0x3ec00000, 0x3f800000,
    0x3c000000, 0xbf7e0000, 0, 0x3f800000, 0x3ec00000, 0x3e000000, 0x3f200000, 0x3f800000,
    0xbf7e0000, 0x3c000000, 0, 0x3f800000, 0, 0x3f800000, 0x3ec00000, 0x3f800000,
];

#[derive(Clone, Copy)]
pub struct Draw {
    pub target_va: u64,
    pub pitch: u32,
    pub width: u32,
    pub height: u32,
    pub vertex_va: u64,
    /// SPH offsets relative to the already bound program region, including
    /// the 7.4 stage-control offsets. Register counts are the committed metadata.
    pub vertex_program: u32,
    pub fragment_program: u32,
    /// Three words: nonce, epoch low, epoch high. The nonce is released last.
    pub report_va: u64,
    pub epoch: u64,
    pub nonce: u32,
}

fn span(address: u64, bytes: u64, alignment: u64) -> bool {
    address != 0 && address & (alignment - 1) == 0
        && address.checked_add(bytes).is_some_and(|end| end <= 1 << 40)
}

/// Encode one TRIANGLES draw for the oracle's 64x64 target. Padded pitches
/// are permitted; resizing this diagnostic requires a new geometry oracle.
/// Program offsets name the SPH (80 bytes before aligned instructions).
/// This function performs no device access and does not bind another object.
pub fn commands(r: Draw) -> Result<Vec<u32>, &'static str> {
    if r.width != 64 || r.height != 64 || r.pitch < 256 || r.pitch & 255 != 0
        || r.pitch >= 1 << 28
        || !span(r.target_va, u64::from(r.pitch) * u64::from(r.height), 256)
        || !span(r.vertex_va, 96, 4) || !span(r.report_va, 12, 4)
        || r.epoch == 0 || r.nonce == 0
    {
        return Err("invalid diagnostic draw resources");
    }
    for offset in [r.vertex_program, r.fragment_program] {
        if offset.checked_add(80).is_none_or(|code| code & 127 != 0) {
            return Err("invalid diagnostic SPH offset");
        }
    }
    let mut words = Vec::new();
    let mut emit = |method: u32, data: &[u32]| {
        words.push(0x20000000 | ((data.len() as u32) << 16)
            | (channel::SUBCHANNEL << 13) | (method / 4));
        words.extend_from_slice(data);
    };
    // Pitch-linear RT0: WIDTH is bytes, not pixels (Mesa surface:590-599).
    // A8R8G8B8 (0xcf) gives B,G,R,A bytes in little-endian memory.
    emit(0x0800, &[(r.target_va >> 32) as u32, r.target_va as u32,
        r.pitch, r.height, 0xcf, 0x1000, 1, 0, 0]);
    emit(0x121c, &[1]);
    for method in [0x1538, 0x12cc, 0x12e8, 0x1380, 0x1594, 0x19bc] {
        emit(method, &[0]);
    }
    // JSON viewport/scissor/window: top-left, half-integer samples.
    emit(0x0a00, &[0x42000000, 0x42000000, 0x3f800000, 0x42000000, 0x42000000, 0]);
    emit(0x0c00, &[r.width << 16, r.height << 16, 0, 0x3f800000]);
    emit(0x0e00, &[1, r.width << 16, r.height << 16]);
    emit(0x0ff4, &[r.width << 16, r.height << 16]);
    emit(0x0d00, &[r.width << 16, r.height << 16]);
    emit(0x194c, &[1]);
    emit(0x1950, &[0]);
    emit(0x0df8, &[0, 0]);
    emit(0x13ac, &[0]);
    emit(0x1924, &[0]);
    emit(0x192c, &[1]);
    emit(0x193c, &[2]);
    emit(0x1510, &[0]);
    emit(0x1918, &[0]);
    emit(0x0dac, &[0x1b02, 0x1b02]);
    emit(0x1360, &[0]);
    emit(0x135c, &[0]);
    emit(0x19c4, &[0]);
    emit(0x0f90, &[1]);
    emit(0x1a00, &[0x1111]);
    for method in [0x1534, 0x15d0, 0x153c] { emit(method, &[0]); }
    // MIT envytools rnndb graph/gf100_3d.xml:217-222: one pass, sample shading disabled.
    emit(0x0754, &[1]);
    emit(0x0fbc, &[0xffff; 4]);
    emit(0x15b8, &[0]);
    emit(0x19e0, &[0]);
    emit(0x12d4, &[0x1d01]);
    emit(0x12ec, &[0]);
    emit(0x037c, &[1]);
    emit(0x0fac, &[1]);
    emit(0x1558, &[1]);
    emit(0x1644, &[0]);
    emit(0x15e4, &[1]);
    // Mesa screen:688-761, Kepler branch; no MME macros are installed or used.
    emit(0x10cc, &[0xff]);
    emit(0x10e0, &[0xff, 0xff]);
    emit(0x10ec, &[0xff, 0xff]);
    emit(0x074c, &[0x3f]);
    emit(0x16a8, &[0x30003]);
    emit(0x1794, &[0x20002]);
    emit(0x12ac, &[0]);
    emit(0x0218, &[0x10]);
    emit(0x10fc, &[0x10]);
    emit(0x1290, &[0x10]);
    emit(0x12d8, &[0x10, 0x10]);
    emit(0x1140, &[0x10]);
    emit(0x1610, &[0xe]);
    emit(0x161c, &[0]);
    emit(0x030c, &[0]);
    emit(0x0300, &[3]);
    emit(0x02d0, &[0x3fffff]);
    emit(0x0fdc, &[1]);
    emit(0x19c0, &[1]);
    emit(0x075c, &[3]);
    emit(0x07fc, &[1]);
    // Disable unused stages explicitly; enable the real vertex/fragment pair.
    for slot in [0, 2, 3, 4] { emit(0x2000 + slot * 64, &[slot << 4]); }
    emit(0x2040, &[0x11, r.vertex_program]);
    emit(0x204c, &[shaders::VERTEX_GPRS]);
    emit(0x2140, &[0x51, r.fragment_program]);
    emit(0x214c, &[shaders::FRAGMENT_GPRS]);
    // Interleaved float4 fetch at a[0x80]/a[0x90], inclusive stream limit.
    emit(0x1880, &[0]);
    emit(0x1c00, &[0x1020, (r.vertex_va >> 32) as u32, r.vertex_va as u32, 0]);
    let last = r.vertex_va + 95;
    emit(0x1f00, &[(last >> 32) as u32, last as u32]);
    emit(0x1160, &[0x38200000, 0x38200800]);
    emit(0x142c, &[0]);
    emit(0x021c, &[1]);
    emit(0x1618, &[4]);
    emit(0x0d74, &[0, 3]);
    emit(0x1614, &[0]);
    emit(0x0110, &[0]);
    // cla197:3274-3366: pipeline-end location 0xf, after writes, one word, flush enabled.
    // No host SET_REFERENCE can substitute for these engine releases.
    for (offset, value) in [(4, r.epoch as u32), (8, (r.epoch >> 32) as u32), (0, r.nonce)] {
        let target = r.report_va + offset;
        emit(0x1b00, &[(target >> 32) as u32, target as u32, value, 0x1000f010]);
    }
    Ok(words)
}

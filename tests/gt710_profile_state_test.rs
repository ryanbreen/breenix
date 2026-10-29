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

// References (G1, file-local MIT grants retained above):
// open-gpu-doc 9fdf5c4062007929d9f4e6cbad9c9771fe61b880:
// cla197.h:1807-1816,2119-2122,2381-2395,2521-2524,2551-2553;
// cla197tex.h:533-574. Mesa mesa-24.3.4:
// nvc0_state.c:130-146,182-189,235; nvc0_tex.c:335-338,743-751.
extern crate alloc;
#[path = "../kernel/src/gpu/gk208/caps.rs"]
pub mod caps;
#[path = "../kernel/src/gpu/gk208/gr/shaders.rs"]
pub mod shaders;
#[path = "../kernel/src/gpu/gk208/encode.rs"]
pub mod encode;
use gpu_abi::{draw, format, pipeline, stream::{self, Resolver, SourceInfo, TargetInfo}};

struct View(u32);
impl Resolver for View {
    fn target(&self, h: u64) -> Option<TargetInfo> {
        (h == 1).then_some(TargetInfo {
            width: 64, height: 64, format: format::B8G8R8A8, invalid: false,
        })
    }
    fn source(&self, h: u64) -> Option<SourceInfo> {
        (h == 2).then_some(SourceInfo {
            width: 32, height: 32, format: self.0, invalid: false,
        })
    }
    fn buffer_size(&self, _: u64) -> Option<u64> { None }
    fn buffer_rect(&self, _: u64, _: u64) -> [i32; 4] { panic!("no buffer draw") }
}
impl encode::Bindings for View {
    fn render_target(&self, h: u64) -> Option<encode::Target> {
        self.target(h).map(|t| encode::Target {
            id: 19, address: 0x2_0000_0000, width: t.width, height: t.height,
            format: t.format, tile_mode: 0x30,
        })
    }
    fn sampled_source(&self, h: u64) -> Option<encode::Target> {
        self.source(h).map(|s| encode::Target {
            id: 23, address: 0x4_0000_0000, width: s.width, height: s.height,
            format: s.format, tile_mode: 0x10,
        })
    }
}
fn packets(words: &[u32]) -> Vec<(u32, &[u32])> {
    let mut result = Vec::new();
    let mut at = 0;
    while at < words.len() {
        let h = words[at];
        let n = ((h >> 16) & 0x1fff) as usize;
        assert_eq!(h & 0xe000e000, 0x20000000);
        assert!(n > 0 && at + n < words.len());
        result.push(((h & 0x1fff) * 4, &words[at + 1..at + 1 + n]));
        at += n + 1;
    }
    result
}
fn word(data: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(data[at..at + 4].try_into().unwrap())
}
fn check(pipe: u32) {
    for fmt in [format::B8G8R8A8, format::R8G8B8A8, format::R8] {
        let mut input = Vec::new();
        stream::encode::begin(&mut input, 1);
        for x in [0, 16] {
            if pipe == pipeline::SOLID {
                stream::encode::draw(&mut input, pipe, 0x81234567, &[[x, 0, x + 8, 8]]);
            } else if pipe == pipeline::COPY {
                stream::encode::draw_sampled(&mut input, pipe, 2, &[[x, 0, x + 8, 8, 0, 0]]);
            } else {
                let mut params = [0; 16];
                params[..8].copy_from_slice(&[x as u32, 0, 8, 8, 0, 0, 32, 32]);
                draw::encode(&mut input, pipe, 0xffffffff, 2, 0, 0,
                    &[draw::Instance { rect: [x, 0, x + 8, 8], params }]);
            }
        }
        let frame = encode::translate(input.as_slice(), &View(fmt)).unwrap();
        assert_eq!(frame.chunks.len(), 2);
        let mut samplers = 0;
        for chunk in &frame.chunks {
            let p = packets(&chunk.words);
            let first_draw = p.iter().position(|(m, _)| *m == 0x1618).unwrap();
            assert!(p.iter().any(|(m, v)| *m == 0xd74 && *v == [0, 6]));
            // SET_BLEND(0)=FALSE, common blend state, 1x1 coverage footprint;
            // anti-alias/alpha-to-coverage/alpha-to-one disabled, MODE_1X1,
            // linear RT writes. 0x12e0 selects the footprint, not an enable bit.
            for (method, expected) in [
                (0x1360, 0), (0x12e4, 0), (0x12e0, 0),
                (0x1534, 0), (0x153c, 0), (0x15d0, 0), (0x15b8, 0),
            ] {
                let writes: Vec<_> = p.iter().enumerate().flat_map(|(i, (m, v))|
                    v.iter().enumerate().filter_map(move |(j, &value)|
                        (*m + j as u32 * 4 == method).then_some((i, value))))
                    .collect();
                assert!(writes.iter().any(|&(i, v)| i < first_draw && v == expected),
                    "pipeline {pipe}: missing {method:#x}={expected:#x} before first draw");
                assert!(writes.iter().all(|&(_, v)| v == expected),
                    "pipeline {pipe}: profile state overwritten at {method:#x}");
            }
            for (i, (method, values)) in p.iter().enumerate() {
                if *method == 0x155c {
                    assert!(i < first_draw);
                    assert_eq!(values.len(), 3);
                    assert_eq!(values[0], 0); // unrelocated frame-relative address
                    for index in 0..=values[2] as usize {
                        let at = values[1] as usize + 32 * index;
                        let a = word(&frame.data, at);
                        let b = word(&frame.data, at + 4);
                        // cla197tex.h:533-574: no sRGB, anisotropy or font filter;
                        // POINT min/mag and MIP_NONE. No trilinear/cube filtering.
                        assert_eq!(a & ((1 << 13) | (0x1ff << 14)), 0);
                        assert_eq!(b, 1 | (1 << 4) | (1 << 6));
                        samplers += 1;
                    }
                }
                if *method == 0x1574 {
                    assert!(i < first_draw);
                    // Mesa nvc0_tex.c:335-338: TIC sRGB conversion stays clear.
                    for index in 0..=values[2] as usize {
                        assert_eq!(word(&frame.data, values[1] as usize + 32 * index + 8)
                            & (1 << 10), 0);
                    }
                }
            }
        }
        assert_eq!(samplers, if pipe == pipeline::SOLID { 0 } else { 2 });
    }
}
#[test]
fn solid_sets_exact_profile_before_drawing() { check(pipeline::SOLID); }
#[test]
fn copy_sets_exact_profile_before_drawing() { check(pipeline::COPY); }
#[test]
fn sampled_draw_sets_exact_profile_without_sampler_filtering() { check(pipeline::BOX_FILTER); }

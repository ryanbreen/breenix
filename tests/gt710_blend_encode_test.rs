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

//! Native alpha draws use the shared parameters and a CE destination snapshot.
extern crate alloc;
#[allow(dead_code)] // The kernel consumes the remaining committed shader catalogue.
#[path = "../kernel/src/gpu/gk208/gr/shaders.rs"]
mod shaders;
#[path = "../kernel/src/gpu/gk208/encode.rs"]
pub mod encode;
#[path = "../kernel/src/gpu/gk208/caps.rs"]
pub mod caps;
use encode::{Bindings, Target};
use gpu_abi::{draw, format, pipeline, stream::{self, Resolver, SourceInfo, TargetInfo}};

struct View { target_format: u32, source_format: u32, scratch: Option<Target> }
impl View {
    fn new(target_format: u32, source_format: u32) -> Self {
        Self { target_format, source_format, scratch: Some(Target {
            id: 29, address: 0x5_1234_5000, width: 64, height: 64,
            format: target_format, tile_mode: 0x20,
        }) }
    }
}
impl Resolver for View {
    fn target(&self, h: u64) -> Option<TargetInfo> {
        (h == 1).then_some(TargetInfo { width: 64, height: 64, format: self.target_format, invalid: false })
    }
    fn source(&self, h: u64) -> Option<SourceInfo> {
        (h == 2).then_some(SourceInfo { width: 13, height: 9, format: self.source_format, invalid: false })
    }
    fn buffer_size(&self, _: u64) -> Option<u64> { None }
    fn buffer_rect(&self, _: u64, _: u64) -> [i32; 4] { panic!("no buffers") }
}
impl Bindings for View {
    fn render_target(&self, h: u64) -> Option<Target> {
        self.target(h).map(|t| Target { id: 19, address: 0x2_0000_0000,
            width: t.width, height: t.height, format: t.format, tile_mode: 0x30 })
    }
    fn sampled_source(&self, h: u64) -> Option<Target> {
        self.source(h).map(|t| Target { id: 23, address: 0x4_1234_5000,
            width: t.width, height: t.height, format: t.format, tile_mode: 0x10 })
    }
    fn blend_scratch(&self, fmt: u32) -> Option<Target> {
        assert_eq!(fmt, self.target_format);
        self.scratch
    }
}
fn packets(words: &[u32]) -> Vec<(u32, Vec<u32>)> {
    let mut p = Vec::new(); let mut at = 0;
    while at < words.len() {
        let h = words[at]; let n = ((h >> 16) & 0x1fff) as usize;
        assert_eq!(h >> 29, 1); assert!(n > 0 && at + n < words.len());
        p.push(((h & 0x1fff) * 4, words[at+1..at+1+n].to_vec())); at += n + 1;
    }
    p
}
fn data_words(data: &[u8], offset: usize, count: usize) -> Vec<u32> {
    data[offset..offset+count*4].chunks_exact(4)
        .map(|b| u32::from_le_bytes(b.try_into().unwrap())).collect()
}
fn instances(pipe: u32, variant: u32) -> [draw::Instance; 2] {
    let mut a = [0; 16]; a[..4].copy_from_slice(&[4,6,0,0]);
    let mut b = [0; 16]; b[..4].copy_from_slice(&[32,30,1,2]);
    if pipe == pipeline::COVERAGE { a[4] = variant; b[4] = variant; }
    if pipe == pipeline::BITMAP_TEXT {
        a[4..8].copy_from_slice(&[5,7,2,variant]);
        b[4..8].copy_from_slice(&[5,7,2,variant]);
    }
    [draw::Instance { rect: [-5,-4,20,20], params: a },
     draw::Instance { rect: [30,28,60,60], params: b }]
}
fn command(pipe: u32, variant: u32) -> Vec<u32> {
    let mut w = Vec::new(); stream::encode::begin(&mut w, 1);
    w.extend([stream::cmd::SCISSOR,6,4,6,52,50]);
    draw::encode(&mut w, pipe, 0x91827364, 2, 0, 0, &instances(pipe,variant)); w
}
#[test]
fn blended_draws_copy_union_then_bind_both_textures_constants_and_catalogue_program() {
    for target_format in [format::B8G8R8A8, format::R8G8B8A8] {
        for (pipe, variant, source_format, name, offset, header, code, gprs) in [
            (pipeline::OVER,0,format::B8G8R8A8,"over",shaders::PIPELINE_OVER_OFFSET,shaders::PIPELINE_OVER_HEADER,shaders::PIPELINE_OVER_CODE,shaders::PIPELINE_OVER_GPRS),
            (pipeline::OVER_STRAIGHT,0,format::R8G8B8A8,"over_straight",shaders::PIPELINE_OVER_STRAIGHT_OFFSET,shaders::PIPELINE_OVER_STRAIGHT_HEADER,shaders::PIPELINE_OVER_STRAIGHT_CODE,shaders::PIPELINE_OVER_STRAIGHT_GPRS),
            (pipeline::COVERAGE,0,format::R8,"coverage",shaders::PIPELINE_COVERAGE_OFFSET,shaders::PIPELINE_COVERAGE_HEADER,shaders::PIPELINE_COVERAGE_CODE,shaders::PIPELINE_COVERAGE_GPRS),
            (pipeline::COVERAGE,1,format::R8G8B8A8,"coverage",shaders::PIPELINE_COVERAGE_OFFSET,shaders::PIPELINE_COVERAGE_HEADER,shaders::PIPELINE_COVERAGE_CODE,shaders::PIPELINE_COVERAGE_GPRS),
            (pipeline::BITMAP_TEXT,0,format::R8,"bitmap_text",shaders::PIPELINE_BITMAP_TEXT_OFFSET,shaders::PIPELINE_BITMAP_TEXT_HEADER,shaders::PIPELINE_BITMAP_TEXT_CODE,shaders::PIPELINE_BITMAP_TEXT_GPRS),
            (pipeline::BITMAP_TEXT,1,format::R8,"bitmap_text",shaders::PIPELINE_BITMAP_TEXT_OFFSET,shaders::PIPELINE_BITMAP_TEXT_HEADER,shaders::PIPELINE_BITMAP_TEXT_CODE,shaders::PIPELINE_BITMAP_TEXT_GPRS),
        ] {
            let w = command(pipe,variant);
            assert_eq!(encode::pipeline_refusal(w.as_slice()),None);
            let view = View::new(target_format,source_format);
            let mut f = encode::translate(w.as_slice(),&view).unwrap();
            assert_eq!(f.chunks.len(),3);
            let union = if pipe == pipeline::BITMAP_TEXT { [4,6,38,38,4,6] } else { [4,6,40,31,4,6] };
            assert_eq!(f.chunks[0].copy,Some((19,29,union)));
            assert_eq!((f.chunks[0].target,f.chunks[0].source),(29,Some(19)));
            assert!(f.chunks[0].words.is_empty());
            assert_eq!(&f.data[0x100000+offset..0x100050+offset],header);
            assert_eq!(&f.data[0x100050+offset..0x100050+offset+code.len()],code);
            let metadata = match name {
                "over" => include_str!("../kernel/assets/kepler/pipeline-over.json"),
                "over_straight" => include_str!("../kernel/assets/kepler/pipeline-over_straight.json"),
                "coverage" => include_str!("../kernel/assets/kepler/pipeline-coverage.json"),
                _ => include_str!("../kernel/assets/kepler/pipeline-bitmap_text.json"),
            };
            assert!(metadata.contains("\"texture_mask\": 3,"));
            assert!(metadata.contains("\"constant_buffer_bytes\": 104,"));
            f.relocate(0x3_0000_0000).unwrap();
            let rects = if pipe == pipeline::BITMAP_TEXT { [[4,6,14,20],[32,30,42,44]] }
                else { [[4,6,17,15],[31,28,44,37]] };
            for (n,c) in f.chunks[1..].iter().enumerate() {
                assert_eq!(c.command,2); assert_eq!(c.copy,None);
                assert_eq!((c.target,c.source),(19,Some(23)));
                let p = packets(&c.words);
                assert!(p.contains(&(0x2140,vec![0x51,offset as u32])));
                assert!(p.contains(&(0x214c,vec![gprs])));
                assert!(p.contains(&(0x1360,vec![0])));
                let tic = &p.iter().find(|(m,_)| *m == 0x1574).unwrap().1;
                let tsc = &p.iter().find(|(m,_)| *m == 0x155c).unwrap().1;
                assert_eq!((tic[0],tic[2],tsc[0],tsc[2]),(3,1,3,1));
                let source_tic0 = match source_format {
                    format::B8G8R8A8 => 0x2a712488, format::R8G8B8A8 => 0x2c692488, _ => 0x3809249d,
                };
                assert_eq!(data_words(&f.data,tic[1] as usize,8),
                    [source_tic0,0x12345000,0x50405004,0x00300000,0x8000000d,0x10009,0x03000000,0]);
                assert_eq!(data_words(&f.data,tic[1] as usize+32,8),
                    [if target_format == format::B8G8R8A8 {0x2a712488} else {0x2c692488},
                     0x12345000,0x50805005,0x00300000,0x80000040,0x10040,0x03000000,0]);
                assert_eq!(data_words(&f.data,tsc[1] as usize,16),
                    [0x92,0x51,0,0,0,0,0,0,0x92,0x51,0,0,0,0,0,0]);
                let cb: Vec<_> = p.iter().filter(|(m,_)| *m == 0x2380).collect();
                assert_eq!(cb.len(),2);
                let mut constants = [0;32]; constants[..16].copy_from_slice(&instances(pipe,variant)[n].params);
                constants[20] = 0x91827364; constants[24] = 13; constants[25] = 9;
                assert_eq!(data_words(&f.data,cb[0].1[2] as usize,32),constants);
                // Compiler handle 0/1 selects TIC 0/1 and TSC 0/1 in CB15.
                assert_eq!(data_words(&f.data,cb[1].1[2] as usize,2),[0,1|(1<<20)]);
                assert!(p.contains(&(0x2490,vec![0xf1])));
                for method in [0x1334,0x1330,0x1338] { assert!(p.contains(&(method,vec![0]))); }
                let vb = &p.iter().find(|(m,_)| *m == 0x1c00).unwrap().1;
                let [x0,y0,x1,y1] = rects[n]; let mut bytes = Vec::new();
                for [x,y] in [[x0,y0],[x1,y0],[x1,y1],[x0,y0],[x1,y1],[x0,y1]] {
                    for v in [x as f32/32.-1.,y as f32/32.-1.,0.,1.,0.,0.,0.,0.] { bytes.extend(v.to_le_bytes()); }
                }
                assert_eq!(&f.data[vb[2] as usize..vb[2] as usize+192],bytes);
            }
        }
    }
}
#[test]
fn successive_blended_commands_take_fresh_snapshots_and_batching_keeps_copy_order() {
    let mut w = command(pipeline::OVER,0);
    draw::encode(&mut w,pipeline::OVER,0x12345678,2,0,0,&instances(pipeline::OVER,0)[..1]);
    let mut f = encode::translate(w.as_slice(),&View::new(format::B8G8R8A8,format::R8G8B8A8)).unwrap();
    assert_eq!(f.chunks.iter().map(|c|c.copy.is_some()).collect::<Vec<_>>(),[true,false,false,true,false]);
    assert_eq!(f.chunks.iter().map(|c|c.command).collect::<Vec<_>>(),[2,2,2,3,3]);
    assert_eq!(f.chunks[3].copy,Some((19,29,[4,6,13,9,4,6])));
    f.batch_draws(4096).unwrap();
    assert_eq!(f.chunks.iter().map(|c|c.copy.is_some()).collect::<Vec<_>>(),[true,false,true,false]);
    f.relocate(0x3_0000_0000).unwrap();
}
#[test]
fn overlapping_blend_refuses_the_whole_stream_before_any_chunks_escape() {
    let view = View::new(format::B8G8R8A8,format::R8);
    for pipe in [pipeline::OVER,pipeline::OVER_STRAIGHT,pipeline::COVERAGE,pipeline::BITMAP_TEXT] {
        let mut w = Vec::new(); stream::encode::begin(&mut w,1);
        stream::encode::clear(&mut w,0x12345678);
        let i = instances(pipe,0)[0];
        draw::encode(&mut w,pipe,0xffffffff,2,0,0,&[i,i]);
        assert_eq!(stream::validate(w.as_slice(),&view).err(),Some(stream::Refusal {index:2,error:draw::OVERLAP}));
        assert!(encode::translate(w.as_slice(),&view).is_err());
    }
}
#[test]
fn missing_aliasing_malformed_or_small_scratch_refuses_and_empty_blend_needs_none() {
    let mut view = View::new(format::B8G8R8A8,format::R8);
    let scratch = view.scratch.unwrap();
    let w = command(pipeline::COVERAGE,0);
    for replacement in [None,Some(Target {format:format::R8G8B8A8,..scratch}),
        Some(Target {id:19,..scratch}),Some(Target {id:23,..scratch}),
        Some(Target {address:0x2_0000_0000,..scratch}),Some(Target {address:0x4_1234_5000,..scratch}),
        Some(Target {width:43,..scratch}),Some(Target {height:36,..scratch}),
        Some(Target {address:1,..scratch}),Some(Target {tile_mode:0x80,..scratch})] {
        view.scratch = replacement;
        assert!(encode::translate(w.as_slice(),&view).is_err());
    }
    view.scratch = None;
    let mut empty = Vec::new(); stream::encode::begin(&mut empty,1);
    empty.extend([stream::cmd::SCISSOR,6,63,63,64,64]);
    draw::encode(&mut empty,pipeline::COVERAGE,0xffffffff,2,0,0,&instances(pipeline::COVERAGE,0));
    let f = encode::translate(empty.as_slice(),&view).unwrap();
    assert!(f.chunks.iter().all(|c|c.copy.is_none() && c.words.is_empty()));
}

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

//! Literal TIC/TSC fields from MIT envytools rnndb g80_texture.xml and
//! g80_defs.xml at f102b82381f3f11cee113d16374c87091db039d9; A197 pool,
//! cache and CB bindings from MIT NVIDIA cla197.h at the pinned revision.
extern crate alloc;
#[allow(dead_code)] // Diagnostic assets remain used by the kernel's bring-up path.
#[path = "../kernel/src/gpu/gk208/gr/shaders.rs"]
mod shaders;
#[path = "../kernel/src/gpu/gk208/encode.rs"]
pub mod encode;
use gpu_abi::{draw, format, pipeline, stream::{self, Resolver, SourceInfo, TargetInfo}};

struct View(u32);
impl Resolver for View {
    fn target(&self, h: u64) -> Option<TargetInfo> {
        (h == 1).then_some(TargetInfo { width: 64, height: 64, format: format::B8G8R8A8, invalid: false })
    }
    fn source(&self, h: u64) -> Option<SourceInfo> {
        (h == 2).then_some(SourceInfo { width: 13, height: 9, format: self.0, invalid: false })
    }
    fn buffer_size(&self, _: u64) -> Option<u64> { None }
    fn buffer_rect(&self, _: u64, _: u64) -> [i32;4] { panic!("no buffers") }
}
impl encode::Bindings for View {
    fn render_target(&self, h: u64) -> Option<encode::Target> {
        self.target(h).map(|t| encode::Target { id: 19, address: 0x2_0000_0000,
            width: t.width, height: t.height, format: t.format, tile_mode: 0x30 })
    }
    fn sampled_source(&self, h: u64) -> Option<encode::Target> {
        self.source(h).map(|t| encode::Target { id: 23, address: 0x4_1234_5000,
            width: t.width, height: t.height, format: t.format, tile_mode: 0x10 })
    }
}
fn packets(words: &[u32]) -> Vec<(u32,Vec<u32>)> {
    let mut p=Vec::new(); let mut at=0;
    while at<words.len() {
        let h=words[at]; let n=((h>>16)&0x1fff) as usize;
        assert_eq!(h>>29,1); assert_eq!((h>>13)&7,0);
        assert!(n>0 && at+n<words.len());
        p.push(((h&0x1fff)*4,words[at+1..at+1+n].to_vec())); at+=n+1;
    }
    p
}
fn words(data:&[u8], offset:usize, count:usize) -> Vec<u32> {
    data[offset..offset+4*count].chunks_exact(4).map(|b|u32::from_le_bytes(b.try_into().unwrap())).collect()
}
fn command(pipe:u32, params:&[u32]) -> Vec<u32> {
    let mut w=Vec::new(); stream::encode::begin(&mut w,1);
    w.extend([stream::cmd::SCISSOR,6,1,1,60,60]);
    let mut a=[0;16]; a[..params.len()].copy_from_slice(params);
    draw::encode(&mut w,pipe,0xa1234567,2,0,0,&[draw::Instance{rect:[-5,-4,60,50],params:a}]); w
}
#[test]
fn sampled_formats_and_pipelines_bind_exact_descriptors_programs_and_clipped_inputs() {
    for (fmt,tic0) in [(format::B8G8R8A8,0x2a712488),(format::R8G8B8A8,0x2c692488),(format::R8,0x3809249d)] {
        for (pipe,params,r,offset,gprs,header,code) in [
            (pipeline::COPY,vec![(-2i32) as u32,(-1i32) as u32,1],[2,1,15,10],0x2b0,4,&shaders::PIPELINE_COPY_HEADER[..],shaders::PIPELINE_COPY_CODE),
            (pipeline::COPY_SCALED,vec![4,3,2,1,2],[1,1,26,19],0x430,13,&shaders::PIPELINE_COPY_SCALED_HEADER[..],shaders::PIPELINE_COPY_SCALED_CODE),
            (pipeline::NEAREST_SCALED,vec![4,3,12,8,0,0,13,9],[4,3,16,11],0xa30,11,&shaders::PIPELINE_NEAREST_SCALED_HEADER[..],shaders::PIPELINE_NEAREST_SCALED_CODE),
            (pipeline::BOX_FILTER,vec![4,3,6,4,0,0,13,9],[4,3,10,7],0xf30,21,&shaders::PIPELINE_BOX_FILTER_HEADER[..],shaders::PIPELINE_BOX_FILTER_CODE),
        ] {
            let mut f=encode::translate(command(pipe,&params).as_slice(),&View(fmt)).unwrap();
            f.relocate(0x3_0000_0000).unwrap(); assert_eq!(f.chunks.len(),1);
            let c=&f.chunks[0]; assert_eq!(c.target,19); assert_eq!(c.source,Some(23)); let p=packets(&c.words);
            assert!(p.contains(&(0x2140,vec![0x51,offset])));
            assert!(p.contains(&(0x214c,vec![gprs])));
            assert_eq!(&f.data[0x100000+offset as usize..0x100050+offset as usize],header);
            assert_eq!(&f.data[0x100050+offset as usize..0x100050+offset as usize+code.len()],code);
            let start=p.iter().position(|(m,_)| *m==0x1574).unwrap();
            assert_eq!(p[start..start+9].iter().map(|(m,_)|*m).collect::<Vec<_>>(),
                [0x1574,0x155c,0x1334,0x1330,0x1338,0x2608,0x2380,0x2490,0x1930]);
            for (index,expected) in [(2,vec![0]),(3,vec![0]),(4,vec![0]),(5,vec![15]),(7,vec![0xf1])] {
                assert_eq!(p[start+index].1,expected);
            }
            let tic=&p[start].1; let tsc=&p[start+1].1;
            assert_eq!(tic[0],3); assert_eq!(tic[2],0);
            assert_eq!(tsc[0],3); assert_eq!(tsc[2],0);
            // 13x9, block height 2 GOBs, 40-bit VRAM address, no sRGB or mipmaps.
            assert_eq!(words(&f.data,tic[1] as usize,8),[tic0,0x12345000,0x50405004,0x00300000,0x8000000d,0x00010009,0x03000000,0]);
            // rnndb g80_texture.xml TSC: clamp to edge, nearest min/mag, mip filter 1.
            assert_eq!(words(&f.data,tsc[1] as usize,8),[0x92,0x51,0,0,0,0,0,0]);
            let aux=&p[start+6].1; assert_eq!(&aux[..2],&[256,3]);
            assert_eq!(words(&f.data,aux[2] as usize,1),[0]); // TIC 0 | TSC 0
            let cb=&p.iter().find(|(m,_)|*m==0x2380).unwrap().1;
            let constants=words(&f.data,cb[2] as usize,32);
            assert_eq!(&constants[..params.len()],params);
            assert!(constants[params.len()..20].iter().all(|v|*v==0));
            assert_eq!(&constants[20..28],&[0xa1234567,0,0,0,13,9,0,0]);
            let vb=&p.iter().find(|(m,_)|*m==0x1c00).unwrap().1;
            let [x0,y0,x1,y1]=r; let mut vertices=Vec::new();
            for [x,y] in [[x0,y0],[x1,y0],[x1,y1],[x0,y0],[x1,y1],[x0,y1]] {
                for v in [x as f32/32.-1.,y as f32/32.-1.,0.,1.,0.,0.,0.,0.] {vertices.extend(v.to_le_bytes());}
            }
            assert_eq!(&f.data[vb[2] as usize..vb[2] as usize+192],vertices);
            assert!(p.contains(&(0xe00,vec![1,1|60<<16,1|60<<16])));
            assert!(p.contains(&(0xd74,vec![0,6])));
        }
    }
}
#[test]
fn legacy_copy_uses_original_origin_and_empty_source_clips_emit_no_draw() {
    let mut w=Vec::new(); stream::encode::begin(&mut w,1);
    stream::encode::draw_sampled(&mut w,pipeline::COPY,2,&[[-5,-4,60,50,-7,-5]]);
    let f=encode::translate(w.as_slice(),&View(format::R8)).unwrap();
    let p=packets(&f.chunks[0].words); let cb=&p.iter().find(|(m,_)|*m==0x2380).unwrap().1;
    assert_eq!(words(&f.data,cb[2] as usize,3),[(-2i32) as u32,(-1i32) as u32,0]);
    let f=encode::translate(command(pipeline::COPY,&[100,100,0]).as_slice(),&View(format::R8)).unwrap();
    assert!(f.chunks.iter().all(|c|c.words.is_empty()));
    let mut w=command(pipeline::COPY,&[0,0,0]); w.extend([0xffff,2]);
    assert!(encode::translate(w.as_slice(),&View(format::R8)).is_err());
}


#[path = "../kernel/src/gpu/gk208/caps.rs"]
pub mod caps;
#[test]
fn admission_programs_bind_literal_inputs_and_keep_code_before_uniforms() {
    let t = encode::Target { id: 31, address: 0x2_0000_0000, width: 80,
        height: 16, format: format::B8G8R8A8, tile_mode: 0x10 };
    for (source, count, offset, header, code, gprs) in [
        (None, 1280, 0x2030, &shaders::PROBE_INTEGER_OPS_HEADER[..], shaders::PROBE_INTEGER_OPS_CODE, 5),
        (Some(encode::Target {id: 32, ..t}), 1, 0x2430, &shaders::PROBE_TEXEL_FETCH_HEADER[..], shaders::PROBE_TEXEL_FETCH_CODE, 4),
    ] {
        let mut f = encode::probe_frame(t, source);
        f.relocate(0x3_0000_0000).unwrap();
        // Mesa nvc0_program.c:782-805: an 80-byte SPH precedes code aligned to 128 bytes.
        for chunk in &f.chunks {
            let p = packets(&chunk.words);
            let start = p.iter().find(|(m,_)| *m == 0x2140).unwrap().1[1];
            assert_eq!((start + 80) % 128, 0, "Kepler instruction alignment");
        }
        assert_eq!(f.chunks.len(), count);
        assert_eq!(&f.data[0x100000+offset..0x100050+offset], header);
        assert_eq!(&f.data[0x100050+offset..0x100050+offset+code.len()], code);
        for (n, c) in f.chunks.iter().enumerate() {
            assert_eq!((c.target, c.source), (31, source.map(|s| s.id)));
            let p = packets(&c.words);
            assert!(p.contains(&(0x2140, vec![0x51, offset as u32])));
            assert!(p.contains(&(0x214c, vec![gprs])));
            let cb = &p.iter().find(|(m,_)| *m==0x2380).unwrap().1;
            assert!(cb[2] as usize >= 0x100050+offset+code.len());
            let actual = words(&f.data, cb[2] as usize, 4);
            let expected = if source.is_some() { vec![0;4] } else {
                let n = n as u32;
                vec![0x81b70401 ^ n.wrapping_mul(0x9e3779b9), 0xfedcba98 ^ n.wrapping_mul(0x7f4a7c15), n&31, (n>>5)&31]
            };
            assert_eq!(actual, expected);
            assert_eq!(p.iter().any(|(m,_)| *m==0x1574), source.is_some());
        }
    }
}

#[test]
fn basic_warm_frames_use_native_programs_and_refuse_later_pipelines() {
    let target = encode::Bindings::render_target(&View(format::B8G8R8A8), 1).unwrap();
    let source = encode::Bindings::sampled_source(&View(format::B8G8R8A8), 2).unwrap();
    for (pipe, offset) in [(pipeline::SOLID, 0x1b0), (pipeline::COPY, 0x2b0),
        (pipeline::COPY_SCALED, 0x430), (pipeline::NEAREST_SCALED, 0xa30),
        (pipeline::BOX_FILTER, 0xf30)] {
        let frame = encode::warm_frame(target, source, pipe).unwrap();
        assert_eq!(frame.chunks.len(), 1);
        let methods = packets(&frame.chunks[0].words);
        assert!(methods.contains(&(0x2140, vec![0x51, offset])));
        assert_eq!(frame.chunks[0].target, target.id);
    }
    assert!(encode::warm_frame(target, source, pipeline::OVER).is_none());
}

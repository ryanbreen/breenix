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

//! Compare native shape draws with the shared VirGL constant-buffer layout.
extern crate alloc;
const PAGE: usize = 4096;
const POISON: u32 = 0x3c5a_a5c3;
mod gpu {
    pub mod dma {
        #[derive(Default)]
        pub struct WordBuf(Vec<u32>);
        impl WordBuf {
            pub fn new() -> Self { Self::default() }
            pub fn len(&self) -> usize { self.0.len() }
            pub fn push(&mut self, word: u32) -> bool { self.0.push(word); true }
            pub fn word(&self, index: usize) -> u32 { self.0[index] }
        }
    }
}
#[allow(dead_code)]
#[path = "../kernel/src/gpu/vertex_arena.rs"] mod vertex_arena;
#[path = "../tools/gpu-host-tests/virgl.rs"] mod virgl;
#[allow(dead_code)]
#[path = "../kernel/src/gpu/gk208/gr/shaders.rs"] mod shaders;
#[path = "../kernel/src/gpu/gk208/encode.rs"] pub mod native;
#[path = "../kernel/src/gpu/gk208/caps.rs"] pub mod caps;
use gpu_abi::{draw, format, pipeline, stream::{self, Resolver, SourceInfo, TargetInfo}};
struct View { reads: std::cell::RefCell<Vec<(u64,u64)>> }
fn table(h: u64) -> Vec<u32> {
    let mut words = vec![0,2,2,2,2,2,2,2,2,0,1,0x80000000,u32::MAX];
    if h == 3 { words.push(0xaabbccdd); }
    words
}
impl Resolver for View {
    fn target(&self, h: u64) -> Option<TargetInfo> { (h == 1).then_some(TargetInfo { width: 64, height: 64, format: format::B8G8R8A8, invalid: false }) }
    fn source(&self, _: u64) -> Option<SourceInfo> { None }
    fn buffer_size(&self, h: u64) -> Option<u64> { matches!(h, 3|4).then(|| table(h).len() as u64*4) }
    fn buffer_rect(&self, _: u64, _: u64) -> [i32;4] { panic!("word data") }
    fn buffer_word(&self, h:u64, o:u64)->u32 { self.reads.borrow_mut().push((h,o)); table(h)[o as usize/4] }
}
impl native::Bindings for View {
    fn render_target(&self, h:u64)->Option<native::Target> { self.target(h).map(|t| native::Target { id:19,address:0x200000000,width:t.width,height:t.height,format:t.format,tile_mode:0x30 }) }
    fn sampled_source(&self, _:u64)->Option<native::Target> { None }
    fn blend_scratch(&self, _:u32)->Option<native::Target> { Some(native::Target { id:20,address:0x300000000,width:64,height:64,format:format::B8G8R8A8,tile_mode:0x30 }) }
    fn span_texture(&self)->Option<native::Target> { Some(native::Target { id:21,address:0x500000000,width:8,height:4,format:format::R8G8B8A8,tile_mode:0 }) }
}
impl virgl::encode::Bindings for View {
    fn target(&self,h:u64)->Option<virgl::encode::TargetRes> { Resolver::target(self,h).map(|info|virgl::encode::TargetRes{id:19,format:virgl::wire::format::B8G8R8A8_UNORM,info}) }
    fn source(&self,_:u64)->Option<virgl::encode::SourceRes> { None }
    fn buffer_rect(&self,_:u64,_:u64)->[i32;4] { panic!("word data") }
    fn buffer_word(&self,h:u64,o:u64)->u32 { Resolver::buffer_word(self,h,o) }
}
fn words()->Vec<u32> {
    let mut w=Vec::new();stream::encode::begin(&mut w,1);
    for (n,(h,bytes)) in [(3,52),(4,52),(3,56)].into_iter().enumerate() {
        let mut params=[0;16]; params[..5].copy_from_slice(&[n as u32+1,1,1,1,0]);
        draw::encode(&mut w,pipeline::COVERAGE_SPANS,0x8142a5e7,0,h,bytes,&[draw::Instance {rect:[0,0,64,64],params}]);
    } w
}
fn packets(words:&[u32])->Vec<(u32,Vec<u32>)> {
    let mut out=Vec::new();let mut at=0;
    while at<words.len(){let h=words[at];let n=((h>>16)&0x1fff) as usize;out.push(((h&0x1fff)*4,words[at+1..at+1+n].to_vec()));at+=n+1;}out
}
fn u32s(bytes:&[u8])->Vec<u32> {bytes.chunks_exact(4).map(|b|u32::from_le_bytes(b.try_into().unwrap())).collect()}
#[test]
fn distinct_tables_pack_once_with_maximum_extent_and_first_use_bases() {
    let v=View{reads:Default::default()};let w=words();assert!(stream::validate(w.as_slice(),&v).is_ok());v.reads.borrow_mut().clear();
    let data=native::pack_spans(w.as_slice(),&v,8).unwrap().unwrap();
    assert_eq!((data.width,data.height),(8,4));
    let expected:Vec<u8>=table(3).into_iter().chain(table(4)).flat_map(u32::to_le_bytes).collect();
    assert_eq!(&data.bytes[..108],expected);assert_eq!(&data.bytes[108..],&[0;20]);
    let reads:Vec<_>=[(3,14),(4,13)].into_iter().flat_map(|(h,n)|(0..n).map(move|i|(h,i*4))).collect();
    assert_eq!(*v.reads.borrow(),reads);
    for width in [0,3,32768] {assert!(native::pack_spans(w.as_slice(),&v,width).is_err());}
}
#[test]
fn spans_constants_match_virgl_and_sampler_two_has_its_own_tic_tsc() {
    let v=View{reads:Default::default()};let w=words();
    assert_eq!(native::pipeline_refusal(w.as_slice()),None);
    let mut f=native::translate(w.as_slice(),&v).unwrap();
    assert_eq!(f.chunks.len(),6); // each command's destination snapshot precedes its draw
    let mut enc=virgl::encode::Enc::new();
    virgl::encode::translate(&mut enc,&mut virgl::encode::Objects::new(),w.as_slice(),&v,virgl::encode::Extras {vertex_buffer:27,scratch:[20,0],data:21,data_width:8,flip_y:false,corrupt:None,suppress:None,swap:None});
    let mut constants=Vec::new();let mut uploads=Vec::new();
    for chunk in enc.finish().unwrap(){let mut at=0;while at<chunk.len(){let h=chunk.word(at);let n=(h>>16) as usize;
        if h&255==virgl::wire::ccmd::SET_CONSTANT_BUFFER {constants.push((at+3..at+1+n).map(|j|chunk.word(j)).collect::<Vec<_>>());}
        if h&255==virgl::wire::ccmd::RESOURCE_INLINE_WRITE && chunk.word(at+1)==21 {uploads.extend((at+12..at+1+n).map(|j|chunk.word(j)));}
        at+=1+n;}}
    assert_eq!(uploads,table(3).into_iter().chain(table(4)).collect::<Vec<_>>());
    assert_eq!(constants.len(),3);
    for (n,pair) in f.chunks.chunks_exact(2).enumerate(){
        assert_eq!(pair[0].copy,Some((19,20,[n as u32+1,1,1,1,n as u32+1,1])));
        let p=packets(&pair[1].words);
        let cb=&p.iter().find(|(m,_)|*m==0x2380).unwrap().1;
        let actual=u32s(&f.data[cb[2] as usize..cb[2] as usize+128]);
        assert_eq!(actual,constants[n]);assert_eq!(&actual[26..28],&[if n==1 {14}else{0},3]);
        let tic=&p.iter().find(|(m,_)|*m==0x1574).unwrap().1;assert_eq!(tic[2],2);
        let off=tic[1] as usize+64;
        assert_eq!(u32s(&f.data[off..off+32]),[0x2c692488,0,0x50005005,0x00300000,0x80000008,0x10004,0x03000000,0]);
        let tsc=&p.iter().find(|(m,_)|*m==0x155c).unwrap().1;assert_eq!(tsc[2],2);
        let off=tsc[1] as usize+64;assert_eq!(u32s(&f.data[off..off+32]),[146,81,0,0,0,0,0,0]);
        let cb15=&p.iter().filter(|(m,_)|*m==0x2380).nth(1).unwrap().1;
        let off=cb15[2] as usize;assert_eq!(u32s(&f.data[off..off+12]),[0,0x100001,0x200002]);
        assert_eq!(pair[1].source,Some(21)); // retained by prepare along with the snapshot chunk
        assert!(p.contains(&(0x2140,vec![0x51,shaders::PIPELINE_COVERAGE_SPANS_OFFSET as u32])));
    }
    f.relocate(0x600000000).unwrap();
}

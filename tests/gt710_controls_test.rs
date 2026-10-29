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

/*******************************************************************************
    Copyright (c) 2019, NVIDIA CORPORATION. All rights reserved.

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

//! Native suite controls change engine methods; the production latch supplies one shot.
extern crate alloc;
#[allow(dead_code)] // Bring-up programs are used by the kernel, outside this harness.
#[path = "../kernel/src/gpu/gk208/gr/shaders.rs"] mod shaders;
#[allow(dead_code)] // Admission APIs have separate CAPS checks.
#[path = "../kernel/src/gpu/gk208/caps.rs"] mod caps;
#[allow(dead_code)] // Relocation and admission APIs have separate native encoder checks.
#[path = "../kernel/src/gpu/gk208/encode.rs"] mod encode;
#[allow(dead_code)] // The pitch-only and bring-up APIs have separate checks.
#[path = "../kernel/src/gpu/gk208/copy.rs"] mod copy;
#[path = "../kernel/src/test_control/point.rs"] mod point;
use gpu_abi::{format, pipeline, stream::{self, Resolver, SourceInfo, TargetInfo}};

struct View;
impl Resolver for View {
    fn target(&self,h:u64)->Option<TargetInfo> {
        (h==1 || h==4).then_some(TargetInfo{width:16,height:8,format:format::B8G8R8A8,invalid:false})
    }
    fn source(&self,h:u64)->Option<SourceInfo> {
        match h {2=>Some(SourceInfo{width:10,height:6,format:format::B8G8R8A8,invalid:false}),
            3=>Some(SourceInfo{width:4,height:3,format:format::R8G8B8A8,invalid:false}),_=>None}
    }
    fn buffer_size(&self,h:u64)->Option<u64> {(h==5).then_some(16)}
    fn buffer_rect(&self,_:u64,_:u64)->[i32;4] {[2,2,5,4]}
}
impl encode::Bindings for View {
    fn render_target(&self,h:u64)->Option<encode::Target> {
        self.target(h).map(|t| encode::Target{id:h as u32+10,address:h<<32,width:t.width,
            height:t.height,format:t.format,tile_mode:0x10})
    }
    fn sampled_source(&self,h:u64)->Option<encode::Target> {
        self.source(h).map(|t| encode::Target{id:h as u32+10,address:h<<32,width:t.width,
            height:t.height,format:t.format,tile_mode:0x10})
    }
}
fn packets(w:&[u32],sub:u32)->Vec<(u32,Vec<u32>)> {
    let mut p=vec![];let mut at=0;
    while at<w.len() {let h=w[at];let n=((h>>16)&0x1fff) as usize;
        assert_eq!(h>>29,1);assert_eq!((h>>13)&7,sub);assert!(n>0 && at+n<w.len());
        p.push(((h&0x1fff)*4,w[at+1..at+1+n].to_vec()));at+=n+1;}
    p
}
fn bytes(f:&encode::Frame,offset:u32,n:usize)->Vec<u32> {
    f.data[offset as usize..offset as usize+4*n].chunks_exact(4)
        .map(|v|u32::from_le_bytes(v.try_into().unwrap())).collect()
}
fn same(a:&encode::Frame,b:&encode::Frame) {
    assert_eq!(a.data,b.data);assert_eq!(a.chunks.len(),b.chunks.len());
    for (a,b) in a.chunks.iter().zip(&b.chunks) {assert_eq!((a.target,a.source),(b.target,b.source));assert_eq!(a.words,b.words);}
}
fn solid_stream(buffer:bool)->Vec<u32> {
    let mut w=vec![];stream::encode::begin(&mut w,1);
    // (0,0) is outside both the scissor and the draw.
    w.extend([stream::cmd::SCISSOR,6,2,2,5,4]);
    if buffer {stream::encode::draw_buffer(&mut w,pipeline::SOLID,0xff123456,5,0,1);}
    else {stream::encode::draw(&mut w,pipeline::SOLID,0xff123456,&[[2,2,5,4]]);}
    w
}
#[test]
fn corruption_is_one_extra_gpu_pixel_draw_ignoring_scissor_then_next_request_is_clean() {
    let p=point::Point::new();p.arm(1,0);
    let w=solid_stream(false);let clean=encode::translate(w.as_slice(),&View).unwrap();
    let shot=p.reserve(1,|_|true).unwrap();
    let f=encode::translate_with_extras(w.as_slice(),&View,encode::Extras{corrupt:Some(1),..Default::default()}).unwrap();shot.fire();
    assert_eq!(f.chunks.len(),2);assert_eq!(f.chunks[0].words,clean.chunks[0].words);
    let c=&f.chunks[1];assert_eq!((c.target,c.source),(11,None));let m=packets(&c.words,0);
    assert!(m.contains(&(0xe00,vec![1,1<<16,1<<16])));
    assert!(m.contains(&(0xd74,vec![0,6])));
    let cb=m.iter().find(|(m,_)|*m==0x2380).unwrap();
    // !0x3c5aa5c3 as BGRA, encoded as logical RGBA binary32 constants.
    assert_eq!(bytes(&f,cb.1[2],4),[0xa5,0x5a,0x3c,0xc3].map(|v|gpu_abi::float::ratio_bits(v,255)));
    let vb=m.iter().find(|(m,_)|*m==0x1c00).unwrap();
    assert_eq!(&bytes(&f,vb.1[2],48)[..4],&[0xbf800000,0xbf800000,0,0x3f800000]);
    assert!(p.reserve(1,|_|true).is_none());assert_eq!(p.fired(),1);
    same(&encode::translate(w.as_slice(),&View).unwrap(),&clean);p.disarm();
}
#[test]
fn suppression_drops_only_the_selected_draw_and_retains_later_draws_and_clean_next_request() {
    for buffer in [false,true] {
        let p=point::Point::new();p.arm(1,0);let shot=p.reserve(1,|_|true).unwrap();
        let mut w=solid_stream(buffer);stream::encode::draw(&mut w,pipeline::SOLID,0xffabcdef,&[[2,2,3,3]]);
        let f=encode::translate_with_extras(w.as_slice(),&View,encode::Extras{suppress:Some(1),..Default::default()}).unwrap();shot.fire();
        assert_eq!(f.chunks.len(),1);let m=packets(&f.chunks[0].words,0);
        let cb=m.iter().find(|(m,_)|*m==0x2380).unwrap();
        assert_eq!(bytes(&f,cb.1[2],4),[0xab,0xcd,0xef,0xff].map(|v|gpu_abi::float::ratio_bits(v,255)));
        let alone=solid_stream(buffer);
        let f=encode::translate_with_extras(alone.as_slice(),&View,encode::Extras{suppress:Some(1),..Default::default()}).unwrap();
        assert_eq!(f.chunks.len(),1);assert!(f.chunks[0].words.is_empty());
        assert!(p.reserve(1,|_|true).is_none());assert_eq!(p.fired(),1);
        assert_eq!(encode::translate(w.as_slice(),&View).unwrap().chunks.len(),2);
    }
}
#[test]
fn wrong_texture_binds_other_storage_format_and_bounds_for_only_one_command() {
    let p=point::Point::new();p.arm(2,0);let shot=p.reserve(2,|_|true).unwrap();
    let mut w=vec![];stream::encode::begin(&mut w,1);let at=w.len();
    for _ in 0..2 {stream::encode::draw_sampled(&mut w,pipeline::COPY,2,&[[0,0,10,6,0,0]]);}
    let other=encode::Bindings::sampled_source(&View,3).unwrap();
    let f=encode::translate_with_extras(w.as_slice(),&View,encode::Extras{swap:Some((at,other)),..Default::default()}).unwrap();shot.fire();
    assert_eq!(f.chunks.len(),2);
    for (n,c) in f.chunks.iter().enumerate() {
        assert_eq!(c.source,Some(if n==0 {13} else {12}));let m=packets(&c.words,0);
        let tic=m.iter().find(|(m,_)|*m==0x1574).unwrap();let d=bytes(&f,tic.1[1],8);
        assert_eq!((d[0],d[1],d[2]&0xff,d[4],d[5]),if n==0 {(0x2c692488,0,3,0x80000004,0x10003)} else {(0x2a712488,0,2,0x8000000a,0x10006)});
        let cb=m.iter().find(|(m,_)|*m==0x2380).unwrap();assert_eq!(&bytes(&f,cb.1[2],26)[24..26],if n==0 {&[4,3]} else {&[10,6]});
    }
    assert!(p.reserve(2,|_|true).is_none());assert_eq!(p.fired(),1);
    let clean=encode::translate(w.as_slice(),&View).unwrap();assert!(clean.chunks.iter().all(|c|c.source==Some(12)));
}
fn image(address:u64)->copy::Image {copy::Image{address,width:16,height:8,texel_bytes:4,pitch:64,block_height:2,bytes:1024}}
fn linear(address:u64)->copy::Linear {copy::Linear{address,bytes:4096,offset:0,stride:64}}
fn transfer(upload:bool)->copy::Plan {
    if upload {copy::upload(image(1<<32),linear(2<<32),[0,0,16,8])}
    else {copy::readback(image(1<<32),linear(2<<32),[0,0,16,8])}.unwrap()
}
#[test]
fn skipped_upload_and_transfer_back_omit_a0b5_data_launch_but_keep_real_releases() {
    for upload in [true,false] {
        let p=point::Point::new();p.arm(1,0);let shot=p.reserve(1,|_|true).unwrap();
        let plan=transfer(upload).with_extras(copy::Extras{skip:true,stale_fence:false});
        let m=packets(&copy::encode_plan(plan,0x800000,17,77).unwrap(),4);shot.fire();
        assert_eq!(m,vec![(0,vec![0xa0b5]),(0x240,vec![0,0x800004,17]),(0x300,vec![12]),
            (0x240,vec![0,0x800008,0]),(0x300,vec![12]),(0x240,vec![0,0x800000,77]),(0x300,vec![12])]);
        assert!(p.reserve(1,|_|true).is_none());assert_eq!(p.fired(),1);
        let m=packets(&copy::encode_plan(transfer(upload),0x800000,17,78).unwrap(),4);
        assert_eq!(m.iter().filter(|(m,_)|*m==0x400).count(),1);
        assert_eq!(m.iter().filter(|(m,_)|*m==0x300).map(|(_,v)|v[0]).collect::<Vec<_>>(),if upload {vec![0x686,12,12,12]} else {vec![0x706,12,12,12]});
    }
}
#[test]
fn stale_transfer_writes_wrong_sequence_on_device_then_a_separate_terminal_drain() {
    let p=point::Point::new();p.arm(1,0);let shot=p.reserve(1,|_|true).unwrap();
    let plan=transfer(false).with_extras(copy::Extras{skip:false,stale_fence:true});
    let m=packets(&copy::encode_plan(plan,0x800000,17,77).unwrap(),4);shot.fire();
    assert!(m.contains(&(0x400,vec![1,0,2,0,0,64,16,8])));
    let releases:Vec<_>=m.iter().filter(|(m,_)|*m==0x240).map(|(_,v)|v.clone()).collect();
    assert_eq!(releases,vec![vec![0,0x800004,17],vec![0,0x800008,0],vec![0,0x800000,78],vec![0,0x80000c,77]]);
    assert_eq!(&m[m.len()-2..],&[(0x240,vec![0,0x80000c,77]),(0x300,vec![12])]);
    assert!(p.reserve(1,|_|true).is_none());assert_eq!(p.fired(),1);
    let m=packets(&copy::encode_plan(transfer(false),0x800000,17,78).unwrap(),4);
    assert_eq!(m.iter().filter(|(m,_)|*m==0x240).map(|(_,v)|v.clone()).collect::<Vec<_>>(),vec![vec![0,0x800004,17],vec![0,0x800008,0],vec![0,0x800000,78]]);
}
#[test]
fn wrong_resource_copies_other_image_to_its_own_backing_and_leaves_requested_backing_unwritten() {
    let p=point::Point::new();p.arm(1,0);let shot=p.reserve(1,|_|true).unwrap();
    let wrong=copy::readback(image(3<<32),linear(4<<32),[0,0,16,8]).unwrap();
    let m=packets(&copy::encode_plan(wrong,0x800000,17,77).unwrap(),4);shot.fire();
    assert!(m.contains(&(0x400,vec![3,0,4,0,0,64,16,8])));
    assert!(p.reserve(1,|_|true).is_none());assert_eq!(p.fired(),1);
    let m=packets(&copy::encode_plan(transfer(false),0x800000,17,78).unwrap(),4);
    assert!(m.contains(&(0x400,vec![1,0,2,0,0,64,16,8])));
}

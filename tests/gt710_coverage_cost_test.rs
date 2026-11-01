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

//! Bound the native full-screen coverage stream's CPU payload and CE work.
extern crate alloc;
#[allow(dead_code)] // The production channel consumes the complete catalogue.
#[path = "../kernel/src/gpu/gk208/gr/shaders.rs"] mod shaders;
#[path = "../kernel/src/gpu/gk208/encode.rs"] pub mod encode;
#[path = "../kernel/src/gpu/gk208/caps.rs"] pub mod caps;
#[path = "../kernel/src/gpu/gk208/copy.rs"] pub mod copy;
use gpu_abi::{draw, format, pipeline, stream::{self, Resolver, SourceInfo, TargetInfo}};
use std::{alloc::{GlobalAlloc, Layout, System}, sync::atomic::{AtomicUsize, Ordering}};
struct Count;
static ALLOCATED: AtomicUsize = AtomicUsize::new(0);
unsafe impl GlobalAlloc for Count {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        ALLOCATED.fetch_add(l.size(), Ordering::Relaxed); System.alloc(l)
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) { System.dealloc(p,l) }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        ALLOCATED.fetch_add(n, Ordering::Relaxed); System.realloc(p,l,n)
    }
}
#[global_allocator] static ALLOCATOR: Count = Count;
struct View;
impl Resolver for View {
    fn target(&self,h:u64)->Option<TargetInfo> { (h==1).then_some(TargetInfo {
        width:2560,height:1600,format:format::B8G8R8A8,invalid:false }) }
    fn source(&self,h:u64)->Option<SourceInfo> { (h==2).then_some(SourceInfo {
        width:2560,height:1600,format:format::R8,invalid:false }) }
    fn buffer_size(&self,_:u64)->Option<u64>{None}
    fn buffer_rect(&self,_:u64,_:u64)->[i32;4]{panic!("no buffers")}
}
impl encode::Bindings for View {
    fn render_target(&self,h:u64)->Option<encode::Target>{self.target(h).map(|t|encode::Target {
        id:19,address:0x2_0000_0000,width:t.width,height:t.height,format:t.format,tile_mode:0x40 })}
    fn sampled_source(&self,h:u64)->Option<encode::Target>{self.source(h).map(|t|encode::Target {
        id:23,address:0x3_0000_0000,width:t.width,height:t.height,format:t.format,tile_mode:0x40 })}
    fn blend_scratch(&self,_:u32)->Option<encode::Target>{Some(encode::Target {
        id:29,address:0x4_0000_0000,..self.render_target(1).unwrap() })}
}
fn packets(words:&[u32])->Vec<(u32,Vec<u32>)>{
    let mut p=Vec::new();let mut at=0;
    while at<words.len(){let h=words[at];let n=((h>>16)&0x1fff) as usize;
        assert_eq!(h>>29,1);assert!(n>0 && at+n<words.len());
        p.push(((h&0x1fff)*4,words[at+1..at+n+1].to_vec()));at+=n+1;}
    p
}
fn stream()->Vec<u32>{
    let mut w=Vec::new();stream::encode::begin(&mut w,1);
    draw::encode(&mut w,pipeline::COVERAGE,0xffb36d21,2,0,0,&[draw::Instance {
        rect:[0,0,2560,1600],params:[0;16] }]);w
}
#[test]
fn full_coverage_avoids_catalogue_allocation_and_keeps_identical_packets(){
    let w=stream();
    ALLOCATED.store(0,Ordering::Relaxed);
    // The runner substitutes the function selected by the production submit path.
    let mut f=encode::RUNTIME_TRANSLATE(w.as_slice(),&View).unwrap();
    f.compact().unwrap();f.batch_draws(1400).unwrap();
    let bytes=ALLOCATED.load(Ordering::Relaxed);
    println!("coverage translation allocated {bytes} bytes");
    assert!(bytes<=64*1024,"coverage translation allocated {bytes} bytes; budget 65536");
    assert_eq!(f.chunks.len(),2);
    assert_eq!(f.chunks[0].copy,Some((19,29,[0,0,2560,1600,0,0])));
    assert_eq!(packets(&f.chunks[1].words).iter().filter(|(m,_)|*m==0xd74).count(),1);
    let mut reference=encode::translate(w.as_slice(),&View).unwrap();
    reference.compact().unwrap();reference.batch_draws(1400).unwrap();
    assert_eq!(&f.data[..],&reference.data[..]);
    f.relocate(0x5_0000_0000).unwrap();reference.relocate(0x5_0000_0000).unwrap();
    for (a,b) in f.chunks.iter().zip(&reference.chunks){
        assert_eq!(a.words,b.words);assert_eq!(a.copy,b.copy);
        assert_eq!((a.command,a.target,a.source),(b.command,b.target,b.source));
    }
}
fn image(address:u64)->copy::Image{copy::Image{address,width:2560,height:1600,
    texel_bytes:4,pitch:10240,block_height:16,bytes:10240*1664}}
#[test]
fn full_snapshot_is_one_byte_copy_with_three_ordered_releases(){
    let (s,d)=(image(0x2_0000_0000),image(0x4_0000_0000));
    let p=packets(&copy::encode_plan(copy::region(s,d,[0,0,2560,1600,0,0],None).unwrap(),0x1000,17,23).unwrap());
    // Equal full layouts admit a byte copy, including identical tiled padding.
    assert_eq!(p.iter().filter(|(m,_)|*m==0x300).map(|(_,v)|v[0]).collect::<Vec<_>>(),vec![0x386,12,12,12]);
    assert!(!p.iter().any(|(m,_)|matches!(*m,0x708|0x70c|0x728)));
    let extent=&p.iter().find(|(m,_)|*m==0x400).unwrap().1;
    assert_eq!(extent,&vec![2,0,4,0,s.bytes as u32,s.bytes as u32,s.bytes as u32,1]);
    assert_eq!(p.iter().filter(|(m,_)|*m==0x240).map(|(_,v)|v.clone()).collect::<Vec<_>>(),
        vec![vec![0,0x1004,17],vec![0,0x1008,0],vec![0,0x1000,23]]);
    // Partial or unequal layouts must retain coordinates and tiled conversion.
    for (dst,r) in [(d,[0,0,2559,1600,0,0]),(copy::Image{block_height:8,..d},[0,0,2560,1600,0,0])]{
        let p=packets(&copy::encode_plan(copy::region(s,dst,r,None).unwrap(),0x1000,17,23).unwrap());
        assert!(p.iter().any(|(m,_)|*m==0x70c));assert!(p.iter().any(|(m,_)|*m==0x728));
    }
}

#[test]
fn resident_frames_keep_probe_inputs_controls_and_empty_fences() {
    let compare = |w:&[u32],x:encode::Extras| {
        let mut a=encode::translate_resident_with_extras(w,&View,x).unwrap();
        let mut b=encode::translate_with_extras(w,&View,x).unwrap();
        a.compact().unwrap();b.compact().unwrap();
        assert_eq!(&a.data[..],&b.data[..]);assert_eq!(a.probe_mask,b.probe_mask);
        a.batch_draws(1400).unwrap();b.batch_draws(1400).unwrap();
        a.relocate(0x5_0000_0000).unwrap();b.relocate(0x5_0000_0000).unwrap();
        assert_eq!(a.chunks.len(),b.chunks.len());
        for (a,b) in a.chunks.iter().zip(&b.chunks){
            assert_eq!(a.words,b.words);assert_eq!(a.copy,b.copy);
            assert_eq!((a.command,a.target,a.source),(b.command,b.target,b.source));
        }
    };
    let mut w=Vec::new();stream::encode::begin(&mut w,1);
    compare(&w,encode::Extras::default());
    stream::encode::clear(&mut w,0xff123456);
    stream::encode::draw(&mut w,pipeline::SOLID,0xff785634,&[[0,0,10,10],[20,20,40,40]]);
    for routine in 0..25 {
        w.extend([stream::cmd::PROBE,24,routine,0,0,0,1,1]);
        let mut inputs=[0;16];if routine==11 {inputs[3]=1;}w.extend(inputs);
    }
    compare(&w,encode::Extras::default());
    compare(&w,encode::Extras {corrupt:Some(1),..encode::Extras::default()});
    compare(&w,encode::Extras {suppress:Some(1),..encode::Extras::default()});
    let w=stream();
    compare(&w,encode::Extras {suppress:Some(1),..encode::Extras::default()});
    compare(&w,encode::Extras {corrupt:Some(1),..encode::Extras::default()});
}

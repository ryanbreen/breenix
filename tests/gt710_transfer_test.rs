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

// Expected fields: open-gpu-doc classes/dma-copy/cla0b5.h (MIT).
//! Literal A0B5 remap words, independent of the encoder's bit constants.
extern crate alloc;
#[allow(dead_code)] // This standalone harness exercises the tiled API; pitch API has its own check.
#[path = "../kernel/src/gpu/gk208/copy.rs"] mod copy;
use copy::{Image, Linear};

fn packets(words: &[u32]) -> Vec<(u32, Vec<u32>)> {
    let mut out = Vec::new(); let mut at = 0;
    while at < words.len() {
        let h = words[at]; let count = ((h >> 16) & 0x1fff) as usize;
        assert_eq!(h >> 29, 1); assert_eq!((h >> 13) & 7, 4);
        assert!(count > 0 && at + count < words.len());
        out.push(((h & 0x1fff) * 4, words[at + 1..at + 1 + count].to_vec()));
        at += count + 1;
    }
    out
}
fn image(bpp: u32) -> Image {
    Image { address: 0x1_0000_0000, width: 48, height: 17,
        texel_bytes: bpp, pitch: if bpp == 4 { 192 } else { 64 },
        block_height: 4, bytes: if bpp == 4 { 6144 } else { 2048 } }
}
fn linear(stride: u32) -> Linear {
    Linear { address: 0x2_0000_0000, bytes: 8192, offset: 19, stride }
}
#[test]
fn subrectangle_remaps_use_pixel_width_and_literal_host_offset_and_stride() {
    for (bpp, stride, remap) in [(4, 211, 0x30000), (1, 67, 0)] {
        let i = image(bpp); let l = linear(stride);
        let upload = copy::upload(i, l, [3, 5, 13, 7]).unwrap();
        let p = packets(&copy::encode_plan(upload, 0x800000, 0x12345678abcdef01, 77).unwrap());
        assert!(p.contains(&(0x400, vec![2,19,1,0,stride,0,13,7])));
        assert!(p.contains(&(0x708, vec![remap])));
        assert!(p.contains(&(0x70c, vec![0x1020,48,17,1,0,0x00050003])));
        assert_eq!(p.iter().filter(|(m,_)| *m==0x300).map(|(_,v)|v[0]).collect::<Vec<_>>(),
            vec![0x686,0x0c,0x0c,0x0c]);
        let sem: Vec<_> = p.iter().filter(|(m,_)| *m==0x240).map(|(_,v)|v.clone()).collect();
        assert_eq!(sem,vec![vec![0,0x800004,0xabcdef01],vec![0,0x800008,0x12345678],vec![0,0x800000,77]]);
        let read = copy::readback(i, l, [3,5,13,7]).unwrap();
        let p = packets(&copy::encode_plan(read,0x800000,1,2).unwrap());
        assert!(p.contains(&(0x400,vec![1,0,2,19,0,stride,13,7])));
        assert!(p.contains(&(0x728,vec![0x1020,48,17,1,0,0x00050003])));
        assert!(p.contains(&(0x708,vec![remap])));
        assert_eq!(p.iter().find(|(m,_)|*m==0x300).unwrap().1, vec![0x706]);
    }
}
#[test]
fn edge_clipping_keeps_host_stride_and_checks_the_last_written_byte() {
    let i=image(4); let mut l=linear(59);
    l.bytes=19+59+3*4;
    let p=packets(&copy::encode_plan(copy::upload(i,l,[45,15,20,9]).unwrap(),0x800000,1,1).unwrap());
    assert!(p.contains(&(0x400,vec![2,19,1,0,59,0,3,2])));
    assert!(p.contains(&(0x70c,vec![0x1020,48,17,1,0,0x000f002d])));
    l.bytes-=1; assert!(copy::upload(i,l,[45,15,20,9]).is_err());
    assert!(copy::upload(i,linear(11),[0,0,3,2]).is_err());
    assert!(copy::readback(i,linear(211),[48,0,1,1]).is_err());
}
#[test]
fn overlap_has_only_source_to_vram_staging_then_staging_to_destination() {
    let i=image(4); let staging=Image { address:0x3_0000_0000,..i };
    assert!(copy::region(i,i,[2,3,20,10,4,5],None).is_err());
    let plan=copy::region(i,i,[2,3,20,10,4,5],Some(staging)).unwrap();
    let p=packets(&copy::encode_plan(plan,0x800000,1,9).unwrap());
    let launches:Vec<_>=p.iter().filter(|(m,_)|*m==0x300).map(|(_,v)|v[0]).collect();
    assert_eq!(launches,vec![0x606,0x606,0x0c,0x0c,0x0c]);
    let addresses:Vec<_>=p.iter().filter(|(m,_)|*m==0x400).map(|(_,v)|v.clone()).collect();
    assert_eq!(addresses,vec![vec![1,0,3,0,0,0,20,10],vec![3,0,1,0,0,0,20,10]]);
    let origins:Vec<_>=p.iter().filter(|(m,_)|*m==0x728 || *m==0x70c).map(|(m,v)|(*m,v[5])).collect();
    assert_eq!(origins,vec![(0x728,0x00030002),(0x70c,0),(0x728,0),(0x70c,0x00050004)]);
    let mut other=i; other.address=0x4_0000_0000;
    let p=packets(&copy::encode_plan(copy::region(i,other,[47,16,8,8,0,0],None).unwrap(),0x800000,1,1).unwrap());
    assert_eq!(p.iter().filter(|(m,_)|*m==0x400).count(),1);
    assert!(p.contains(&(0x400,vec![1,0,4,0,0,0,1,1])));
    assert!(copy::region(i,i,[2,3,20,10,4,5],Some(i)).is_err());
}

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

//! Literal A197 packets from MIT open-gpu-doc cla197.h at
//! 9fdf5c4062007929d9f4e6cbad9c9771fe61b880: RT 0x800, viewport 0xa00,
//! scissor 0xe00, vertex fetch 0x1c00, programs 0x2040/0x2140, CB 0x2380.
extern crate alloc;
#[allow(dead_code)] // Other committed catalogue programs have their own users in the kernel.
#[path = "../kernel/src/gpu/gk208/gr/shaders.rs"]
mod shaders;
#[path = "../kernel/src/gpu/gk208/encode.rs"]
pub mod encode;
#[path = "../kernel/src/gpu/gk208/caps.rs"]
pub mod caps;
use gpu_abi::{format, stream::{self, Resolver, TargetInfo, SourceInfo}};

struct View(u32);
impl Resolver for View {
    fn target(&self, h: u64) -> Option<TargetInfo> {
        (h == 1).then_some(TargetInfo { width: 64, height: 64, format: self.0, invalid: false })
    }
    fn source(&self, _: u64) -> Option<SourceInfo> { None }
    fn buffer_size(&self, _: u64) -> Option<u64> { None }
    fn buffer_rect(&self, _: u64, _: u64) -> [i32; 4] { panic!("no buffer") }
}
impl encode::Bindings for View {
    fn sampled_source(&self, _: u64) -> Option<encode::Target> { None }
    fn render_target(&self, h: u64) -> Option<encode::Target> {
        self.target(h).map(|t| encode::Target { id: 19, address: 0x2_0000_0000,
            width: t.width, height: t.height, format: t.format, tile_mode: 0x30 })
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
fn command(routine: u32, word: u32, inputs: &[u32]) -> Vec<u32> {
    let mut w = Vec::new(); stream::encode::begin(&mut w, 1);
    w.extend([stream::cmd::SCISSOR,6,3,5,60,61]);
    w.extend([stream::cmd::PROBE,24,routine,word,0,0,64,64]);
    w.extend_from_slice(inputs); w.resize(w.len() + 16 - inputs.len(), 0); w
}
#[test]
fn profile_vectors_bind_per_routine_program_constants_and_clipped_rectangle() {
    // Vectors from integer_ops, wide_arithmetic and fragcoord_integer.
    let vectors: &[(u32,u32,&[u32],usize,&[u8],&[u8;80],u32)] = &[
        (0,0,&[0xffffffff, 1],shaders::PROBE_INT_00_OFFSET,shaders::PROBE_INT_00_CODE,shaders::PROBE_INT_00_HEADER,shaders::PROBE_INT_00_GPRS),
        (0,0,&[0x7fffffff, 1],shaders::PROBE_INT_00_OFFSET,shaders::PROBE_INT_00_CODE,shaders::PROBE_INT_00_HEADER,shaders::PROBE_INT_00_GPRS),
        (1,0,&[0, 1],shaders::PROBE_INT_01_OFFSET,shaders::PROBE_INT_01_CODE,shaders::PROBE_INT_01_HEADER,shaders::PROBE_INT_01_GPRS),
        (1,0,&[0x80000000, 1],shaders::PROBE_INT_01_OFFSET,shaders::PROBE_INT_01_CODE,shaders::PROBE_INT_01_HEADER,shaders::PROBE_INT_01_GPRS),
        (2,0,&[0xffffffff, 0xffffffff],shaders::PROBE_INT_02_OFFSET,shaders::PROBE_INT_02_CODE,shaders::PROBE_INT_02_HEADER,shaders::PROBE_INT_02_GPRS),
        (3,0,&[1, 31],shaders::PROBE_INT_03_OFFSET,shaders::PROBE_INT_03_CODE,shaders::PROBE_INT_03_HEADER,shaders::PROBE_INT_03_GPRS),
        (3,0,&[0xffffffff, 0],shaders::PROBE_INT_03_OFFSET,shaders::PROBE_INT_03_CODE,shaders::PROBE_INT_03_HEADER,shaders::PROBE_INT_03_GPRS),
        (4,0,&[0x80000000, 31],shaders::PROBE_INT_04_OFFSET,shaders::PROBE_INT_04_CODE,shaders::PROBE_INT_04_HEADER,shaders::PROBE_INT_04_GPRS),
        (5,0,&[0x80000000, 31],shaders::PROBE_INT_05_OFFSET,shaders::PROBE_INT_05_CODE,shaders::PROBE_INT_05_HEADER,shaders::PROBE_INT_05_GPRS),
        (5,0,&[0x80000000, 0],shaders::PROBE_INT_05_OFFSET,shaders::PROBE_INT_05_CODE,shaders::PROBE_INT_05_HEADER,shaders::PROBE_INT_05_GPRS),
        (6,0,&[0x80000000, 0x7fffffff],shaders::PROBE_INT_06_OFFSET,shaders::PROBE_INT_06_CODE,shaders::PROBE_INT_06_HEADER,shaders::PROBE_INT_06_GPRS),
        (6,0,&[0x7fffffff, 0x80000000],shaders::PROBE_INT_06_OFFSET,shaders::PROBE_INT_06_CODE,shaders::PROBE_INT_06_HEADER,shaders::PROBE_INT_06_GPRS),
        (7,0,&[0xffffffff, 0],shaders::PROBE_INT_07_OFFSET,shaders::PROBE_INT_07_CODE,shaders::PROBE_INT_07_HEADER,shaders::PROBE_INT_07_GPRS),
        (7,0,&[0, 0xffffffff],shaders::PROBE_INT_07_OFFSET,shaders::PROBE_INT_07_CODE,shaders::PROBE_INT_07_HEADER,shaders::PROBE_INT_07_GPRS),
        (8,0,&[0],shaders::PROBE_INT_08_OFFSET,shaders::PROBE_INT_08_CODE,shaders::PROBE_INT_08_HEADER,shaders::PROBE_INT_08_GPRS),
        (8,0,&[127],shaders::PROBE_INT_08_OFFSET,shaders::PROBE_INT_08_CODE,shaders::PROBE_INT_08_HEADER,shaders::PROBE_INT_08_GPRS),
        (8,0,&[128],shaders::PROBE_INT_08_OFFSET,shaders::PROBE_INT_08_CODE,shaders::PROBE_INT_08_HEADER,shaders::PROBE_INT_08_GPRS),
        (8,0,&[65025],shaders::PROBE_INT_08_OFFSET,shaders::PROBE_INT_08_CODE,shaders::PROBE_INT_08_HEADER,shaders::PROBE_INT_08_GPRS),
        (9,0,&[0xffffffff, 0xffffffff],shaders::PROBE_INT_09_OFFSET,shaders::PROBE_INT_09_CODE,shaders::PROBE_INT_09_HEADER,shaders::PROBE_INT_09_GPRS),
        (9,1,&[0xffffffff, 0xffffffff],shaders::PROBE_INT_09_OFFSET,shaders::PROBE_INT_09_CODE,shaders::PROBE_INT_09_HEADER,shaders::PROBE_INT_09_GPRS),
        (10,0,&[0xffffffff, 0xffffffff, 0xffffffff],shaders::PROBE_INT_10_OFFSET,shaders::PROBE_INT_10_CODE,shaders::PROBE_INT_10_HEADER,shaders::PROBE_INT_10_GPRS),
        (10,1,&[0xffffffff, 0xffffffff, 0xffffffff],shaders::PROBE_INT_10_OFFSET,shaders::PROBE_INT_10_CODE,shaders::PROBE_INT_10_HEADER,shaders::PROBE_INT_10_GPRS),
        (10,2,&[0xffffffff, 0xffffffff, 0xffffffff],shaders::PROBE_INT_10_OFFSET,shaders::PROBE_INT_10_CODE,shaders::PROBE_INT_10_HEADER,shaders::PROBE_INT_10_GPRS),
        (11,0,&[101, 0, 0, 7],shaders::PROBE_INT_11_OFFSET,shaders::PROBE_INT_11_CODE,shaders::PROBE_INT_11_HEADER,shaders::PROBE_INT_11_GPRS),
        (11,3,&[101, 0, 0, 7],shaders::PROBE_INT_11_OFFSET,shaders::PROBE_INT_11_CODE,shaders::PROBE_INT_11_HEADER,shaders::PROBE_INT_11_GPRS),
        (11,0,&[0xffffffff, 0xffffffff, 0xffffffff, 1],shaders::PROBE_INT_11_OFFSET,shaders::PROBE_INT_11_CODE,shaders::PROBE_INT_11_HEADER,shaders::PROBE_INT_11_GPRS),
        (11,1,&[0xffffffff, 0xffffffff, 0xffffffff, 1],shaders::PROBE_INT_11_OFFSET,shaders::PROBE_INT_11_CODE,shaders::PROBE_INT_11_HEADER,shaders::PROBE_INT_11_GPRS),
        (11,2,&[0xffffffff, 0xffffffff, 0xffffffff, 1],shaders::PROBE_INT_11_OFFSET,shaders::PROBE_INT_11_CODE,shaders::PROBE_INT_11_HEADER,shaders::PROBE_INT_11_GPRS),
        (11,3,&[0xffffffff, 0xffffffff, 0xffffffff, 1],shaders::PROBE_INT_11_OFFSET,shaders::PROBE_INT_11_CODE,shaders::PROBE_INT_11_HEADER,shaders::PROBE_INT_11_GPRS),
        (11,4,&[0xffffffff, 0xffffffff, 0xffffffff, 1],shaders::PROBE_INT_11_OFFSET,shaders::PROBE_INT_11_CODE,shaders::PROBE_INT_11_HEADER,shaders::PROBE_INT_11_GPRS),
        (11,5,&[0xffffffff, 0xffffffff, 0xffffffff, 1],shaders::PROBE_INT_11_OFFSET,shaders::PROBE_INT_11_CODE,shaders::PROBE_INT_11_HEADER,shaders::PROBE_INT_11_GPRS),
        (11,0,&[0xffffffff, 0xffffffff, 0xffffffff, 0, 0, 0x80000000],shaders::PROBE_INT_11_OFFSET,shaders::PROBE_INT_11_CODE,shaders::PROBE_INT_11_HEADER,shaders::PROBE_INT_11_GPRS),
        (11,3,&[0xffffffff, 0xffffffff, 0xffffffff, 0, 0, 0x80000000],shaders::PROBE_INT_11_OFFSET,shaders::PROBE_INT_11_CODE,shaders::PROBE_INT_11_HEADER,shaders::PROBE_INT_11_GPRS),
        (11,4,&[0xffffffff, 0xffffffff, 0xffffffff, 0, 0, 0x80000000],shaders::PROBE_INT_11_OFFSET,shaders::PROBE_INT_11_CODE,shaders::PROBE_INT_11_HEADER,shaders::PROBE_INT_11_GPRS),
        (11,5,&[0xffffffff, 0xffffffff, 0xffffffff, 0, 0, 0x80000000],shaders::PROBE_INT_11_OFFSET,shaders::PROBE_INT_11_CODE,shaders::PROBE_INT_11_HEADER,shaders::PROBE_INT_11_GPRS),
        (18,0,&[],shaders::PROBE_INT_18_OFFSET,shaders::PROBE_INT_18_CODE,shaders::PROBE_INT_18_HEADER,shaders::PROBE_INT_18_GPRS),
        (19,0,&[],shaders::PROBE_INT_19_OFFSET,shaders::PROBE_INT_19_CODE,shaders::PROBE_INT_19_HEADER,shaders::PROBE_INT_19_GPRS),
    ];
    for &(routine, word, inputs, offset, code, header, gprs) in vectors {
        let w = command(routine,word,inputs);
        assert_eq!(encode::pipeline_refusal(w.as_slice()),None);
        let f = encode::translate(w.as_slice(), &View(format::B8G8R8A8)).unwrap();
        assert_eq!(f.chunks.len(),1);
        let p = packets(&f.chunks[0].words);
        assert!(p.contains(&(0x2140,vec![0x51,offset as u32])));
        assert!(p.contains(&(0x214c,vec![gprs])));
        assert_eq!(&f.data[0x100000+offset..0x100000+offset+80],header);
        assert_eq!(&f.data[0x100000+offset+80..0x100000+offset+80+code.len()],code);
        let cb = &p.iter().find(|(m,_)| *m == 0x2380).unwrap().1;
        let mut expected = [0u32;20]; expected[..inputs.len()].copy_from_slice(inputs);
        expected[16]=word; expected[17]=routine;
        let bytes: Vec<_> = expected.into_iter().flat_map(u32::to_le_bytes).collect();
        assert_eq!(&f.data[cb[2] as usize..cb[2] as usize+80],bytes);
        let vb = &p.iter().find(|(m,_)| *m == 0x1c00).unwrap().1;
        let mut bytes = Vec::new();
        for [x,y] in [[3,5],[60,5],[60,61],[3,5],[60,61],[3,61]] {
            for v in [x as f32/32.-1.,y as f32/32.-1.,0.,1.,0.,0.,0.,0.] { bytes.extend(v.to_le_bytes()); }
        }
        assert_eq!(&f.data[vb[2] as usize..vb[2] as usize+192],bytes);
    }
}
#[test]
fn deferred_routines_refuse_at_the_probe_command() {
    for routine in (12..=17).chain(20..=24) {
        let w=command(routine,0,&[]);
        assert_eq!(encode::pipeline_refusal(w.as_slice()),Some(stream::Refusal {index:2,error:stream::error::BAD_PIPELINE}));
    }
}

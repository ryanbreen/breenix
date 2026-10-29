//! Literal A197 packets from MIT open-gpu-doc cla197.h at
//! 9fdf5c4062007929d9f4e6cbad9c9771fe61b880: RT 0x800, viewport 0xa00,
//! scissor 0xe00, vertex fetch 0x1c00, programs 0x2040/0x2140, CB 0x2380.
extern crate alloc;
#[allow(dead_code)] // Other committed catalogue programs have their own users in the kernel.
#[path = "../kernel/src/gpu/gk208/gr/shaders.rs"]
mod shaders;
#[path = "../kernel/src/gpu/gk208/encode.rs"]
mod encode;
use gpu_abi::{format, pipeline, stream::{self, Resolver, TargetInfo, SourceInfo}};

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
    fn render_target(&self, h: u64) -> Option<encode::Target> {
        self.target(h).map(|t| encode::Target { id: 19, address: 0x2_0000_0000,
            width: t.width, height: t.height, format: t.format, tile_mode: 0x30 })
    }
}
fn frame() -> Vec<u32> {
    let mut w = Vec::new();
    stream::encode::begin(&mut w, 1);
    stream::encode::draw(&mut w, pipeline::SOLID, 0xff0000ff, &[[0, 0, 32, 32]]);
    // A BGL clip stack emits its current intersection on push and restores it on pop.
    w.extend([stream::cmd::SCISSOR, 6, 8, 12, 48, 56]);
    w.extend([stream::cmd::SCISSOR, 6, 16, 20, 40, 44]);
    stream::encode::draw(&mut w, pipeline::SOLID, 0xff00ff00, &[[0, 0, 64, 64]]);
    w.extend([stream::cmd::SCISSOR, 6, 8, 12, 48, 56]);
    stream::encode::draw(&mut w, pipeline::SOLID, 0xffff0000, &[[32, 32, 64, 64]]);
    w
}
fn packets(words: &[u32]) -> Vec<(u32, Vec<u32>)> {
    let mut p = Vec::new(); let mut at = 0;
    while at < words.len() {
        let h = words[at]; let n = ((h >> 16) & 0x1fff) as usize;
        assert_eq!(h >> 29, 1); assert_eq!((h >> 13) & 7, 0);
        assert!(n > 0 && at + n < words.len());
        p.push(((h & 0x1fff) * 4, words[at+1..at+1+n].to_vec()));
        at += n + 1;
    }
    p
}
#[test]
fn three_solids_bind_native_layout_scissors_programs_and_instance_bytes() {
    for (format, code) in [(format::B8G8R8A8, 0xcf), (format::R8G8B8A8, 0xd5)] {
        let mut f = encode::translate(frame().as_slice(), &View(format)).unwrap();
        f.relocate(0x3_0000_0000).unwrap();
        assert_eq!(f.chunks.len(), 3);
        let scissors = [[0,0,64,64], [16,20,40,44], [8,12,48,56]];
        let rectangles = [[0,0,32,32], [16,20,40,44], [32,32,48,56]];
        let colours = [[1.,0.,0.,1.], [0.,1.,0.,1.], [0.,0.,1.,1.]];
        for (i, chunk) in f.chunks.iter().enumerate() {
            assert_eq!(chunk.target, 19);
            let p = packets(&chunk.words);
            assert!(p.contains(&(0x800, vec![2,0,64,64,code,0x30,1,0,0])));
            assert!(p.contains(&(0xa00, vec![0x42000000,0x42000000,0x3f800000,0x42000000,0x42000000,0])));
            let [x0,y0,x1,y1] = scissors[i];
            assert!(p.contains(&(0xe00, vec![1,x0 | x1<<16,y0 | y1<<16])));
            assert!(p.contains(&(0x1608, vec![3,0x100000])));
            assert!(p.contains(&(0x2040, vec![0x11,0x30])));
            assert!(p.contains(&(0x2140, vec![0x51,0x1b0])));
            assert!(p.contains(&(0x214c, vec![4])));
            assert!(p.contains(&(0x2490, vec![1]))); // fragment constant slot 0
            assert!(p.contains(&(0xd74, vec![0,6])));
            let vb = p.iter().find(|(m,_)| *m == 0x1c00).unwrap().1.clone();
            assert_eq!(vb[0], 0x1020); assert_eq!(vb[1], 3);
            let offset = vb[2] as usize;
            let [x0,y0,x1,y1] = rectangles[i];
            let mut expected = Vec::new();
            for [x,y] in [[x0,y0],[x1,y0],[x1,y1],[x0,y0],[x1,y1],[x0,y1]] {
                for v in [x as f32/32.-1.,y as f32/32.-1.,0.,1.,0.,0.,0.,0.] {
                    expected.extend(v.to_le_bytes());
                }
            }
            assert_eq!(&f.data[offset..offset+192], expected);
            let cb = &p.iter().find(|(m,_)| *m == 0x2380).unwrap().1;
            assert_eq!(&cb[..2], &[256,3]);
            let expected: Vec<u8> = colours[i].into_iter().flat_map(f32::to_le_bytes).collect();
            assert_eq!(&f.data[cb[2] as usize..cb[2] as usize+16], expected);
        }
        assert_eq!(&f.data[0x1001b0..0x100200], shaders::PIPELINE_SOLID_HEADER);
        assert_eq!(&f.data[0x100200..0x100230], shaders::PIPELINE_SOLID_CODE);
    }
}
#[test]
fn invalid_tail_and_unsupported_pipeline_emit_nothing() {
    let mut w = frame(); w.extend([stream::cmd::DRAW, 9, pipeline::SOLID, 0, 1, 9, 2, 1, 3]);
    assert!(encode::translate(w.as_slice(), &View(format::B8G8R8A8)).is_err());
    let mut w = frame(); w.extend([0xffff, 2]);
    assert!(encode::translate(w.as_slice(), &View(format::B8G8R8A8)).is_err());
    let mut w = Vec::new(); stream::encode::begin(&mut w, 1);
    stream::encode::draw(&mut w, pipeline::OVER, 0, &[[0,0,4,4]]);
    assert!(encode::translate(w.as_slice(), &View(format::B8G8R8A8)).is_err());
}

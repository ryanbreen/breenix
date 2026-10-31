//! The shared performance/rect-throughput stream must launch one draw per colour.
extern crate alloc;
#[allow(dead_code)] // The kernel uses the rest of the committed catalogue.
#[path = "../kernel/src/gpu/gk208/gr/shaders.rs"]
mod shaders;
#[path = "../kernel/src/gpu/gk208/encode.rs"]
pub mod encode;
#[path = "../kernel/src/gpu/gk208/caps.rs"]
pub mod caps;
use gpu_abi::{format, pipeline, stream::{self, Resolver, SourceInfo, TargetInfo}};

struct View;
impl Resolver for View {
    fn target(&self, h: u64) -> Option<TargetInfo> {
        (h == 1).then_some(TargetInfo { width: 2560, height: 1600,
            format: format::B8G8R8A8, invalid: false })
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
    fn sampled_source(&self, _: u64) -> Option<encode::Target> { None }
}
fn colour(n: u32) -> u32 {
    0xff000000 | u32::from((n * 37) as u8)
        | u32::from((n * 101 + 17) as u8) << 8 | u32::from((n * 59 + 113) as u8) << 16
}
fn tile(i: usize) -> [i32; 4] {
    let (x, y) = ((i % 160) as i32 * 16, (i / 160) as i32 * 16);
    [x, y, x + 16, y + 16]
}
fn packets(words: &[u32]) -> Vec<(u32, &[u32])> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < words.len() {
        let h = words[at];
        let n = ((h >> 16) & 0x1fff) as usize;
        assert_eq!(h >> 29, 1);
        assert!(n > 0 && at + n < words.len());
        out.push(((h & 0x1fff) * 4, &words[at + 1..at + n + 1]));
        at += n + 1;
    }
    out
}
#[test]
fn exact_rect_throughput_stream_uses_one_draw_and_constant_block_per_command() {
    // userspace/programs/src/suite_gpu/performance.rs::rect_throughput:
    // the background frame precedes the timed submit, which contains only these draws.
    let mut words = Vec::new();
    stream::encode::begin(&mut words, 1);
    for d in 0..100 {
        let rects: Vec<_> = (d * 100..d * 100 + 100).map(tile).collect();
        stream::encode::draw(&mut words, pipeline::SOLID, colour(d as u32), &rects);
    }
    let mut f = encode::translate(words.as_slice(), &View).unwrap();
    let p: Vec<_> = f.chunks.iter().flat_map(|c| packets(&c.words)).collect();
    assert_eq!(p.iter().filter(|(m, _)| *m == 0xd74).count(), 100);
    assert!(p.iter().filter(|(m, _)| *m == 0x110).count() <= 101);
    assert_eq!(f.chunks.len(), 100);
    for (d, c) in f.chunks.iter().enumerate() {
        assert_eq!(c.command, d as u32 + 1);
        let p = packets(&c.words);
        assert_eq!(p.iter().filter(|(m, _)| *m == 0x2380).count(), 1);
        assert_eq!(p.iter().filter(|(m, _)| *m == 0x1c00).count(), 1);
        assert!(p.contains(&(0xd74, &[0, 600][..])));
        let cb = p.iter().find(|(m, _)| *m == 0x2380).unwrap().1[2] as usize;
        let expected: Vec<_> = [16, 8, 0, 24].into_iter()
            .flat_map(|shift| gpu_abi::float::ratio_bits(colour(d as u32) >> shift & 255, 255).to_le_bytes()).collect();
        assert_eq!(&f.data[cb..cb + 16], expected);
        let vb = p.iter().find(|(m, _)| *m == 0x1c00).unwrap().1[2] as usize;
        let limit = p.iter().find(|(m, _)| *m == 0x1f00).unwrap().1;
        assert_eq!(limit, &[0, (vb + 600 * 32 - 1) as u32]);
        for (n, r) in (d * 100..d * 100 + 100).map(tile).enumerate() {
            let [x0, y0, x1, y1] = r;
            let mut expected = Vec::new();
            for [x, y] in [[x0,y0],[x1,y0],[x1,y1],[x0,y0],[x1,y1],[x0,y1]] {
                for (v, size) in [(x, 2560), (y, 1600)] {
                    let n = v * 2 - size;
                    expected.extend((gpu_abi::float::ratio_bits(n.unsigned_abs(), size as u32)
                        | if n < 0 { 1 << 31 } else { 0 }).to_le_bytes());
                }
                expected.extend([0, 0x3f800000u32, 0, 0, 0, 0].into_iter().flat_map(u32::to_le_bytes));
            }
            assert_eq!(&f.data[vb + n * 192..vb + (n + 1) * 192], expected);
        }
    }
    // The normal submit path compacts, packs bounded requests and relocates.
    f.compact().unwrap();
    f.batch_draws(1400).unwrap();
    f.relocate(0x3_0000_0000).unwrap();
    let p: Vec<_> = f.chunks.iter().flat_map(|c| packets(&c.words)).collect();
    assert_eq!(p.iter().filter(|(m, _)| *m == 0xd74).count(), 100);
    assert!(p.iter().filter(|(m, _)| *m == 0x110).count() <= 101);
    assert!(f.chunks.iter().all(|c| c.words.len() <= 1400));
}

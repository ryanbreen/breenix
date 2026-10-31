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
    fn source(&self, h: u64) -> Option<SourceInfo> {
        self.target(h).map(|t| SourceInfo { width: t.width, height: t.height, format: t.format, invalid: false })
    }
    fn buffer_size(&self, h: u64) -> Option<u64> { (h == 2).then_some(48) }
    fn buffer_rect(&self, _: u64, offset: u64) -> [i32; 4] {
        [[0, 0, 4, 4], [4, 4, 20, 20], [2560, 0, 2576, 16]][offset as usize / 16]
    }
}
impl encode::Bindings for View {
    fn render_target(&self, h: u64) -> Option<encode::Target> {
        self.target(h).map(|t| encode::Target { id: 19, address: 0x2_0000_0000,
            width: t.width, height: t.height, format: t.format, tile_mode: 0x30 })
    }
    fn sampled_source(&self, h: u64) -> Option<encode::Target> { self.render_target(h) }
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

#[test]
fn buffered_rectangles_keep_clipping_and_idle_only_at_transfer_or_parking() {
    let mut words = Vec::new();
    stream::encode::begin(&mut words, 1);
    words.extend([stream::cmd::SCISSOR, 6, 2, 2, 10, 10]);
    stream::encode::draw_buffer(&mut words, pipeline::SOLID, colour(0), 2, 0, 3);
    stream::encode::copy(&mut words, 1, [0, 0, 10, 10], 1, [20, 20]);
    stream::encode::draw(&mut words, pipeline::SOLID, colour(1), &[[2,2,4,4], [4,4,8,8]]);
    let f = encode::translate(words.as_slice(), &View).unwrap();
    assert_eq!(f.chunks.len(), 3);
    let p = packets(&f.chunks[0].words);
    assert!(p.contains(&(0xd74, &[0, 12][..])));
    assert_eq!(p.last(), Some(&(0x110, &[0][..])));
    assert!(f.chunks[1].copy.is_some());
    assert!(!packets(&f.chunks[2].words).iter().any(|(m, _)| *m == 0x110));
    let vb = p.iter().find(|(m, _)| *m == 0x1c00).unwrap().1[2] as usize;
    for (i, [x, y]) in [[2,2], [4,2], [4,4], [2,2], [4,4], [2,4],
        [4,4], [10,4], [10,10], [4,4], [10,10], [4,10]].into_iter().enumerate() {
        for (k, (v, size)) in [(x, 2560i32), (y, 1600i32)].into_iter().enumerate() {
            let n = v * 2 - size;
            let expected = gpu_abi::float::ratio_bits(n.unsigned_abs(), size as u32)
                | if n < 0 { 1 << 31 } else { 0 };
            assert_eq!(&f.data[vb + i * 32 + k * 4..vb + i * 32 + k * 4 + 4], expected.to_le_bytes());
        }
    }
    let target = encode::Bindings::render_target(&View, 1).unwrap();
    let source = encode::Target { id: 23, address: 0x2_0100_0000, ..target };
    let (_, parked) = encode::parking_frame(target, source, 0x3_0000_0000, 0x1800_0000).unwrap();
    let p = packets(&parked);
    assert_eq!(p.first(), Some(&(0x110, &[0][..])));
    assert_eq!(p.last(), Some(&(0x110, &[0][..])));
    assert!(!p.iter().any(|(m, _)| *m == 0xd74));
}

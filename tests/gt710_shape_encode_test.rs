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
struct View;
impl Resolver for View {
    fn target(&self, h: u64) -> Option<TargetInfo> { (h == 1).then_some(TargetInfo { width: 64, height: 64, format: format::B8G8R8A8, invalid: false }) }
    fn source(&self, _: u64) -> Option<SourceInfo> { None }
    fn buffer_size(&self, _: u64) -> Option<u64> { None }
    fn buffer_rect(&self, _: u64, _: u64) -> [i32; 4] { panic!("no buffer") }
}
impl native::Bindings for View {
    fn render_target(&self, h: u64) -> Option<native::Target> { self.target(h).map(|t| native::Target { id: 19, address: 0x200000000, width: t.width, height: t.height, format: t.format, tile_mode: 0x30 }) }
    fn sampled_source(&self, _: u64) -> Option<native::Target> { None }
}
impl virgl::encode::Bindings for View {
    fn target(&self, h: u64) -> Option<virgl::encode::TargetRes> { Resolver::target(self, h).map(|info| virgl::encode::TargetRes { id: 19, format: virgl::wire::format::B8G8R8A8_UNORM, info }) }
    fn source(&self, _: u64) -> Option<virgl::encode::SourceRes> { None }
    fn buffer_rect(&self, _: u64, _: u64) -> [i32; 4] { panic!("no buffer") }
    fn buffer_word(&self, _: u64, _: u64) -> u32 { panic!("no buffer") }
}
fn packets(words: &[u32]) -> Vec<(u32, Vec<u32>)> {
    let mut out = Vec::new(); let mut at = 0;
    while at < words.len() { let h = words[at]; let n = ((h >> 16) & 0x1fff) as usize; out.push(((h & 0x1fff)*4, words[at+1..at+1+n].to_vec())); at += n+1; }
    out
}
#[test]
fn shape_constants_programs_and_clipped_quads_match_shared_encoder() {
    let cases = [
        (pipeline::LINE, &[4, 6, 45, 48][..], shaders::PIPELINE_LINE_OFFSET, shaders::PIPELINE_LINE_HEADER, shaders::PIPELINE_LINE_GPRS),
        (pipeline::CIRCLE_FILL, &[25, 25, 18][..], shaders::PIPELINE_CIRCLE_FILL_OFFSET, shaders::PIPELINE_CIRCLE_FILL_HEADER, shaders::PIPELINE_CIRCLE_FILL_GPRS),
        (pipeline::CIRCLE_OUTLINE, &[25, 25, 18][..], shaders::PIPELINE_CIRCLE_OUTLINE_OFFSET, shaders::PIPELINE_CIRCLE_OUTLINE_HEADER, shaders::PIPELINE_CIRCLE_OUTLINE_GPRS),
        (pipeline::ROUNDED_RECT, &[4, 6, 40, 42, 7][..], shaders::PIPELINE_ROUNDED_RECT_OFFSET, shaders::PIPELINE_ROUNDED_RECT_HEADER, shaders::PIPELINE_ROUNDED_RECT_GPRS),
        (pipeline::GRADIENT_V, &[6, 42, 0x21436587][..], shaders::PIPELINE_GRADIENT_V_OFFSET, shaders::PIPELINE_GRADIENT_V_HEADER, shaders::PIPELINE_GRADIENT_V_GPRS),
        (pipeline::TRIANGLE, &[4, 6, 45, 10, 20, 48][..], shaders::PIPELINE_TRIANGLE_OFFSET, shaders::PIPELINE_TRIANGLE_HEADER, shaders::PIPELINE_TRIANGLE_GPRS),
    ];
    for (pipe, params, offset, header, gprs) in cases {
        let mut i = draw::Instance { rect: [0, 0, 60, 60], params: [0; 16] }; i.params[..params.len()].copy_from_slice(params);
        let mut words = Vec::new(); stream::encode::begin(&mut words, 1); words.extend([stream::cmd::SCISSOR, 6, 10, 12, 38, 40]);
        draw::encode(&mut words, pipe, 0x8142a5e7, 0, 0, 0, &[i]);
        assert_eq!(native::pipeline_refusal(words.as_slice()), None);
        let f = native::translate(words.as_slice(), &View).unwrap();
        let packets = packets(&f.chunks[0].words);
        let program = &packets.iter().find(|(m,_)| *m == 0x2140).unwrap().1;
        assert_eq!(program[1] as usize, offset); assert_eq!(&f.data[0x100000+offset..0x100000+offset+80], header);
        assert_eq!(packets.iter().find(|(m,_)| *m == 0x214c).unwrap().1, [gprs]);
        let cb = &packets.iter().find(|(m,_)| *m == 0x2380).unwrap().1;
        let mut enc = virgl::encode::Enc::new();
        virgl::encode::translate(&mut enc, &mut virgl::encode::Objects::new(), words.as_slice(), &View, virgl::encode::Extras { vertex_buffer: 27, scratch: [0;2], data: 0, data_width: gpu_abi::gk208::MAX_2D_SIZE, flip_y: false, corrupt: None, suppress: None, swap: None });
        let mut virgl_constants = Vec::new();
        for chunk in enc.finish().unwrap() { let mut at = 0; while at < chunk.len() { let h = chunk.word(at); let n = (h >> 16) as usize; if h & 255 == virgl::wire::ccmd::SET_CONSTANT_BUFFER { virgl_constants = (at+3..at+1+n).map(|j| chunk.word(j)).collect(); } at += 1+n; } }
        assert_eq!(virgl_constants.len(), 32);
        let actual: Vec<_> = f.data[cb[2] as usize..cb[2] as usize+128].chunks_exact(4).map(|b| u32::from_le_bytes(b.try_into().unwrap())).collect();
        assert_eq!(actual, virgl_constants, "pipeline {pipe}");
        let r = draw::clip(pipe, i, 64, 64, Some([10,12,38,40])).unwrap();
        let vb = &packets.iter().find(|(m,_)| *m == 0x1c00).unwrap().1;
        let [x0,y0,x1,y1] = r; let mut expected = Vec::new();
        for [x,y] in [[x0,y0],[x1,y0],[x1,y1],[x0,y0],[x1,y1],[x0,y1]] { for v in [x as f32/32.-1., y as f32/32.-1., 0., 1., 0., 0., 0., 0.] { expected.extend(v.to_le_bytes()); } }
        assert_eq!(&f.data[vb[2] as usize..vb[2] as usize+192], expected, "pipeline {pipe}");
    }
}

//! Host tests of the real arena allocator and VirGL translator; only DMA storage is replaced.
#![cfg(test)]
extern crate alloc;
const PAGE: usize = 4096;
const POISON: u32 = 0x3C5A_A5C3;
mod gpu {
    pub use crate::virgl;
    pub mod svga { pub use crate::svga_wire as wire; }
    pub mod dma {
        thread_local! { static FAIL: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }
        thread_local! { static PAGES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
        pub fn take_pages() -> usize { PAGES.with(|pages| pages.replace(0)) }
        pub fn fail_allocations(fail: bool) { FAIL.with(|flag| flag.set(fail)); }
        #[derive(Default)]
        pub struct WordBuf(Vec<u32>);
        impl WordBuf {
            pub fn new() -> Self { Self::default() }
            pub fn len(&self) -> usize { self.0.len() }
            pub fn push(&mut self, word: u32) -> bool {
                if FAIL.with(|flag| flag.get()) { return false; }
                if self.0.len() % 1024 == 0 { PAGES.with(|pages| pages.set(pages.get() + 1)); }
                self.0.push(word); true
            }
            pub fn word(&self, index: usize) -> u32 { self.0[index] }
        }
    }
}
#[path = "../../kernel/src/gpu/vertex_arena.rs"]
mod vertex_arena;
pub mod virgl;
use virgl::{encode, wire};

struct View;
impl encode::Bindings for View {
    fn target(&self, handle: u64) -> Option<encode::TargetRes> {
        (handle == 1).then_some(encode::TargetRes { id: 10, format: wire::format::B8G8R8A8_UNORM,
            info: gpu_abi::stream::TargetInfo { width: 64, height: 64, format: gpu_abi::format::B8G8R8A8, invalid: false } })
    }
    fn source(&self, _: u64) -> Option<encode::SourceRes> { None }
    fn buffer_rect(&self, _: u64, _: u64) -> [i32; 4] { panic!("no buffer in fixture") }
    fn buffer_word(&self, _: u64, _: u64) -> u32 { panic!("no crossing table in fixture") }
}
fn extras() -> encode::Extras {
    encode::Extras { vertex_buffer: 27, scratch: [0; 2], data: 0, data_width: 1,
        flip_y: false, corrupt: None, suppress: None, swap: None }
}

#[test]
fn frame_uploads_vertices_before_draws_in_one_bounded_batch_per_chunk() {
    use gpu_abi::stream::encode as bgl;
    for count in [1, 40, 1024, 1025, 2049] {
        let mut words = Vec::new();
        bgl::begin(&mut words, 1);
        for i in 0..count { bgl::draw(&mut words, gpu_abi::pipeline::SOLID, 0xff000000 | i, &[[1, 2, 7, 9]]); }
        let mut sizing = encode::Enc::size_only();
        encode::translate_at(&mut sizing, &mut encode::Objects::new(), words.as_slice(), &View, extras(), 96);
        let mut enc = encode::Enc::new();
        encode::translate_at(&mut enc, &mut encode::Objects::new(), words.as_slice(), &View, extras(), 96);
        let chunks = enc.finish().unwrap();
        if count <= 40 { assert_eq!(chunks.len(), 1, "a small frame split its vertex upload from its draws"); }
        let allocated: usize = chunks.iter().map(|c| (c.len() * 4).div_ceil(PAGE) * PAGE).sum();
        assert_eq!(sizing.backing_bytes(), Some(allocated as u64));
        let (mut uploads, mut vertices, mut draws) = (0, 0, 0);
        for chunk in chunks {
            let mut at = 0;
            while at < chunk.len() {
                let command = chunk.word(at) & 255;
                let len = (chunk.word(at) >> 16) as usize;
                assert!(at + 1 + len <= chunk.len());
                if command == wire::ccmd::RESOURCE_INLINE_WRITE && chunk.word(at + 1) == 27 {
                    assert_eq!(draws, 0, "a vertex upload followed a draw");
                    assert_eq!(chunk.word(at + 3), wire::transfer::WRITE | wire::transfer::DISCARD_RANGE,
                        "reused vertex storage needs a range-discard write");
                    assert_eq!(chunk.word(at + 6), 96 + vertices * 8);
                    let bytes = chunk.word(at + 9);
                    assert_eq!(len, 11 + bytes as usize / 4);
                    assert!(bytes <= 48 * 1024);
                    vertices += bytes / 8;
                    uploads += 1;
                }
                if command == wire::ccmd::DRAW_VBO {
                    assert_eq!(chunk.word(at + 1), 12 + 6 * draws);
                    assert_eq!(chunk.word(at + 2), 6);
                    draws += 1;
                }
                at += 1 + len;
            }
        }
        assert_eq!(draws, count);
        assert_eq!(vertices, count * 6);
        assert_eq!(uploads, count.div_ceil(1024));
    }
}

#[test]
fn failed_vertex_storage_returns_no_command_stream() {
    let mut words = Vec::new();
    gpu_abi::stream::encode::begin(&mut words, 1);
    gpu_abi::stream::encode::draw(&mut words, gpu_abi::pipeline::SOLID, 0xff112233, &[[1, 2, 7, 9]]);
    let mut enc = encode::Enc::new();
    gpu::dma::fail_allocations(true);
    encode::translate_at(&mut enc, &mut encode::Objects::new(), words.as_slice(), &View, extras(), 0);
    gpu::dma::fail_allocations(false);
    assert!(enc.finish().is_none());
}

#[test]
fn clear_only_stream_preserves_its_command_storage() {
    gpu::dma::take_pages();
    let mut words = Vec::new();
    gpu_abi::stream::encode::begin(&mut words, 1);
    gpu_abi::stream::encode::clear(&mut words, 0xff112233);
    let mut enc = encode::Enc::new();
    encode::translate(&mut enc, &mut encode::Objects::new(), words.as_slice(), &View, extras());
    let chunks = enc.finish().unwrap();
    assert_eq!(chunks.len(), 1);
    let first = chunks[0].word(0) & 255;
    assert_ne!(first, wire::ccmd::RESOURCE_INLINE_WRITE);
    assert_eq!(gpu::dma::take_pages(), 1, "clear-only translation allocated extra command storage");
}

#[test]
fn fixed_quads_keep_vertices_identical_and_place_each_rectangle() {
    use gpu_abi::stream::encode as bgl;
    let mut words = Vec::new();
    bgl::begin(&mut words, 1);
    bgl::draw(&mut words, gpu_abi::pipeline::SOLID, 0xff123456,
        &[[1, 2, 7, 9], [17, 13, 35, 37]]);
    let mut sizing = encode::Enc::size_only();
    sizing.fixed_quads();
    encode::translate_at(&mut sizing, &mut encode::Objects::new(), words.as_slice(), &View, extras(), encode::FIXED_QUAD_OFFSET);
    let mut enc = encode::Enc::new();
    enc.fixed_quads();
    encode::translate_at(&mut enc, &mut encode::Objects::new(), words.as_slice(), &View, extras(), encode::FIXED_QUAD_OFFSET);
    let chunks = enc.finish().unwrap();
    let allocated: usize = chunks.iter().map(|c| (c.len()*4).div_ceil(PAGE)*PAGE).sum();
    assert_eq!(sizing.backing_bytes(), Some(allocated as u64));
    let (mut uploads, mut draws, mut viewports) = (0, 0, Vec::new());
    for c in chunks {
        let mut at = 0;
        while at < c.len() {
            let code = c.word(at)&255;
            let len = (c.word(at)>>16) as usize;
            if code == wire::ccmd::RESOURCE_INLINE_WRITE {
                assert_eq!(draws, 0);
                assert_eq!(c.word(at+6), encode::FIXED_QUAD_OFFSET);
                assert_eq!(c.word(at+9), 48);
                let unit:Vec<_>=(at+12..at+24).map(|i|f32::from_bits(c.word(i))).collect();
                assert_eq!(unit, [-1.,-1.,1.,-1.,-1.,1.,1.,-1.,1.,1.,-1.,1.]);
                uploads += 1;
            }
            if code == wire::ccmd::SET_VIEWPORT_STATE {
                viewports.push((at+2..at+8).map(|i|f32::from_bits(c.word(i))).collect::<Vec<_>>());
            }
            if code == wire::ccmd::DRAW_VBO {
                assert_eq!(c.word(at+1), encode::FIXED_QUAD_OFFSET/8);
                assert_eq!(c.word(at+2), 6);
                draws += 1;
            }
            at += 1+len;
        }
    }
    assert_eq!((uploads, draws), (1, 2));
    assert_eq!(&viewports[1..], &[vec![3.,3.5,0.5,4.,5.5,0.5], vec![9.,12.,0.5,26.,25.,0.5]]);
}

#[test]
fn fixed_quads_share_one_unit_upload_across_sampled_and_buffer_draws() {
    struct Mixed;
    impl encode::Bindings for Mixed {
        fn target(&self, h: u64) -> Option<encode::TargetRes> { View.target(h) }
        fn source(&self, h: u64) -> Option<encode::SourceRes> {
            (h == 2).then_some(encode::SourceRes {id: 11, format: wire::format::B8G8R8A8_UNORM, width: 12, height: 6})
        }
        fn buffer_rect(&self, _: u64, _: u64) -> [i32;4] { [1,1,4,4] }
        fn buffer_word(&self, _: u64, _: u64) -> u32 { panic!("no crossing table") }
    }
    use gpu_abi::stream::encode as bgl;
    let mut words = Vec::new();
    bgl::begin(&mut words, 1);
    bgl::draw_sampled(&mut words, gpu_abi::pipeline::COPY, 2, &[[0,0,12,6,0,0]]);
    bgl::draw_buffer(&mut words, gpu_abi::pipeline::SOLID, 0xff0000ff, 3, 0, 1);
    bgl::draw_sampled(&mut words, gpu_abi::pipeline::COPY, 2, &[[4,2,8,5,1,1]]);
    let mut enc = encode::Enc::new();
    enc.fixed_quads();
    encode::translate_at(&mut enc, &mut encode::Objects::new(), words.as_slice(), &Mixed, extras(), encode::FIXED_QUAD_OFFSET);
    let (mut uploads, mut draws, mut viewports) = (0, 0, Vec::new());
    for c in enc.finish().unwrap() {
        let mut at=0;
        while at<c.len() {
            let code=c.word(at)&255;
            let len=(c.word(at)>>16) as usize;
            if code==wire::ccmd::RESOURCE_INLINE_WRITE {
                assert_eq!(draws,0);
                assert_eq!(c.word(at+6),encode::FIXED_QUAD_OFFSET);
                assert_eq!(c.word(at+9),48);
                let unit:Vec<_>=(at+12..at+24).map(|i|f32::from_bits(c.word(i))).collect();
                assert_eq!(unit,[-1.,-1.,1.,-1.,-1.,1.,1.,-1.,1.,1.,-1.,1.]);
                uploads+=1;
            }
            if code==wire::ccmd::SET_VIEWPORT_STATE {
                viewports.push((at+2..at+8).map(|i|f32::from_bits(c.word(i))).collect::<Vec<_>>());
            }
            if code==wire::ccmd::DRAW_VBO {
                assert_eq!(c.word(at+1),encode::FIXED_QUAD_OFFSET/8);
                assert_eq!(c.word(at+2),6);
                draws+=1;
            }
            at+=1+len;
        }
    }
    assert_eq!((uploads,draws),(1,3));
    assert_eq!(&viewports[1..],&[vec![6.,3.,0.5,6.,3.,0.5],vec![1.5,1.5,0.5,2.5,2.5,0.5],vec![2.,1.5,0.5,6.,3.5,0.5]]);
}

#[path = "../../kernel/src/gpu/backend_answers.rs"]
mod backend_answers;
#[path = "../../kernel/src/gpu/svga/wire.rs"]
pub mod svga_wire;

#[test]
fn backend_answers_are_isolated() {
    use gpu_abi::{backend, gk208};
    let rows = [
        (backend::VIRGL, wire::resp::OK_NODATA, wire::cmd::SUBMIT_3D, wire::cmd::TRANSFER_TO_HOST_3D),
        (backend::SVGA3D_DX, svga_wire::status::COMPLETED, svga_wire::cmd::DX_DRAW, svga_wire::cmd::UPDATE_SURFACE),
        (backend::GK208, gk208::SUCCESS, gk208::DRAW_RELEASE, gk208::TRANSFER_RELEASE),
    ];
    for &(number, success, draw, upload) in &rows {
        assert_eq!(backend_answers::success(number), Some(success));
        assert_eq!(backend_answers::draw_kind(number), Some(draw));
        assert_eq!(backend_answers::upload_kind(number), Some(upload));
        for &(other, answer, _, _) in &rows {
            assert_eq!(backend_answers::ok(number, answer), number == other);
        }
    }
    for answer in wire::resp::OK_NODATA..wire::resp::ERR_UNSPEC {
        assert!(backend_answers::ok(backend::VIRGL, answer));
        assert!(!backend_answers::ok(backend::GK208, answer));
    }
    for answer in svga_wire::status::PENDING..=svga_wire::status::PARTIAL_COMPLETE {
        assert!(!backend_answers::ok(backend::GK208, answer));
    }
    assert!(!backend_answers::ok(backend::GK208, gk208::MMU_FAULT_PDE));
    assert_eq!(backend_answers::success(0), None);
    assert_eq!(backend_answers::draw_kind(0), None);
    assert_eq!(backend_answers::upload_kind(0), None);
    assert!(!backend_answers::ok(0, gk208::SUCCESS));
}

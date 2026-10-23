//! Emit Breenix's glyph/sampling sequence for the Linux DRM black-box probe.
//! Usage: vertex-reuse OUTPUT_DIR FONT [VERTEX_USAGE] [--completion-read | --fixed-quads]
//! The directory's `ids` comes from tools/gpu-experiments/vertex-reuse.c.
//! Usage 0 reproduces the old upload; the default uses the production encoder policy.
extern crate alloc;
use std::{collections::BTreeMap, fs};
const PAGE: usize = 4096;
const POISON: u32 = 0x3C5A_A5C3;
mod gpu {

    pub mod dma {
        pub struct WordBuf(pub Vec<u32>);
        impl WordBuf {
            pub fn new() -> Self {
                Self(Vec::new())
            }
            pub fn len(&self) -> usize {
                self.0.len()
            }
            pub fn push(&mut self, w: u32) -> bool {
                self.0.push(w);
                true
            }
            pub fn word(&self, i: usize) -> u32 {
                self.0[i]
            }
        }
    }
}
mod virgl;
use virgl::encode as e;
struct Bind {
    targets: BTreeMap<u64, (u32, u32, u32)>,
    bytes: Vec<u8>,
}
impl e::Bindings for Bind {
    fn target(&self, h: u64) -> Option<e::TargetRes> {
        self.targets.get(&h).map(|&(id, w, h)| e::TargetRes {
            id,
            format: virgl::wire::format::B8G8R8A8_UNORM,
            info: gpu_abi::stream::TargetInfo {
                width: w,
                height: h,
                invalid: false,
                format: gpu_abi::format::B8G8R8A8,
            },
        })
    }
    fn source(&self, h: u64) -> Option<e::SourceRes> {
        self.targets.get(&h).map(|&(id, w, h)| e::SourceRes {
            id,
            format: virgl::wire::format::B8G8R8A8_UNORM,
            width: w,
            height: h,
        })
    }
    fn buffer_rect(&self, h: u64, o: u64) -> [i32; 4] {
        std::array::from_fn(|i| self.buffer_word(h, o + i as u64 * 4) as i32)
    }
    fn buffer_word(&self, _h: u64, o: u64) -> u32 {
        u32::from_le_bytes(self.bytes[o as usize..o as usize + 4].try_into().unwrap())
    }
}
fn save(enc: e::Enc, name: &str, vb: u32, usage: Option<u32>) {
    let chunks = enc.finish().unwrap();
    let mut bytes = Vec::new();
    bytes.extend((chunks.len() as u32).to_le_bytes());
    for c in chunks {
        let mut words: Vec<_> = (0..c.len()).map(|i| c.word(i)).collect();
        let mut at = 0;
        while at < words.len() {
            let len = (words[at] >> 16) as usize;
            if words[at] & 255 == 9 && words[at + 1] == vb {
                if let Some(usage) = usage { words[at + 3] = usage; }
            }
            at += 1 + len;
        }
        bytes.extend((c.len() as u32).to_le_bytes());
        for i in 0..c.len() {
            bytes.extend(words[i].to_le_bytes());
        }
    }
    fs::write(name, bytes).unwrap();
}
fn main() {
    let args: Vec<_> = std::env::args().collect();
    assert!(
        (3..=5).contains(&args.len()),
        "usage: vertex-reuse OUTPUT_DIR FONT [VERTEX_USAGE] [--completion-read | --fixed-quads]"
    );
    let mut usage = None;
    let (mut completion_read, mut fixed_quads) = (false, false);
    for arg in &args[3..] {
        match arg.as_str() {
            "--completion-read" => completion_read = true,
            "--fixed-quads" => fixed_quads = true,
            value => { assert!(usage.is_none(), "duplicate vertex usage"); usage = Some(value.parse::<u32>().unwrap()); }
        }
    }
    assert!(!(completion_read && fixed_quads), "choose one completion policy");
    let dir = &args[1];
    let ids: Vec<u32> = fs::read_to_string(format!("{dir}/ids"))
        .unwrap()
        .split_whitespace()
        .map(|s| s.parse().unwrap())
        .collect();
    // IDs: vertex, scratch, dummy texture, blend scratch, data, six glyph targets, sample.
    assert_eq!(
        ids.len(),
        12,
        "the DRM helper must publish twelve resource IDs"
    );
    let mut o = e::Objects::new();
    let mut enc = e::Enc::new();
    e::warm_base(&mut enc, &mut o, ids[0], ids[1], 1);
    for p in [0, 7, 8] {
        e::warm_pipeline(&mut enc, &mut o, ids[0], ids[1], ids[2], 1, false, p);
    }
    save(enc, &format!("{dir}/warm.bin"), ids[0], usage);

    for frame in 0..20 {
        let size = [8.0, 12.0, 16.0, 24.0, 48.0][frame % 5];
        let font_bytes = fs::read(&args[2]).unwrap();
        let font = libfont::Font::parse(&font_bytes).unwrap();
        let mut tables = Vec::new();
        let mut glyphs = Vec::new();
        let mut off = 0;
        for ch in "Brenix".chars() {
            let g = font.glyph_index(ch);
            let outline = font.glyph_outline(g).unwrap();
            if let libfont::GlyphLayout::Outline {
                segments,
                width,
                height,
                ..
            } = font
                .layout_glyph(&outline, g, libfont::Font::size26(size).unwrap())
                .unwrap()
            {
                let t = libfont::crossings::CrossingTable::new(&segments, 0, height);
                let expected =
                    libgfx_ref::rasterizer::rasterize(&segments, width, height, 0, 0).unwrap();
                glyphs.push((ch, width, height, off, expected));
                off += t.bytes().len();
                tables.push(t);
            }
        }
        let packed = libfont::crossings::CrossingTable::pack(&tables.iter().collect::<Vec<_>>());
        let mut b = Bind {
            targets: BTreeMap::new(),
            bytes: packed.bytes().to_vec(),
        };
        let mut words = Vec::new();
        let mut expected = Vec::new();
        b.targets.insert(99, (ids[11], 128, 128));
        gpu_abi::stream::encode::begin(&mut words, 99);
        gpu_abi::stream::encode::clear(&mut words, 0xff000000);
        gpu_abi::stream::encode::draw(&mut words, 0, 0xff123456, &[[0, 0, 16, 16]]);
        for (n, (_, w, h, at, reference)) in glyphs.iter().enumerate() {
            let oy = (n + 1) * 8;
            b.targets.insert(n as u64 + 10, (ids[n + 5], 64, 128));
            gpu_abi::stream::encode::begin(&mut words, n as u64 + 10);
            gpu_abi::stream::encode::clear(&mut words, 0);
            let mut params = [0; 16];
            params[..5].copy_from_slice(&[0, oy as u32, *w as u32, *h as u32, *at as u32]);
            gpu_abi::draw::encode(
                &mut words,
                8,
                0xffffffff,
                0,
                1,
                off as u32,
                &[gpu_abi::draw::Instance {
                    rect: [0, oy as i32, *w as i32, (oy + h) as i32],
                    params,
                }],
            );
            let mut pixels = vec![0u8; 64 * 128 * 4];
            for y in 0..*h {
                for x in 0..*w {
                    for c in 0..4 {
                        pixels[((y + oy) * 64 + x) * 4 + c] = reference.coverage[y * w + x];
                    }
                }
            }
            expected.extend(pixels);
        }
        let mut enc = e::Enc::new();
        if fixed_quads { enc.fixed_quads(); }
        e::translate_at(
            &mut enc,
            &mut o,
            words.as_slice(),
            &b,
            e::Extras {
                vertex_buffer: ids[0],
                scratch: [ids[3], 0],
                data: ids[4],
                data_width: 16384,
                flip_y: false,
                corrupt: None,
                suppress: None,
                swap: None,
            },
            if fixed_quads { e::FIXED_QUAD_OFFSET } else { 0 },
        );
        if completion_read { enc.completion_read(ids[10], 256); }
        save(
            enc,
            &format!("{dir}/frame-{frame}-glyph.bin"),
            ids[0],
            usage,
        );
        fs::write(format!("{dir}/frame-{frame}-expected.bin"), expected).unwrap();
        let mut samples = Vec::new();
        gpu_abi::stream::encode::begin(&mut samples, 99);
        gpu_abi::stream::encode::clear(&mut samples, 0);
        let mut sample_expected = vec![0u8; 128 * 128 * 4];
        for (n, (_, w, h, _, reference)) in glyphs.iter().enumerate() {
            let (x, y) = ((n % 3) * 40, (n / 3) * 64);
            let mut params = [0; 16];
            params[..5].copy_from_slice(&[x as u32, y as u32, 0, ((n + 1) * 8) as u32, 1]);
            gpu_abi::draw::encode(
                &mut samples,
                7,
                0xffffffff,
                n as u64 + 10,
                0,
                0,
                &[gpu_abi::draw::Instance {
                    rect: [x as i32, y as i32, (x + w) as i32, (y + h) as i32],
                    params,
                }],
            );
            for yy in 0..*h {
                for xx in 0..*w {
                    for c in 0..3 {
                        sample_expected[((yy + y) * 128 + xx + x) * 4 + c] =
                            reference.coverage[yy * w + xx];
                    }
                }
            }
        }
        fs::write(format!("{dir}/frame-{frame}-sampled.bin"), sample_expected).unwrap();
        let mut enc = e::Enc::new();
        if fixed_quads { enc.fixed_quads(); }
        e::translate_at(
            &mut enc,
            &mut o,
            samples.as_slice(),
            &b,
            e::Extras {
                vertex_buffer: ids[0],
                scratch: [ids[3], 0],
                data: 0,
                data_width: 16384,
                flip_y: false,
                corrupt: None,
                suppress: None,
                swap: None,
            },
            if fixed_quads { e::FIXED_QUAD_OFFSET } else { 0 },
        );
        if completion_read { enc.completion_read(ids[11], 512); }
        save(
            enc,
            &format!("{dir}/frame-{frame}-sample.bin"),
            ids[0],
            usage,
        );
        println!(
            "Encoded frame {frame}, {} glyphs, {} bytes",
            glyphs.len(),
            off
        );
    }
}

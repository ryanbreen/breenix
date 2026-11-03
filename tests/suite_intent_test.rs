use super::intent::{layout, samples};
use crate::gpu::abi::{self, stream::{self, Resolver, SourceInfo, TargetInfo}};

struct IntentResources { picture: [u32; 2], mode: [u32; 2] }
impl Resolver for IntentResources {
    fn target(&self, handle: u64) -> Option<TargetInfo> {
        (handle == 1).then_some(TargetInfo { width: self.mode[0], height: self.mode[1],
            invalid: false, format: abi::format::B8G8R8A8 })
    }
    fn source(&self, handle: u64) -> Option<SourceInfo> {
        (handle == 2).then_some(SourceInfo { width: self.picture[0], height: self.picture[1],
            invalid: false, format: abi::format::B8G8R8A8 })
    }
    fn buffer_size(&self, _: u64) -> Option<u64> { None }
    fn buffer_rect(&self, _: u64, _: u64) -> [i32; 4] { unreachable!() }
}

#[test]
fn intent_commands_fit_the_existing_gpu_work_budget() {
    for mode in [[1280, 800], [2560, 1600]] {
        for picture in [[1, 1], [17, 9], [1216, 736], [1217, 737], [1280, 800], [2560, 1600]] {
            let resources = IntentResources { picture, mode };
            let mut area = 0;
            for words in super::intent::streams(picture, mode, 1, 2).unwrap() {
                let plan = stream::validate(&words[..], &resources)
                    .unwrap_or_else(|e| panic!("picture {picture:?} mode {mode:?}: {e:?}"));
                area += plan.area;
            }
            let l = layout(picture, mode).unwrap();
            let border = l.border.map_or(0, |r| r[2] as u64 * r[3] as u64);
            let mut light_area = 0;
            for y in (0..mode[1]).step_by(16) {
                for x in (0..mode[0]).step_by(16) {
                    if (x / 16 + y / 16) % 2 == 1 {
                        light_area += u64::from(16.min(mode[0] - x))
                            * u64::from(16.min(mode[1] - y));
                    }
                }
            }
            assert_eq!(area, mode[0] as u64 * mode[1] as u64 + light_area + border
                + l.picture[2] as u64 * l.picture[3] as u64);
        }
    }
}

#[test]
fn intent_background_alternates_at_frame_origin_behind_border_and_picture() {
    const PICTURE: u32 = 0xFF12_3456;
    for (picture, mode) in [
        ([17, 9], [1280, 800]),
        ([1, 1], [1301, 815]),
        ([2560, 1600], [1280, 800]),
        ([1280, 800], [1280, 800]),
        ([17, 9], [2560, 1600]),
    ] {
        let mut pixels = vec![0; (mode[0] * mode[1]) as usize];
        for words in super::intent::streams(picture, mode, 1, 2).unwrap() {
            stream::validate(&words[..], &IntentResources { picture, mode }).unwrap();
            let mut at = 0;
            while at < words.len() {
                let command = &words[at..at + words[at + 1] as usize];
                let mut paint = |rect: &[u32], colour| {
                    for y in rect[1]..rect[3] {
                        for x in rect[0]..rect[2] {
                            pixels[(y * mode[0] + x) as usize] = colour;
                        }
                    }
                };
                match command[0] {
                    stream::cmd::BEGIN => assert_eq!(&command[2..], &[1, 0]),
                    stream::cmd::CLEAR => paint(&[0, 0, mode[0], mode[1]], command[2]),
                    stream::cmd::DRAW => {
                        assert_eq!(command[2], abi::pipeline::SOLID);
                        for rect in command[5..].chunks_exact(4) { paint(rect, command[3]); }
                    }
                    abi::draw::COMMAND => {
                        assert_eq!(command[2], abi::pipeline::NEAREST_SCALED);
                        for instance in command[abi::draw::HEADER..].chunks_exact(abi::draw::INSTANCE) {
                            paint(&instance[..4], PICTURE);
                        }
                    }
                    other => panic!("unexpected intent command {other}"),
                }
                at += command.len();
            }
        }
        let l = layout(picture, mode).unwrap();
        let contains = |[left, top, width, height]: [u32; 4], x, y|
            x >= left && x < left + width && y >= top && y < top + height;
        for y in 0..mode[1] {
            for x in 0..mode[0] {
                let expected = if contains(l.picture, x, y) { PICTURE }
                    else if l.border.is_some_and(|b| contains(b, x, y)) { 0xFFFF_FFFF }
                    else if (x / 16 + y / 16) % 2 == 0 { 0xFF5A_5A5A }
                    else { 0xFF7A_7A7A };
                assert_eq!(pixels[(y * mode[0] + x) as usize], expected,
                    "picture {picture:?} mode {mode:?} pixel ({x},{y})");
            }
        }
    }
}

#[test]
fn intent_layout_covers_scale_full_screen_and_integer_decimation() {
    // Mode 1280x800 has a 1216x736 inner bound. Rectangles are x,y,width,height.
    let cases = [
        ([1u32, 1], 736u32, 1u32, [272, 32, 736, 736], Some([270, 30, 740, 740])),
        ([17, 9], 71, 1, [36, 80, 1207, 639], Some([34, 78, 1211, 643])),
        ([1216, 736], 1, 1, [32, 32, 1216, 736], Some([30, 30, 1220, 740])),
        ([1217, 736], 1, 2, [335, 216, 609, 368], Some([333, 214, 613, 372])),
        ([1216, 737], 1, 2, [336, 215, 608, 369], Some([334, 213, 612, 373])),
        ([1280, 800], 1, 1, [0, 0, 1280, 800], None),
        ([2560, 1600], 1, 3, [213, 133, 854, 534], Some([211, 131, 858, 538])),
    ];
    for (picture, scale, divisor, rect, border) in cases {
        let got = layout(picture, [1280, 800]).unwrap();
        assert_eq!((got.scale, got.divisor, got.picture, got.border),
            (scale, divisor, rect, border), "picture {picture:?}");
        if border.is_some() {
            assert!(rect[2] <= 1216 && rect[3] <= 736);
            if divisor > 1 {
                assert!(picture[0].div_ceil(divisor - 1) > 1216
                    || picture[1].div_ceil(divisor - 1) > 736);
            }
        }
    }
    let full = layout([2560, 1600], [2560, 1600]).unwrap();
    assert_eq!((full.picture, full.border), ([0, 0, 2560, 1600], None));
    assert!(layout([0, 1], [1280, 800]).is_err());
    assert!(layout([1, 1], [64, 800]).is_err());
}

#[test]
fn downscale_samples_every_kth_pixel_including_partial_edge_tiles() {
    let picture = [2560, 1600];
    let layout = layout(picture, [1280, 800]).unwrap();
    let tiles = samples(picture, layout);
    // Check the source-coordinate formula used by NEAREST_SCALED for this 854x534 output,
    // including odd dimensions: proportional scaling of the whole source is wrong.
    for y in 0..layout.picture[3] {
        for x in 0..layout.picture[2] {
            let px = x + layout.picture[0];
            let py = y + layout.picture[1];
            let hits: Vec<_> = tiles.iter().filter(|(dst, _)|
                px >= dst[0] && px < dst[0] + dst[2]
                && py >= dst[1] && py < dst[1] + dst[3]).collect();
            assert_eq!(hits.len(), 1);
            let (dst, src) = hits[0];
            let sx = src[0] + (px - dst[0]) * src[2] / dst[2];
            let sy = src[1] + (py - dst[1]) * src[3] / dst[3];
            assert_eq!([sx, sy], [x * 3, y * 3]);
            assert!(src[0] + src[2] <= picture[0] && src[1] + src[3] <= picture[1]);
        }
    }
}

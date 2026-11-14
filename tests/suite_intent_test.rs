use super::intent::{layout, samples};
use crate::gpu::abi::{self, stream::{self, Resolver, SourceInfo, TargetInfo}};

#[test]
fn start_record_marks_only_configured_screen_suites() {
    for id in ["gpu", "breengel", "console", "crypto", "directories", "files-io", "filesystem", "gt710",
        "ipc", "libc", "memory", "networking", "processes", "rust-std", "sdk", "server", "signals",
        "smoke", "terminals", "threads", "time", "toolchain", "ui-testing", "users", "utilities",
        "window-manager", "other"] {
        for configured in [false, true] {
            let old = format!("SUITE {id} START cases=12 only=case pace_ms=50");
            let expected = if configured && matches!(id, "gpu" | "breengel") {
                format!("{old} screen_proof=on")
            } else { old };
            assert_eq!(super::start_record(id, 12, " only=case", " pace_ms=50", configured), expected);
        }
    }
}

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
            for page in layout(picture, mode, &[]).unwrap() {
            for words in super::intent::streams(picture, mode, 1, 2, page).unwrap() {
                let plan = stream::validate(&words[..], &resources)
                    .unwrap_or_else(|e| panic!("picture {picture:?} mode {mode:?}: {e:?}"));
                area += plan.area;
            }
            }
            let plans = super::intent::layout(picture, mode, &[]).unwrap();
            let border: u64 = plans.iter().map(|l| l.border.map_or(0, |r| r[2] as u64 * r[3] as u64)).sum();
            let picture_area: u64 = plans.iter().flat_map(|l| samples(picture,l.clone())).map(|(r,_)| r[2] as u64*r[3] as u64).sum();
            let mut light_area = 0;
            for y in (0..mode[1]).step_by(16) {
                for x in (0..mode[0]).step_by(16) {
                    if (x / 16 + y / 16) % 2 == 1 {
                        light_area += u64::from(16.min(mode[0] - x))
                            * u64::from(16.min(mode[1] - y));
                    }
                }
            }
            assert_eq!(area, plans.len() as u64 * (mode[0] as u64 * mode[1] as u64 + light_area) + border + picture_area);
        }
    }
}

#[test]
fn intent_background_alternates_at_frame_origin_behind_border_and_picture() {
    const PICTURE: u32 = 0xFF12_3456;
    for (picture, mode) in [
        ([1872, 1788], [2560, 1600]),
        ([17, 9], [1280, 800]),
        ([1, 1], [1301, 815]),
        ([2560, 1600], [1280, 800]),
        ([1280, 800], [1280, 800]),
        ([17, 9], [2560, 1600]),
    ] {
        let mut pixels = vec![0; (mode[0] * mode[1]) as usize];
        for l in if picture == [1872,1788] {
            super::intent::picture_pages("breengel","png-decode/independent-fixtures",picture,mode).unwrap()
        } else { super::intent::layout(picture, mode, &[]).unwrap() } {
        for words in super::intent::streams(picture, mode, 1, 2, l.clone()).unwrap() {
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
        let mut picture_pixels=vec![false;(mode[0]*mode[1]) as usize];
        for (dst,_) in samples(picture,l.clone()) {
            for y in dst[1]..dst[1]+dst[3] { for x in dst[0]..dst[0]+dst[2] {
                picture_pixels[(y*mode[0]+x) as usize]=true;
            } }
        }
        let contains = |[left, top, width, height]: [u32; 4], x, y|
            x >= left && x < left + width && y >= top && y < top + height;
        for y in 0..mode[1] {
            for x in 0..mode[0] {
                let expected = if picture_pixels[(y*mode[0]+x) as usize] { PICTURE }
                    else if contains(l.picture, x, y) { 0xFF0C_131F }
                    else if l.border.is_some_and(|b| contains(b, x, y)) { 0xFFFF_FFFF }
                    else if (x / 16 + y / 16) % 2 == 0 { 0xFF5A_5A5A }
                    else { 0xFF7A_7A7A };
                assert_eq!(pixels[(y * mode[0] + x) as usize], expected,
                    "picture {picture:?} mode {mode:?} pixel ({x},{y})");
            }
        }
        }
    }
}

#[test]
fn intent_layout_preserves_enlargement_and_full_screen() {
    for (picture, scale, rect, border) in [
        ([1u32, 1], 736u32, [272, 32, 736, 736], Some([270, 30, 740, 740])),
        ([17, 9], 71, [36, 80, 1207, 639], Some([34, 78, 1211, 643])),
        ([1216, 736], 1, [32, 32, 1216, 736], Some([30, 30, 1220, 740])),
        ([1217, 736], 1, [31, 32, 1217, 736], Some([29, 30, 1221, 740])),
        ([1216, 737], 1, [32, 31, 1216, 737], Some([30, 29, 1220, 741])),
        ([1280, 800], 1, [0, 0, 1280, 800], None),
    ] {
        let got = layout(picture, [1280, 800], &[]).unwrap().remove(0);
        assert_eq!((got.scale, got.picture, got.border), (scale, rect, border));
    }
    assert!(layout([0, 1], [1280, 800], &[]).is_err());
    assert!(layout([1, 1], [64, 800], &[]).is_err());
}

#[test]
fn native_intent_layout_keeps_full_and_folded_pictures_lossless() {
    for (suite, case, picture, mode, displayed, border) in [
        ("gpu", "present/flip", [1280, 1024], [1280, 1024], [1280, 1024], false),
        ("breengel", "composition/compose-position", [2560, 2048], [2560, 2048], [2560, 2048], false),
        ("gpu", "profile/int-float-exact", [16384, 2], [1280, 1024], [1276, 26], true),
        ("gpu", "profile/int-float-exact", [16384, 2], [2560, 1600], [2556, 14], true),
        ("breengel", "bmp/padding", [1446, 1553], [2560, 1600], [1446, 1553], true),
        ("breengel", "bmp/padding", [1446, 1553], [2560, 2048], [1446, 1553], true),
        ("breengel", "panel/kernel-screens", [1280, 1024], [2560, 2048], [1280, 1024], true),
    ] {
        let mut plans = super::intent::picture_pages(suite, case, picture, mode).unwrap();
        assert_eq!(plans.len(), 1, "{suite}/{case} {mode:?}");
        let plan = plans.remove(0);
        assert_eq!(plan.scale, 1);
        assert_eq!(plan.picture, [(mode[0]-displayed[0])/2, (mode[1]-displayed[1])/2,
            displayed[0], displayed[1]]);
        assert_eq!(plan.border.is_some(), border);
        if let Some([x, y, w, h]) = plan.border {
            assert_eq!([x+2, y+2, w-4, h-4], plan.picture);
            assert!(x+w <= mode[0] && y+h <= mode[1]);
        }
        let mut hits = vec![0u8; (picture[0]*picture[1]) as usize];
        let mut occupied = vec![false; (mode[0]*mode[1]) as usize];
        for (dst, src) in samples(picture, plan) {
            assert_eq!(dst[2..], src[2..]);
            assert!(dst[0]+dst[2] <= mode[0] && dst[1]+dst[3] <= mode[1]);
            for y in 0..src[3] { for x in 0..src[2] {
                let at = ((dst[1]+y)*mode[0]+dst[0]+x) as usize;
                assert!(!occupied[at], "{suite}/{case}: overprint");
                occupied[at] = true;
                hits[((src[1]+y)*picture[0]+src[0]+x) as usize] += 1;
            } }
        }
        assert!(hits.iter().all(|&n| n == 1), "{suite}/{case}: source union");
    }
}

#[test]
fn panel_pages_keep_tile_boundaries_and_exact_source_union() {
    for (case, size, page_sizes) in [
        ("png-decode/interlace-small", [2022,2083], vec![[2490,1490], [1710,295]]),
        ("png-decode/ancillary-ignored", [2160,1748], vec![[2160,874], [2160,874]]),
        ("png-decode/independent-fixtures", [1872,1788], vec![[2496,1490]]),
        ("png-encode/sizes", [1322,1460], vec![[1322,1460]]),
    ] {
        let plans = super::intent::picture_pages("breengel", case, size, [2560,1600]).unwrap();
        assert_eq!(plans.iter().map(|p| [p.picture[2],p.picture[3]]).collect::<Vec<_>>(),page_sizes);
        let mut hits = vec![0u8;(size[0]*size[1]) as usize];
        for p in plans {
            assert_eq!(p.scale,1);
            let b = p.border.unwrap();
            assert!(b[0]+b[2]<=2560 && b[1]+b[3]<=1600);
            for (dst,src) in samples(size,p) {
                assert_eq!(dst[2..],src[2..]);
                for y in src[1]..src[1]+src[3] { for x in src[0]..src[0]+src[2] {
                    hits[(y*size[0]+x) as usize]+=1;
                } }
                if case == "png-decode/independent-fixtures" {
                    assert_eq!(src[2..],[156,298]); // Whole 32 px fixture pairs, labels and padding.
                }
            }
        }
        assert!(hits.iter().all(|&n| n==1),"{case}: source union");
    }
}

// U11 / #2678: inspect the submitted mapping, independently of layout metadata.
// Assert integer destination blocks and a source hit count of one per texel.
#[test]
fn oversized_intents_preserve_every_source_pixel_without_overprinting() {
    for (suite,case,picture,expected_pages) in [
        ("gpu","profile/int-float-exact",[16384,2],1),
        ("gpu","performance/upload-bandwidth",[2048,2048],2),
        ("breengel","bmp/padding",[1446,1553],1),
        ("breengel","png-decode/interlace-small",[2022,2083],2),
        ("breengel","png-decode/ancillary-ignored",[2160,1748],2),
        ("breengel","png-decode/independent-fixtures",[1872,1788],1),
        ("breengel","png-encode/sizes",[1322,1460],1),
    ] {
        let mode = [2560, 1600];
        let mut source_hits = vec![0u8; (picture[0] * picture[1]) as usize];
        let mut occupied = vec![false; (mode[0] * mode[1]) as usize];
        let mut pages = 0;
        let plans=super::intent::picture_pages(suite,case,picture,mode).unwrap();
        for plan in plans {
        for words in super::intent::streams(picture,mode,1,2,plan).unwrap() {
            stream::validate(&words[..], &IntentResources { picture, mode }).unwrap();
            let mut at = 0;
            while at < words.len() {
                let command = &words[at..at + words[at + 1] as usize];
                if command[0] == stream::cmd::CLEAR {
                    occupied.fill(false);
                    pages += 1;
                }
                if command[0] == abi::draw::COMMAND {
                    for instance in command[abi::draw::HEADER..].chunks_exact(abi::draw::INSTANCE) {
                        let p = &instance[4..];
                        let (dw, dh, sw, sh) = (p[2], p[3], p[6], p[7]);
                        assert!(dw >= sw && dh >= sh && dw % sw == 0 && dh % sh == 0,
                            "{picture:?}: source pixels must have integer scale >= 1");
                        assert_eq!(dw / sw, dh / sh);
                        let scale = dw / sw;
                        if picture == [2022, 2083] || picture == [2048, 2048] {
                            assert_eq!(scale, 1, "paged pictures stay at scale 1");
                        }
                        for y in instance[1]..instance[3] {
                            for x in instance[0]..instance[2] {
                                assert!(x >= 2 && y >= 2 && x < mode[0]-2 && y < mode[1]-2);
                                let dst = (y * mode[0] + x) as usize;
                                assert!(!occupied[dst], "{picture:?}: destination overprint");
                                occupied[dst] = true;
                                if (x-p[0]) % scale == 0 && (y-p[1]) % scale == 0 {
                                    let sx = p[4] + (x-p[0]) / scale;
                                    let sy = p[5] + (y-p[1]) / scale;
                                    source_hits[(sy * picture[0] + sx) as usize] += 1;
                                }
                            }
                        }
                    }
                }
                at += command.len();
            }
        }
        }
        assert!(source_hits.iter().all(|&n| n == 1), "{picture:?}: page union differs from judged picture");
        assert_eq!(pages,expected_pages);
    }
}

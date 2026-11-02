use super::intent::{layout, samples};

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

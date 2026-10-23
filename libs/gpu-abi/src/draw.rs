//! Parameterized catalogue draws (G5 contract PR #1714).
use crate::{
    pipeline as p,
    stream::{self, SourceInfo},
};

pub const COMMAND: u32 = 9;
pub const HEADER: usize = 10;
pub const INSTANCE: usize = 20;
pub const OVERLAP: u32 = 20;
pub const PARAMETERS: u32 = 21;
/// Sampled colour flags: source X is opaque, and destination X must be opaque.
/// OVER/OVER_STRAIGHT use both; COPY_SCALED uses SOURCE_OPAQUE to force output X.
pub const SOURCE_OPAQUE: u32 = 1;
pub const DESTINATION_OPAQUE: u32 = 2;
// Parameterized COPY uses [source_offset_x, source_offset_y, force_opaque].

#[derive(Clone, Copy, Debug)]
pub struct Instance {
    pub rect: [i32; 4],
    pub params: [u32; 16],
}

pub const fn blended(pipe: u32) -> bool {
    matches!(
        pipe,
        p::OVER | p::OVER_STRAIGHT | p::COVERAGE | p::COVERAGE_SPANS | p::BITMAP_TEXT
    )
}
pub const fn sampled(pipe: u32) -> bool {
    matches!(
        pipe,
        p::COPY | p::COPY_SCALED
            | p::NEAREST_SCALED
            | p::BOX_FILTER
            | p::OVER
            | p::OVER_STRAIGHT
            | p::COVERAGE
            | p::BITMAP_TEXT
    )
}
pub const fn steps(pipe: u32) -> Option<u64> {
    match pipe {
        p::COPY => Some(1),
        p::COPY_SCALED | p::NEAREST_SCALED => Some(8192),
        p::BOX_FILTER => Some(512),
        p::OVER | p::OVER_STRAIGHT => Some(128),
        p::COVERAGE | p::BITMAP_TEXT => Some(256),
        p::CIRCLE_FILL | p::CIRCLE_OUTLINE => Some(2048),
        p::GRADIENT_V => Some(32768),
        p::ROUNDED_RECT | p::TRIANGLE => Some(4096),
        p::LINE => Some(8192),
        p::COVERAGE_SPANS => Some(8192),
        _ => None,
    }
}
pub fn overlap(a: [u32; 4], b: [u32; 4]) -> bool {
    a[0] < b[2] && b[0] < a[2] && a[1] < b[3] && b[1] < a[3]
}
/// Refuse unused operands and domains before device work. Signed sizes are permitted
/// for shapes: nonpositive sizes have the contract's empty-set meaning.
pub fn valid(pipe: u32, a: &[u32; 16], tex: Option<SourceInfo>) -> bool {
    let used = match pipe {
        p::COPY => 3,
        p::COPY_SCALED => 5,
        p::NEAREST_SCALED | p::BOX_FILTER => 8,
        p::OVER | p::OVER_STRAIGHT => 4,
        p::COVERAGE => 5,
        p::COVERAGE_SPANS => 5,
        p::BITMAP_TEXT => 8,
        p::LINE => 4,
        p::CIRCLE_FILL | p::CIRCLE_OUTLINE => 3,
        p::ROUNDED_RECT => 5,
        p::GRADIENT_V => 3,
        p::TRIANGLE => 6,
        _ => return false,
    };
    if a[used..].iter().any(|&x| x != 0) {
        return false;
    }
    match pipe {
        p::COPY => a[2] <= 1,
        p::COPY_SCALED => a[4] > 0 && a[4] <= i32::MAX as u32,
        p::NEAREST_SCALED | p::BOX_FILTER => {
            a[2] > 0
                && a[3] > 0
                && a[2] <= 16384
                && a[3] <= 16384
                && a[6] > 0
                && a[7] > 0
                && tex.is_some_and(|t| a[6] <= t.width && a[7] <= t.height)
                && (pipe != p::BOX_FILTER || (a[6] >= a[2] && a[7] >= a[3]))
        }
        p::COVERAGE => {
            a[4] <= 1 && tex.is_some_and(|t| (a[4] == 0) == (t.format == crate::format::R8))
        }
        p::BITMAP_TEXT => {
            a[4] > 0
                && a[4] <= 16384
                && a[5] > 0
                && a[5] <= 16384
                && a[6] > 0
                && a[6] <= i32::MAX as u32
                && a[7] <= 1
                && (a[7] != 0 || a[4] <= 8)
                && tex.is_some_and(|t| t.format == crate::format::R8)
        }
        p::COVERAGE_SPANS => {
            a[2] > 0 && a[2] <= 16384 && a[3] > 0 && a[3] <= 16384 && a[4] % 4 == 0
        }
        _ => true,
    }
}
/// Shader execution is bounded to the target and caller clip. Additional source and
/// shape predicates are evaluated on the device; these bounds describe geometry.
pub fn clip(
    pipe: u32,
    i: Instance,
    width: u32,
    height: u32,
    scissor: Option<[i32; 4]>,
) -> Option<[u32; 4]> {
    let a = i.params.map(|v| v as i32 as i64);
    let bound = match pipe {
        p::LINE => Some([
            a[0].min(a[2]),
            a[1].min(a[3]),
            a[0].max(a[2]) + 1,
            a[1].max(a[3]) + 1,
        ]),
        p::CIRCLE_FILL | p::CIRCLE_OUTLINE => {
            if a[2] < 0 {
                return None;
            }
            Some([a[0] - a[2], a[1] - a[2], a[0] + a[2] + 1, a[1] + a[2] + 1])
        }
        p::ROUNDED_RECT => {
            if a[2] <= 0 || a[3] <= 0 {
                return None;
            }
            Some([a[0], a[1], a[0] + a[2], a[1] + a[3]])
        }
        p::GRADIENT_V => {
            if a[1] <= 0 {
                return None;
            }
            Some([0, a[0], width as i64, a[0] + a[1]])
        }
        p::TRIANGLE => Some([
            a[0].min(a[2]).min(a[4]),
            a[1].min(a[3]).min(a[5]),
            a[0].max(a[2]).max(a[4]) + 1,
            a[1].max(a[3]).max(a[5]) + 1,
        ]),
        _ => None,
    };
    let mut r = i.rect.map(i64::from);
    if let Some(b) = bound {
        r = [
            r[0].max(b[0]),
            r[1].max(b[1]),
            r[2].min(b[2]),
            r[3].min(b[3]),
        ];
    }
    if r[2] <= r[0] || r[3] <= r[1] {
        return None;
    }
    stream::clipped(
        r.map(|v| v.clamp(i32::MIN as i64, i32::MAX as i64) as i32),
        width,
        height,
        scissor,
    )
}

/// Intersect with the source footprint using wide geometry before any coordinate
/// division. A surviving copy_scaled fetch is in range even if its intermediate
/// two's-complement coordinate wraps a 32-bit word.
pub fn source_clip(pipe: u32, mut i: Instance, source: Option<SourceInfo>) -> Option<Instance> {
    let a = i.params.map(|v| v as i32 as i128);
    let bound = match pipe {
        p::COPY => {
            let t = source?;
            Some([-a[0], -a[1], t.width as i128 - a[0], t.height as i128 - a[1]])
        }
        p::COPY_SCALED => {
            let t = source?;
            let n = a[4];
            Some([
                a[0] - a[2] * n,
                a[1] - a[3] * n,
                a[0] + (t.width as i128 - a[2]) * n,
                a[1] + (t.height as i128 - a[3]) * n,
            ])
        }
        p::OVER | p::OVER_STRAIGHT | p::COVERAGE => {
            let t = source?;
            Some([
                a[0] - a[2],
                a[1] - a[3],
                a[0] - a[2] + t.width as i128,
                a[1] - a[3] + t.height as i128,
            ])
        }
        p::NEAREST_SCALED | p::BOX_FILTER => Some([a[0], a[1], a[0] + a[2], a[1] + a[3]]),
        p::BITMAP_TEXT => Some([a[0], a[1], a[0] + a[4] * a[6], a[1] + a[5] * a[6]]),
        p::COVERAGE_SPANS => Some([a[0], a[1], a[0] + a[2], a[1] + a[3]]),
        _ => None,
    };
    if let Some(b) = bound {
        let r = i.rect.map(i128::from);
        let r = [
            r[0].max(b[0]),
            r[1].max(b[1]),
            r[2].min(b[2]),
            r[3].min(b[3]),
        ];
        if r[2] <= r[0] || r[3] <= r[1] {
            return None;
        }
        i.rect = r.map(|v| v.clamp(i32::MIN as i128, i32::MAX as i128) as i32);
    }
    Some(i)
}

pub fn encode(
    out: &mut impl Extend<u32>,
    pipe: u32,
    colour: u32,
    texture: u64,
    buffer: u64,
    data_bytes: u32,
    instances: &[Instance],
) {
    out.extend([
        COMMAND,
        (HEADER + INSTANCE * instances.len()) as u32,
        pipe,
        colour,
        texture as u32,
        (texture >> 32) as u32,
        buffer as u32,
        (buffer >> 32) as u32,
        data_bytes,
        instances.len() as u32,
    ]);
    for i in instances {
        out.extend(i.rect.map(|x| x as u32));
        out.extend(i.params);
    }
}

/// Sweep rectangle x endpoints and maintain maximum y coverage. Coordinates are
/// clipped to the device maximum, so range updates have at most fourteen levels.
/// End events sort before starts at the same x, preserving half-open edges.
#[cfg(any(feature = "alloc", test))]
pub fn disjoint(rectangles: &[[u32; 4]]) -> Result<bool, alloc::collections::TryReserveError> {
    use alloc::vec::Vec;
    if rectangles.len() < 2 { return Ok(true); }
    let mut events = Vec::new();
    events.try_reserve_exact(rectangles.len() * 2)?;
    for r in rectangles {
        events.push((r[0], 1i32, r[1], r[3]));
        events.push((r[2], -1i32, r[1], r[3]));
    }
    events.sort_unstable();
    let bottom = rectangles.iter().map(|r| r[1]).min().unwrap();
    let top = rectangles.iter().map(|r| r[3]).max().unwrap();
    let side = (top - bottom).next_power_of_two();
    let mut maximum = Vec::new();
    let mut lazy = Vec::new();
    maximum.try_reserve_exact(4 * side as usize)?;
    lazy.try_reserve_exact(4 * side as usize)?;
    maximum.resize(4 * side as usize, 0i32);
    lazy.resize(4 * side as usize, 0i32);
    fn update(
        maximum: &mut [i32],
        lazy: &mut [i32],
        node: usize,
        lo: u32,
        hi: u32,
        a: u32,
        b: u32,
        delta: i32,
    ) {
        if b <= lo || hi <= a {
            return;
        }
        if a <= lo && hi <= b {
            maximum[node] += delta;
            lazy[node] += delta;
            return;
        }
        let mid = (lo + hi) / 2;
        update(maximum, lazy, node * 2, lo, mid, a, b, delta);
        update(maximum, lazy, node * 2 + 1, mid, hi, a, b, delta);
        maximum[node] = lazy[node] + maximum[node * 2].max(maximum[node * 2 + 1]);
    }
    for (_, delta, y0, y1) in events {
        update(&mut maximum, &mut lazy, 1, 0, side, y0-bottom, y1-bottom, delta);
        if maximum[1] > 1 {
            return Ok(false);
        }
    }
    Ok(true)
}

#[cfg(test)]
mod g5_tests {
    use super::*;
    use crate::stream::{self, encode, Resolver, SourceInfo, TargetInfo};
    use alloc::vec::Vec;
    struct Resources;
    impl Resolver for Resources {
        fn target(&self, h: u64) -> Option<TargetInfo> {
            (h == 1).then_some(TargetInfo {
                width: 64,
                height: 32,
                format: 0,
                invalid: false,
            })
        }
        fn source(&self, h: u64) -> Option<SourceInfo> {
            (h == 2).then_some(SourceInfo {
                width: 64,
                height: 32,
                format: 0,
                invalid: false,
            })
        }
        fn buffer_size(&self, _h: u64) -> Option<u64> {
            None
        }
        fn buffer_rect(&self, _h: u64, _o: u64) -> [i32; 4] {
            panic!("no buffers in these streams")
        }
    }
    fn instance(rect: [i32; 4]) -> Instance {
        Instance {
            rect,
            params: [0; 16],
        }
    }
    #[test]
    fn g5_hazard_refuses_overlap_at_the_command_and_counts_the_copy() {
        let mut s = Vec::new();
        encode::begin(&mut s, 1);
        super::encode(
            &mut s,
            p::OVER,
            0,
            2,
            0,
            0,
            &[instance([0, 0, 8, 8]), instance([8, 0, 16, 8])],
        );
        let plan = stream::validate(&s[..], &Resources).unwrap();
        assert_eq!(
            (plan.area, plan.estimate, plan.scratch),
            (128, 128 * 128 + 128, [[16, 8], [0, 0]])
        );
        s[HEADER + 4 + INSTANCE] = 7;
        assert_eq!(
            stream::validate(&s[..], &Resources),
            Err(stream::Refusal {
                index: 1,
                error: OVERLAP
            })
        );
    }
    #[test]
    fn g5_sweep_checks_both_axes_and_half_open_edges() {
        assert!(disjoint(&[[0, 0, 5, 5], [0, 5, 5, 10], [5, 0, 10, 10]]).unwrap());
        assert!(!disjoint(&[[0, 0, 5, 5], [4, 4, 8, 8]]).unwrap());
        let rs: Vec<_> = (0..16384).map(|x| [x, 0, x + 1, 16384]).collect();
        assert!(disjoint(&rs).unwrap());
    }
    #[test]
    fn formatted_copy_preserves_clipping_and_rejects_invalid_metadata() {
        let tex = Some(stream::SourceInfo { width: 5, height: 4, format: 0, invalid: false });
        let mut i = Instance { rect: [0, 0, 8, 8], params: [0; 16] };
        i.params[0] = (-2i32) as u32;
        i.params[1] = (-1i32) as u32;
        i.params[2] = 1;
        assert!(valid(p::COPY, &i.params, tex));
        assert_eq!(source_clip(p::COPY, i, tex).unwrap().rect, [2, 1, 7, 5]);
        i.params[2] = 2;
        assert!(!valid(p::COPY, &i.params, tex));
        i.params[2] = 0;
        i.params[3] = 1;
        assert!(!valid(p::COPY, &i.params, tex));
    }
    #[test]
    fn g5_parameters_and_unused_handles_are_atomic_refusals() {
        for (pipe, params, texture, buffer, bytes) in [
            (p::COPY_SCALED, [0; 16], 2, 0, 0),
            (p::LINE, [1; 16], 0, 0, 0),
            (p::LINE, [0; 16], 2, 0, 0),
            (p::LINE, [0; 16], 0, 2, 4),
        ] {
            let mut s = Vec::new();
            encode::begin(&mut s, 1);
            super::encode(
                &mut s,
                pipe,
                0,
                texture,
                buffer,
                bytes,
                &[Instance {
                    rect: [0, 0, 8, 8],
                    params,
                }],
            );
            assert_eq!(
                stream::validate(&s[..], &Resources),
                Err(stream::Refusal {
                    index: 1,
                    error: PARAMETERS
                })
            );
        }
    }
    #[test]
    fn g5_extreme_bounds_use_wide_geometry() {
        let mut a = [0; 16];
        a[..3].copy_from_slice(&[i32::MAX as u32, 0, i32::MAX as u32]);
        assert_eq!(
            clip(
                p::CIRCLE_FILL,
                Instance {
                    rect: [0, 0, 64, 32],
                    params: a
                },
                64,
                32,
                None
            ),
            Some([0, 0, 64, 32])
        );
        a[2] = u32::MAX;
        assert!(clip(
            p::CIRCLE_FILL,
            Instance {
                rect: [0, 0, 64, 32],
                params: a
            },
            64,
            32,
            None
        )
        .is_none());
    }
    #[test]
    fn g5_crossing_sort_treats_both_signed_zeros_as_equal() {
        struct Buffer(Vec<u32>);
        impl Resolver for Buffer {
            fn target(&self, h: u64) -> Option<TargetInfo> {
                Resources.target(h)
            }
            fn source(&self, h: u64) -> Option<SourceInfo> {
                Resources.source(h)
            }
            fn buffer_size(&self, h: u64) -> Option<u64> {
                (h == 3).then_some(self.0.len() as u64 * 4)
            }
            fn buffer_rect(&self, _h: u64, _o: u64) -> [i32; 4] {
                panic!("word-addressed crossing data")
            }
            fn buffer_word(&self, _h: u64, o: u64) -> u32 {
                self.0[o as usize / 4]
            }
        }
        let r = Buffer(alloc::vec![
            0,
            2,
            2,
            2,
            2,
            2,
            2,
            2,
            2,
            0,
            1,
            0x80000000,
            u32::MAX
        ]);
        let mut i = instance([0, 0, 1, 1]);
        i.params[2] = 1;
        i.params[3] = 1;
        let mut s = Vec::new();
        encode::begin(&mut s, 1);
        super::encode(&mut s, p::COVERAGE_SPANS, 0, 0, 3, 52, &[i]);
        assert!(stream::validate(&s[..], &r).is_ok());
    }
}

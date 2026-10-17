//! The BGL command stream `SUBMIT` carries, and its validator.
//!
//! A stream is a sequence of 32-bit little-endian words. Each command starts with two
//! words, its [`cmd`] code and its length in words counting both; the command's index is
//! its position in the stream (0 for the first). The layouts are in
//! `docs/gui/gpu-abi.md`; this module is their single definition.
//!
//! [`validate`] walks a whole stream before anything is sent (`docs/gui/gpu.md` section
//! 1.4.1, validation atomicity): it resolves every handle through the caller's
//! [`Resolver`], clips every rectangle to its target, totals draws, instances, named
//! buffer bytes and the work estimate against [`crate::limits`], and returns the first
//! refusal with its command's index. Its cost is linear in the stream's length plus the
//! rectangles it reads from buffers, which the instance limit bounds.

use crate::{limits, pipeline};

/// Command codes.
pub mod cmd {
    /// `[BEGIN, 4, target_lo, target_hi]`: later commands draw into this target.
    pub const BEGIN: u32 = 1;
    /// `[CLEAR, 3, colour]`: every pixel of the current target becomes `colour`.
    pub const CLEAR: u32 = 2;
    /// `[DRAW, 5 + 4n, pipeline, colour, n, (x0, y0, x1, y1) * n]`: `n` half-open
    /// rectangles of signed 32-bit coordinates.
    pub const DRAW: u32 = 3;
    /// `[DRAW_BUFFER, 8, pipeline, colour, buffer_lo, buffer_hi, offset, n]`: `n`
    /// rectangles read from a buffer at a byte offset (16 bytes each, as in `DRAW`).
    pub const DRAW_BUFFER: u32 = 4;
}

/// Why a stream was refused (`Submit::error`).
pub mod error {
    pub const NONE: u32 = 0;
    /// A command's length runs past the stream's end or is shorter than its layout.
    pub const TRUNCATED: u32 = 1;
    pub const UNKNOWN_COMMAND: u32 = 2;
    /// A drawing command before any `BEGIN`.
    pub const NO_TARGET: u32 = 3;
    /// A handle that names nothing this process owns.
    pub const BAD_HANDLE: u32 = 4;
    /// A pipeline that is not in the catalogue or not built yet.
    pub const BAD_PIPELINE: u32 = 5;
    /// A rectangle with `x1 < x0` or `y1 < y0`.
    pub const BAD_RECT: u32 = 6;
    /// A command whose length disagrees with its rectangle count.
    pub const BAD_LENGTH: u32 = 7;
    /// A buffer range outside the buffer, or not 16-byte aligned.
    pub const BAD_RANGE: u32 = 8;
    pub const DRAW_LIMIT: u32 = 9;
    pub const INSTANCE_LIMIT: u32 = 10;
    pub const BUFFER_LIMIT: u32 = 11;
    /// The work estimate passed its cap (EDQUOT).
    pub const WORK_LIMIT: u32 = 12;
    /// The stream is longer than `limits::STREAM_BYTES`, empty, or not whole words.
    pub const STREAM_LENGTH: u32 = 13;
    /// A target the device left invalid (`docs/gui/gpu.md` section 1.4.1).
    pub const INVALID_TARGET: u32 = 14;
}

/// The words of a stream the kernel has copied out of user memory.
pub trait Words {
    fn len(&self) -> usize;
    fn word(&self, index: usize) -> u32;
}

impl Words for [u32] {
    fn len(&self) -> usize {
        <[u32]>::len(self)
    }
    fn word(&self, index: usize) -> u32 {
        self[index]
    }
}

/// A target as the validator needs it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TargetInfo {
    pub width: u32,
    pub height: u32,
    /// The device left it invalid; drawing to it is refused until a whole write.
    pub invalid: bool,
}

/// The submitting process's resources, as the validator sees them.
pub trait Resolver {
    /// The target `handle` names, if the process owns one by that handle.
    fn target(&self, handle: u64) -> Option<TargetInfo>;
    /// The size of the buffer `handle` names, if the process owns one by that handle.
    fn buffer_size(&self, handle: u64) -> Option<u64>;
    /// The rectangle at byte `offset` of a buffer the resolver already named.
    fn buffer_rect(&self, handle: u64, offset: u64) -> [i32; 4];
}

/// What a valid stream will do, for the counters and the translator.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    pub commands: u32,
    pub draws: u64,
    pub instances: u64,
    /// Pixels written, after clipping.
    pub area: u64,
    /// Rectangles that keep pixels after clipping: the ones the device draws.
    pub drawn: u64,
    /// Fragment-steps (section 1.4.1).
    pub estimate: u64,
    /// Bytes of the distinct buffers the stream names.
    pub buffer_bytes: u64,
}

/// A refusal: the command's index and its [`error`] code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Refusal {
    pub index: u32,
    pub error: u32,
}

impl Refusal {
    /// Whether the refusal is the work cap's (EDQUOT) rather than EINVAL.
    pub fn is_work_limit(&self) -> bool {
        self.error == error::WORK_LIMIT
    }
}

/// The number of distinct buffers one stream may name.
pub const MAX_NAMED_BUFFERS: usize = 64;

/// A rectangle clipped to a `width` x `height` target, or `None` when nothing remains.
pub fn clip(rect: [i32; 4], width: u32, height: u32) -> Option<[u32; 4]> {
    let [x0, y0, x1, y1] = rect;
    let cx0 = (x0 as i64).clamp(0, width as i64);
    let cy0 = (y0 as i64).clamp(0, height as i64);
    let cx1 = (x1 as i64).clamp(0, width as i64);
    let cy1 = (y1 as i64).clamp(0, height as i64);
    if cx1 <= cx0 || cy1 <= cy0 {
        return None;
    }
    Some([cx0 as u32, cy0 as u32, cx1 as u32, cy1 as u32])
}

fn handle_at(words: &(impl Words + ?Sized), at: usize) -> u64 {
    words.word(at) as u64 | (words.word(at + 1) as u64) << 32
}

/// Walk `words` and return its plan, or the first refusal.
pub fn validate(words: &(impl Words + ?Sized), resolver: &impl Resolver) -> Result<Plan, Refusal> {
    let total = words.len();
    let mut plan = Plan::default();
    let mut target: Option<TargetInfo> = None;
    let mut named: [u64; MAX_NAMED_BUFFERS] = [0; MAX_NAMED_BUFFERS];
    let mut named_count = 0usize;
    let mut at = 0usize;
    let mut index: u32 = 0;
    if total == 0 {
        return Err(Refusal { index: 0, error: error::STREAM_LENGTH });
    }
    while at < total {
        let refuse = |error| Err(Refusal { index, error });
        if total - at < 2 {
            return refuse(error::TRUNCATED);
        }
        let code = words.word(at);
        let len = words.word(at + 1) as usize;
        if len < 2 || len > total - at {
            return refuse(error::TRUNCATED);
        }
        match code {
            cmd::BEGIN => {
                if len != 4 {
                    return refuse(error::BAD_LENGTH);
                }
                match resolver.target(handle_at(words, at + 2)) {
                    Some(t) => target = Some(t),
                    None => return refuse(error::BAD_HANDLE),
                }
            }
            cmd::CLEAR => {
                if len != 3 {
                    return refuse(error::BAD_LENGTH);
                }
                let Some(t) = target else { return refuse(error::NO_TARGET) };
                let area = t.width as u64 * t.height as u64;
                count_draw(&mut plan, index, 1, area, 1)?;
                // A clear writes the whole target, so it makes an invalid target valid.
                target = Some(TargetInfo { invalid: false, ..t });
            }
            cmd::DRAW | cmd::DRAW_BUFFER => {
                if len < 5 {
                    return refuse(error::TRUNCATED);
                }
                let Some(t) = target else { return refuse(error::NO_TARGET) };
                let pipe = words.word(at + 2);
                let Some(steps) = pipeline::steps(pipe) else { return refuse(error::BAD_PIPELINE) };
                if t.invalid {
                    return refuse(error::INVALID_TARGET);
                }
                let mut area = 0u64;
                let count;
                if code == cmd::DRAW {
                    count = words.word(at + 4) as usize;
                    if len != 5 + 4 * count {
                        return refuse(error::BAD_LENGTH);
                    }
                    for r in 0..count {
                        let base = at + 5 + 4 * r;
                        let rect = [
                            words.word(base) as i32,
                            words.word(base + 1) as i32,
                            words.word(base + 2) as i32,
                            words.word(base + 3) as i32,
                        ];
                        if rect[2] < rect[0] || rect[3] < rect[1] {
                            return refuse(error::BAD_RECT);
                        }
                        if let Some([x0, y0, x1, y1]) = clip(rect, t.width, t.height) {
                            area += (x1 - x0) as u64 * (y1 - y0) as u64;
                            plan.drawn += 1;
                        }
                    }
                } else {
                    if len != 8 {
                        return refuse(error::BAD_LENGTH);
                    }
                    let buffer = handle_at(words, at + 4);
                    let offset = words.word(at + 6) as u64;
                    count = words.word(at + 7) as usize;
                    let Some(size) = resolver.buffer_size(buffer) else { return refuse(error::BAD_HANDLE) };
                    let end = offset + 16 * count as u64;
                    if offset % 16 != 0 || end > size {
                        return refuse(error::BAD_RANGE);
                    }
                    if !named[..named_count].contains(&buffer) {
                        if named_count == MAX_NAMED_BUFFERS {
                            return refuse(error::BUFFER_LIMIT);
                        }
                        named[named_count] = buffer;
                        named_count += 1;
                        plan.buffer_bytes += size;
                        if plan.buffer_bytes > limits::BUFFER_BYTES {
                            return refuse(error::BUFFER_LIMIT);
                        }
                    }
                    // The instance limit bounds the reads below; check it before them.
                    if plan.instances + count as u64 > limits::INSTANCES {
                        return refuse(error::INSTANCE_LIMIT);
                    }
                    for r in 0..count as u64 {
                        let rect = resolver.buffer_rect(buffer, offset + 16 * r);
                        if rect[2] < rect[0] || rect[3] < rect[1] {
                            return refuse(error::BAD_RECT);
                        }
                        if let Some([x0, y0, x1, y1]) = clip(rect, t.width, t.height) {
                            area += (x1 - x0) as u64 * (y1 - y0) as u64;
                            plan.drawn += 1;
                        }
                    }
                }
                count_draw(&mut plan, index, count as u64, area, steps)?;
            }
            _ => return refuse(error::UNKNOWN_COMMAND),
        }
        at += len;
        index += 1;
    }
    plan.commands = index;
    Ok(plan)
}

/// Count one draw of `instances` writing `area` pixels at `steps` per fragment.
fn count_draw(plan: &mut Plan, index: u32, instances: u64, area: u64, steps: u64) -> Result<(), Refusal> {
    plan.draws += 1;
    if plan.draws > limits::DRAWS {
        return Err(Refusal { index, error: error::DRAW_LIMIT });
    }
    plan.instances += instances;
    if plan.instances > limits::INSTANCES {
        return Err(Refusal { index, error: error::INSTANCE_LIMIT });
    }
    plan.area += area;
    plan.estimate += area * steps;
    if plan.estimate > limits::WORK {
        return Err(Refusal { index, error: error::WORK_LIMIT });
    }
    Ok(())
}

/// Encoders for the commands, for userspace and tests.
pub mod encode {
    use super::cmd;

    pub fn begin(out: &mut impl Extend<u32>, target: u64) {
        out.extend([cmd::BEGIN, 4, target as u32, (target >> 32) as u32]);
    }

    pub fn clear(out: &mut impl Extend<u32>, colour: u32) {
        out.extend([cmd::CLEAR, 3, colour]);
    }

    /// `rects` are `(x0, y0, x1, y1)`.
    pub fn draw(out: &mut impl Extend<u32>, pipeline: u32, colour: u32, rects: &[[i32; 4]]) {
        out.extend([cmd::DRAW, 5 + 4 * rects.len() as u32, pipeline, colour, rects.len() as u32]);
        for r in rects {
            out.extend(r.iter().map(|&v| v as u32));
        }
    }

    pub fn draw_buffer(out: &mut impl Extend<u32>, pipeline: u32, colour: u32, buffer: u64, offset: u32, count: u32) {
        out.extend([cmd::DRAW_BUFFER, 8, pipeline, colour, buffer as u32, (buffer >> 32) as u32, offset, count]);
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::vec::Vec;

    struct One {
        target: u64,
        width: u32,
        height: u32,
        buffers: Vec<(u64, u64)>,
    }

    impl Resolver for One {
        fn target(&self, handle: u64) -> Option<TargetInfo> {
            (handle == self.target).then_some(TargetInfo { width: self.width, height: self.height, invalid: false })
        }
        fn buffer_size(&self, handle: u64) -> Option<u64> {
            self.buffers.iter().find(|b| b.0 == handle).map(|b| b.1)
        }
        fn buffer_rect(&self, _handle: u64, _offset: u64) -> [i32; 4] {
            [0, 0, 1, 1]
        }
    }

    fn one() -> One {
        One { target: 0x1234_5678_9abc, width: 64, height: 32, buffers: Vec::new() }
    }

    #[test]
    fn a_clear_and_a_draw_are_planned() {
        let r = one();
        let mut s = Vec::new();
        encode::begin(&mut s, r.target);
        encode::clear(&mut s, 0xff00_00ff);
        encode::draw(&mut s, pipeline::SOLID, 1, &[[-5, -5, 10, 10], [60, 30, 100, 100], [3, 3, 3, 9]]);
        let plan = validate(&s[..], &r).unwrap();
        assert_eq!(plan.commands, 3);
        assert_eq!(plan.draws, 2);
        assert_eq!(plan.instances, 4);
        assert_eq!(plan.area, 64 * 32 + 100 + 8);
        assert_eq!(plan.estimate, plan.area);
        // The third rectangle is empty: two of the three keep pixels.
        assert_eq!(plan.drawn, 2);
    }

    #[test]
    fn refusals_name_their_command() {
        let r = one();
        let mut s = Vec::new();
        encode::begin(&mut s, r.target);
        encode::clear(&mut s, 0);
        encode::draw(&mut s, pipeline::SOLID, 1, &[[5, 5, 4, 9]]);
        assert_eq!(validate(&s[..], &r), Err(Refusal { index: 2, error: error::BAD_RECT }));

        let mut s = Vec::new();
        encode::clear(&mut s, 0);
        assert_eq!(validate(&s[..], &r), Err(Refusal { index: 0, error: error::NO_TARGET }));

        let mut s = Vec::new();
        encode::begin(&mut s, r.target + 1);
        assert_eq!(validate(&s[..], &r), Err(Refusal { index: 0, error: error::BAD_HANDLE }));

        let mut s = Vec::new();
        encode::begin(&mut s, r.target);
        s.extend([99, 2]);
        assert_eq!(validate(&s[..], &r), Err(Refusal { index: 1, error: error::UNKNOWN_COMMAND }));

        let mut s = Vec::new();
        encode::begin(&mut s, r.target);
        s.extend([cmd::CLEAR, 9, 0]);
        assert_eq!(validate(&s[..], &r), Err(Refusal { index: 1, error: error::TRUNCATED }));

        let mut s = Vec::new();
        encode::begin(&mut s, r.target);
        encode::draw(&mut s, pipeline::LINE, 1, &[[0, 0, 1, 1]]);
        assert_eq!(validate(&s[..], &r), Err(Refusal { index: 1, error: error::BAD_PIPELINE }));
        assert_eq!(validate(&[][..], &r), Err(Refusal { index: 0, error: error::STREAM_LENGTH }));
    }

    #[test]
    fn limits_refuse_the_command_that_passes_them() {
        let r = One { target: 7, width: 16_384, height: 16_384, buffers: std::vec![(9, 40 << 20), (10, 30 << 20)] };
        let mut s = Vec::new();
        encode::begin(&mut s, 7);
        for _ in 0..limits::DRAWS {
            encode::draw(&mut s, pipeline::SOLID, 0, &[]);
        }
        assert!(validate(&s[..], &r).is_ok());
        encode::draw(&mut s, pipeline::SOLID, 0, &[]);
        assert_eq!(validate(&s[..], &r), Err(Refusal { index: limits::DRAWS as u32 + 1, error: error::DRAW_LIMIT }));

        let mut s = Vec::new();
        encode::begin(&mut s, 7);
        for _ in 0..8 {
            encode::clear(&mut s, 0);
        }
        // Eight 16384x16384 clears are 2^31 fragment-steps; a ninth passes the cap.
        assert_eq!(validate(&s[..], &r).unwrap().estimate, limits::WORK);
        encode::clear(&mut s, 0);
        let refusal = validate(&s[..], &r).unwrap_err();
        assert_eq!(refusal, Refusal { index: 9, error: error::WORK_LIMIT });
        assert!(refusal.is_work_limit());

        let mut s = Vec::new();
        encode::begin(&mut s, 7);
        encode::draw_buffer(&mut s, pipeline::SOLID, 0, 9, 0, 1);
        encode::draw_buffer(&mut s, pipeline::SOLID, 0, 9, 16, 1);
        assert_eq!(validate(&s[..], &r).unwrap().buffer_bytes, 40 << 20);
        encode::draw_buffer(&mut s, pipeline::SOLID, 0, 10, 0, 1);
        assert_eq!(validate(&s[..], &r), Err(Refusal { index: 3, error: error::BUFFER_LIMIT }));

        let mut s = Vec::new();
        encode::begin(&mut s, 7);
        encode::draw_buffer(&mut s, pipeline::SOLID, 0, 9, 8, 1);
        assert_eq!(validate(&s[..], &r), Err(Refusal { index: 1, error: error::BAD_RANGE }));
    }

    #[test]
    fn clipping_keeps_only_the_target() {
        assert_eq!(clip([-10, -10, 5, 5], 4, 4), Some([0, 0, 4, 4]));
        assert_eq!(clip([4, 0, 9, 9], 4, 4), None);
        assert_eq!(clip([i32::MIN, i32::MIN, i32::MAX, i32::MAX], 7, 3), Some([0, 0, 7, 3]));
    }
}

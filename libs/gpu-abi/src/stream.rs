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
    /// `[COPY, 12, source_lo, source_hi, sx, sy, w, h, destination_lo, destination_hi, dx,
    /// dy]`: a region copy of the source rectangle to the destination point, unsigned.
    pub const COPY: u32 = 5;
    /// `[DRAW_SAMPLED, 6 + 6n, pipeline, texture_lo, texture_hi, n, (x0, y0, x1, y1, sx,
    /// sy) * n]`: `n` half-open rectangles of the current target drawn from the texture,
    /// pixel `(x, y)` reading texel `(sx + x - x0, sy + y - y0)`; signed 32-bit.
    pub const DRAW_SAMPLED: u32 = 6;
}

/// Why a stream was refused (`Submit::error`).
pub mod error {
    pub const NONE: u32 = 0;
    /// A command's length runs past the stream's end or is shorter than its layout.
    pub const TRUNCATED: u32 = 1;
    pub const UNKNOWN_COMMAND: u32 = 2;
    /// A drawing command other than `COPY` before any `BEGIN`.
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
    /// A command that samples a target or texture the device left invalid (`docs/gui/gpu.md`
    /// section 1.4.1): a `COPY` source or a `DRAW_SAMPLED` texture. Drawing into an
    /// invalid target is allowed.
    pub const INVALID_TARGET: u32 = 14;
    /// A `COPY` between resources of different formats.
    pub const FORMAT_MISMATCH: u32 = 15;
    /// A sampling pipeline in `DRAW` or `DRAW_BUFFER`, or another pipeline in
    /// `DRAW_SAMPLED`.
    pub const WRONG_COMMAND: u32 = 16;
    /// A `DRAW_SAMPLED` whose texture is the target it draws into.
    pub const SAMPLES_TARGET: u32 = 17;
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
    /// The device left it invalid: it may be drawn into, and sampling it is refused
    /// until a whole write completes OK.
    pub invalid: bool,
    /// Its BGL format.
    pub format: u32,
}

/// A target or texture a command reads from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceInfo {
    pub width: u32,
    pub height: u32,
    pub format: u32,
    /// The device left it invalid, so it may not be sampled.
    pub invalid: bool,
}

/// The submitting process's resources, as the validator sees them.
pub trait Resolver {
    /// The target `handle` names, if the process owns one by that handle.
    fn target(&self, handle: u64) -> Option<TargetInfo>;
    /// The target or texture `handle` names, if the process owns one by that handle.
    fn source(&self, handle: u64) -> Option<SourceInfo>;
    /// The size of the buffer `handle` names, if the process owns one by that handle.
    fn buffer_size(&self, handle: u64) -> Option<u64>;
    /// The rectangle at byte `offset` of a buffer the resolver already named, read from
    /// the submission's one copy of that range, taken before validation
    /// (`docs/gui/gpu.md` section 1.4.1: buffers are snapshotted at submission).
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
    /// The largest copy that goes through a scratch texture, per target format
    /// (B8G8R8A8, R8G8B8A8): width and height, 0 when no copy in that format is staged.
    pub scratch: [[u32; 2]; 2],
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

/// A `COPY`'s rectangle after clipping: the source rectangle `[sx, sx + w) x [sy, sy + h)`
/// clipped to a `source` sized resource, then, anchored at `(dx, dy)`, to a `destination`
/// sized one. Returns `(sx, sy, w, h, dx, dy)`, or no rectangle when the clipped one is empty.
pub fn copy_clip(r: [u32; 6], source: [u32; 2], destination: [u32; 2]) -> Option<[u32; 6]> {
    let [sx, sy, w, h, dx, dy] = r.map(|v| v as u64);
    let (sw, sh, dw, dh) = (source[0] as u64, source[1] as u64, destination[0] as u64, destination[1] as u64);
    if sx >= sw || sy >= sh || dx >= dw || dy >= dh {
        return None;
    }
    let w = w.min(sw - sx).min(dw - dx);
    let h = h.min(sh - sy).min(dh - dy);
    if w == 0 || h == 0 {
        return None;
    }
    Some([sx as u32, sy as u32, w as u32, h as u32, dx as u32, dy as u32])
}

/// Whether a copy within one resource reads pixels it also writes.
pub fn copy_overlaps(c: [u32; 6]) -> bool {
    let [sx, sy, w, h, dx, dy] = c;
    sx < dx + w && dx < sx + w && sy < dy + h && dy < sy + h
}

/// A `DRAW_SAMPLED` instance `(x0, y0, x1, y1, sx, sy)` clipped to a `width` x `height`
/// target and to the footprint of a `tw` x `th` texture placed so pixel `(x0, y0)` reads
/// texel `(sx, sy)`, or no rectangle when the clipped one is empty.
pub fn sampled_clip(i: [i32; 6], width: u32, height: u32, tw: u32, th: u32) -> Option<[u32; 4]> {
    let [x0, y0, x1, y1, sx, sy] = i.map(|v| v as i64);
    let fx0 = x0 - sx;
    let fy0 = y0 - sy;
    let x0 = x0.max(fx0).clamp(0, width as i64);
    let y0 = y0.max(fy0).clamp(0, height as i64);
    let x1 = x1.min(fx0 + tw as i64).clamp(0, width as i64);
    let y1 = y1.min(fy0 + th as i64).clamp(0, height as i64);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    Some([x0 as u32, y0 as u32, x1 as u32, y1 as u32])
}

fn handle_at(words: &(impl Words + ?Sized), at: usize) -> u64 {
    words.word(at) as u64 | (words.word(at + 1) as u64) << 32
}

/// Walk `words` and return its plan, or the first refusal.
pub fn validate(words: &(impl Words + ?Sized), resolver: &impl Resolver) -> Result<Plan, Refusal> {
    let total = words.len();
    let mut plan = Plan::default();
    let mut target: Option<TargetInfo> = None;
    let mut target_handle = 0u64;
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
                target_handle = handle_at(words, at + 2);
                match resolver.target(target_handle) {
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
            }
            cmd::DRAW | cmd::DRAW_BUFFER => {
                if len < 5 {
                    return refuse(error::TRUNCATED);
                }
                let Some(t) = target else { return refuse(error::NO_TARGET) };
                let pipe = words.word(at + 2);
                let Some(steps) = pipeline::steps(pipe) else { return refuse(error::BAD_PIPELINE) };
                if pipeline::samples(pipe) {
                    return refuse(error::WRONG_COMMAND);
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
            cmd::COPY => {
                if len != 12 {
                    return refuse(error::BAD_LENGTH);
                }
                let source = handle_at(words, at + 2);
                let destination = handle_at(words, at + 8);
                let Some(s) = resolver.source(source) else { return refuse(error::BAD_HANDLE) };
                let Some(d) = resolver.target(destination) else { return refuse(error::BAD_HANDLE) };
                if s.format != d.format {
                    return refuse(error::FORMAT_MISMATCH);
                }
                if s.invalid {
                    return refuse(error::INVALID_TARGET);
                }
                let r = [4, 5, 6, 7, 10, 11].map(|k| words.word(at + k));
                let (area, copies) = match copy_clip(r, [s.width, s.height], [d.width, d.height]) {
                    Some(c) => {
                        let staged = source == destination && copy_overlaps(c);
                        if staged {
                            let slot = &mut plan.scratch[usize::from(d.format == crate::format::R8G8B8A8)];
                            slot[0] = slot[0].max(c[2]);
                            slot[1] = slot[1].max(c[3]);
                        }
                        (c[2] as u64 * c[3] as u64, if staged { 2 } else { 1 })
                    }
                    None => (0, 1),
                };
                count_draw(&mut plan, index, 1, area, copies)?;
            }
            cmd::DRAW_SAMPLED => {
                if len < 6 {
                    return refuse(error::TRUNCATED);
                }
                let Some(t) = target else { return refuse(error::NO_TARGET) };
                let pipe = words.word(at + 2);
                let Some(steps) = pipeline::steps(pipe) else { return refuse(error::BAD_PIPELINE) };
                if !pipeline::samples(pipe) {
                    return refuse(error::WRONG_COMMAND);
                }
                let count = words.word(at + 5) as usize;
                if len != 6 + 6 * count {
                    return refuse(error::BAD_LENGTH);
                }
                let texture = handle_at(words, at + 3);
                let Some(src) = resolver.source(texture) else { return refuse(error::BAD_HANDLE) };
                if texture == target_handle {
                    return refuse(error::SAMPLES_TARGET);
                }
                if src.invalid {
                    return refuse(error::INVALID_TARGET);
                }
                let mut area = 0u64;
                for r in 0..count {
                    let base = at + 6 + 6 * r;
                    let i = [0, 1, 2, 3, 4, 5].map(|k| words.word(base + k) as i32);
                    if i[2] < i[0] || i[3] < i[1] {
                        return refuse(error::BAD_RECT);
                    }
                    if let Some([x0, y0, x1, y1]) = sampled_clip(i, t.width, t.height, src.width, src.height) {
                        area += (x1 - x0) as u64 * (y1 - y0) as u64;
                        plan.drawn += 1;
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

    /// `rect` is `(sx, sy, w, h)` of `source`; it lands at `point` of `destination`.
    pub fn copy(out: &mut impl Extend<u32>, source: u64, rect: [u32; 4], destination: u64, point: [u32; 2]) {
        out.extend([cmd::COPY, 12, source as u32, (source >> 32) as u32, rect[0], rect[1], rect[2], rect[3]]);
        out.extend([destination as u32, (destination >> 32) as u32, point[0], point[1]]);
    }

    /// `instances` are `(x0, y0, x1, y1, sx, sy)`.
    pub fn draw_sampled(out: &mut impl Extend<u32>, pipeline: u32, texture: u64, instances: &[[i32; 6]]) {
        out.extend([cmd::DRAW_SAMPLED, 6 + 6 * instances.len() as u32, pipeline, texture as u32, (texture >> 32) as u32, instances.len() as u32]);
        for i in instances {
            out.extend(i.iter().map(|&v| v as u32));
        }
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
        invalid: bool,
        buffers: Vec<(u64, u64)>,
        textures: Vec<(u64, SourceInfo)>,
    }

    impl Resolver for One {
        fn target(&self, handle: u64) -> Option<TargetInfo> {
            (handle == self.target).then_some(TargetInfo { width: self.width, height: self.height, invalid: self.invalid, format: crate::format::B8G8R8A8 })
        }
        fn source(&self, handle: u64) -> Option<SourceInfo> {
            if handle == self.target {
                return Some(SourceInfo { width: self.width, height: self.height, format: crate::format::B8G8R8A8, invalid: self.invalid });
            }
            self.textures.iter().find(|t| t.0 == handle).map(|t| t.1)
        }
        fn buffer_size(&self, handle: u64) -> Option<u64> {
            self.buffers.iter().find(|b| b.0 == handle).map(|b| b.1)
        }
        fn buffer_rect(&self, _handle: u64, _offset: u64) -> [i32; 4] {
            [0, 0, 1, 1]
        }
    }

    fn one() -> One {
        One { target: 0x1234_5678_9abc, width: 64, height: 32, invalid: false, buffers: Vec::new(), textures: Vec::new() }
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
        let r = One { target: 7, width: 16_384, height: 16_384, invalid: false, buffers: std::vec![(9, 40 << 20), (10, 30 << 20)], textures: Vec::new() };
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
    fn an_invalid_target_may_be_drawn_into() {
        let r = One { invalid: true, ..one() };
        let mut s = Vec::new();
        encode::begin(&mut s, r.target);
        encode::draw(&mut s, pipeline::SOLID, 1, &[[0, 0, 4, 4]]);
        encode::clear(&mut s, 0);
        encode::draw(&mut s, pipeline::SOLID, 1, &[[1, 1, 2, 2]]);
        let plan = validate(&s[..], &r).unwrap();
        assert_eq!(plan.draws, 3);
        assert_eq!(plan.area, 16 + 64 * 32 + 1);
    }

    fn with_textures() -> One {
        let tex = |w, h, format, invalid| SourceInfo { width: w, height: h, format, invalid };
        One {
            textures: std::vec![
                (0x51, tex(10, 6, crate::format::B8G8R8A8, false)),
                (0x52, tex(4, 4, crate::format::R8G8B8A8, false)),
                (0x53, tex(4, 4, crate::format::B8G8R8A8, true)),
                (0x54, tex(64, 2, crate::format::R8, false)),
            ],
            ..one()
        }
    }

    #[test]
    fn copies_are_clipped_counted_and_staged_when_they_overlap() {
        let r = with_textures();
        let mut s = Vec::new();
        // Before any BEGIN: clipped by the 10x6 source to 7x5, then at x 60 by the target to 4x5.
        encode::copy(&mut s, 0x51, [3, 1, 8, 9], r.target, [60, 0]);
        // Within the target, overlapping: staged.
        encode::copy(&mut s, r.target, [0, 0, 10, 10], r.target, [5, 5]);
        // Within the target, disjoint: direct.
        encode::copy(&mut s, r.target, [0, 0, 4, 4], r.target, [10, 0]);
        // Clipped away: counted, with no pixel copied.
        encode::copy(&mut s, 0x51, [10, 0, 4, 4], r.target, [0, 0]);
        let plan = validate(&s[..], &r).unwrap();
        assert_eq!(plan.draws, 4);
        assert_eq!(plan.instances, 4);
        assert_eq!(plan.area, 4 * 5 + 100 + 16);
        assert_eq!(plan.estimate, 20 + 2 * 100 + 16);
        assert_eq!(plan.scratch, [[10, 10], [0, 0]]);
        assert_eq!(plan.drawn, 0);
        assert_eq!(copy_clip([3, 1, 8, 9, 60, 0], [10, 6], [64, 32]), Some([3, 1, 4, 5, 60, 0]));
        assert_eq!(copy_clip([0, 0, 8, 8, 0, 0], [10, 6], [64, 32]), Some([0, 0, 8, 6, 0, 0]));
        assert_eq!(copy_clip([10, 0, 1, 1, 0, 0], [10, 6], [64, 32]), None);
        assert_eq!(copy_clip([0, 0, 1, 1, 64, 0], [10, 6], [64, 32]), None);
        assert_eq!(copy_clip([u32::MAX, 0, u32::MAX, 1, 0, 0], [10, 6], [64, 32]), None);
        assert!(copy_overlaps([0, 0, 10, 10, 5, 5]) && !copy_overlaps([0, 0, 4, 4, 4, 0]) && copy_overlaps([1, 0, 4, 1, 0, 0]));
    }

    #[test]
    fn copy_refusals() {
        let r = with_textures();
        let mut s = Vec::new();
        encode::copy(&mut s, 0x52, [0, 0, 1, 1], r.target, [0, 0]);
        assert_eq!(validate(&s[..], &r), Err(Refusal { index: 0, error: error::FORMAT_MISMATCH }));
        let mut s = Vec::new();
        encode::copy(&mut s, 0x53, [0, 0, 1, 1], r.target, [0, 0]);
        assert_eq!(validate(&s[..], &r), Err(Refusal { index: 0, error: error::INVALID_TARGET }));
        let mut s = Vec::new();
        encode::copy(&mut s, r.target, [0, 0, 1, 1], 0x51, [0, 0]);
        assert_eq!(validate(&s[..], &r), Err(Refusal { index: 0, error: error::BAD_HANDLE }));
        let mut s = Vec::new();
        encode::copy(&mut s, 0x99, [0, 0, 1, 1], r.target, [0, 0]);
        assert_eq!(validate(&s[..], &r), Err(Refusal { index: 0, error: error::BAD_HANDLE }));
        let mut s = Vec::new();
        s.extend([cmd::COPY, 11, 0x51, 0, 0, 0, 1, 1, r.target as u32, (r.target >> 32) as u32, 0]);
        assert_eq!(validate(&s[..], &r), Err(Refusal { index: 0, error: error::BAD_LENGTH }));
    }

    #[test]
    fn sampled_draws_clip_to_the_texture() {
        let r = with_textures();
        let mut s = Vec::new();
        encode::begin(&mut s, r.target);
        // 10x6 texture placed at (-2, 3): pixel (x, y) reads texel (x + 2, y - 3).
        encode::draw_sampled(&mut s, pipeline::COPY, 0x51, &[[-100, -100, 100, 100, -98, -103], [0, 0, 0, 5, 0, 0]]);
        let plan = validate(&s[..], &r).unwrap();
        assert_eq!(plan.area, 8 * 6);
        assert_eq!(plan.drawn, 1);
        assert_eq!(plan.instances, 2);
        assert_eq!(sampled_clip([-100, -100, 100, 100, -98, -103], 64, 32, 10, 6), Some([0, 3, 8, 9]));
        assert_eq!(sampled_clip([60, 30, 70, 40, 0, 0], 64, 32, 10, 6), Some([60, 30, 64, 32]));
        assert_eq!(sampled_clip([0, 0, 4, 4, 10, 0], 64, 32, 10, 6), None);
        assert_eq!(sampled_clip([i32::MIN, i32::MIN, i32::MAX, i32::MAX, i32::MAX, i32::MAX], 64, 32, 10, 6), None);
    }

    #[test]
    fn sampled_draw_refusals() {
        let r = with_textures();
        let refusal = |body: &dyn Fn(&mut Vec<u32>)| {
            let mut s = Vec::new();
            encode::begin(&mut s, r.target);
            body(&mut s);
            validate(&s[..], &r).unwrap_err()
        };
        let at1 = |error| Refusal { index: 1, error };
        assert_eq!(refusal(&|s| encode::draw_sampled(s, pipeline::SOLID, 0x51, &[])), at1(error::WRONG_COMMAND));
        assert_eq!(refusal(&|s| encode::draw(s, pipeline::COPY, 0, &[[0, 0, 1, 1]])), at1(error::WRONG_COMMAND));
        assert_eq!(refusal(&|s| encode::draw_buffer(s, pipeline::COPY, 0, 9, 0, 1)), at1(error::WRONG_COMMAND));
        assert_eq!(refusal(&|s| encode::draw_sampled(s, pipeline::COPY_SCALED, 0x51, &[])), at1(error::BAD_PIPELINE));
        assert_eq!(refusal(&|s| encode::draw_sampled(s, pipeline::COPY, r.target, &[])), at1(error::SAMPLES_TARGET));
        assert_eq!(refusal(&|s| encode::draw_sampled(s, pipeline::COPY, 0x53, &[])), at1(error::INVALID_TARGET));
        assert_eq!(refusal(&|s| encode::draw_sampled(s, pipeline::COPY, 0x77, &[])), at1(error::BAD_HANDLE));
        assert_eq!(refusal(&|s| encode::draw_sampled(s, pipeline::COPY, 0x51, &[[3, 0, 2, 1, 0, 0]])), at1(error::BAD_RECT));
        assert_eq!(refusal(&|s| s.extend([cmd::DRAW_SAMPLED, 7, pipeline::COPY, 0x51, 0, 1, 0])), at1(error::BAD_LENGTH));
        let mut s = Vec::new();
        encode::draw_sampled(&mut s, pipeline::COPY, 0x51, &[]);
        assert_eq!(validate(&s[..], &r), Err(Refusal { index: 0, error: error::NO_TARGET }));
    }

    #[test]
    fn clipping_keeps_only_the_target() {
        assert_eq!(clip([-10, -10, 5, 5], 4, 4), Some([0, 0, 4, 4]));
        assert_eq!(clip([4, 0, 9, 9], 4, 4), None);
        assert_eq!(clip([i32::MIN, i32::MIN, i32::MAX, i32::MAX], 7, 3), Some([0, 0, 7, 3]));
    }
}

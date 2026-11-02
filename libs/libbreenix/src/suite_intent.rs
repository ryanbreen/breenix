//! Text-free intent frames, drawn after the case verdict and its end checkpoint.
use crate::{gpu::{self, abi::{self, stream::encode}}, graphics, time};
use std::vec::Vec;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Layout {
    pub scale: u32,
    pub divisor: u32,
    pub picture: [u32; 4],
    pub border: Option<[u32; 4]>,
}

/// Coordinator's large-picture ruling: integer enlargement, exact mode, or decimation.
pub(super) fn layout(picture: [u32; 2], mode: [u32; 2]) -> Result<Layout, &'static str> {
    let [w, h] = picture;
    let [mw, mh] = mode;
    if w == 0 || h == 0 || mw <= 64 || mh <= 64 || [w, h, mw, mh].iter().any(|&v| v > 16384) {
        return Err("intent-dimensions");
    }
    if picture == mode {
        return Ok(Layout { scale: 1, divisor: 1, picture: [0, 0, w, h], border: None });
    }
    let scale = ((mw - 64) / w).min((mh - 64) / h);
    let (scale, divisor, width, height) = if scale > 0 {
        (scale, 1, w * scale, h * scale)
    } else {
        let k = w.div_ceil(mw - 64).max(h.div_ceil(mh - 64));
        (1, k, w.div_ceil(k), h.div_ceil(k))
    };
    let (x, y) = ((mw - width) / 2, (mh - height) / 2);
    Ok(Layout { scale, divisor, picture: [x, y, width, height],
        border: Some([x - 2, y - 2, width + 4, height + 4]) })
}

/// Up to four GPU sample rectangles preserve exact k-spaced samples at partial edges.
pub(super) fn samples(picture: [u32; 2], layout: Layout) -> Vec<([u32; 4], [u32; 4])> {
    if layout.divisor == 1 {
        return vec![(layout.picture, [0, 0, picture[0], picture[1]])];
    }
    let k = layout.divisor;
    let axes = |size: u32| {
        let n = size / k;
        let mut spans = Vec::new();
        if n > 0 { spans.push((0, n, 0, n * k)); }
        if size % k != 0 { spans.push((n, 1, n * k, 1)); }
        spans
    };
    let mut tiles = Vec::new();
    for (dy, dh, sy, sh) in axes(picture[1]) {
        for (dx, dw, sx, sw) in axes(picture[0]) {
            tiles.push(([layout.picture[0] + dx, layout.picture[1] + dy, dw, dh], [sx, sy, sw, sh]));
        }
    }
    tiles
}

pub(super) fn configured(suite: &str) -> bool {
    matches!(suite, "gpu" | "breengel")
        && ["/proc/breenix/screen-proof", "/etc/breenix/screen-proof"].iter()
            .filter_map(|path| std::fs::read_to_string(path).ok()).any(|s| s.trim() == "on")
}

pub(super) fn pace_ms() -> u64 {
    ["/proc/breenix/pace", "/etc/breenix/pace"].iter()
        .filter_map(|path| std::fs::read_to_string(path).ok())
        .find_map(|s| s.trim().parse::<u32>().ok()).map_or(1000, u64::from)
}

pub(super) fn streams(source: [u32; 2], mode: [u32; 2], target: u64, texture: u64)
    -> Result<Vec<Vec<u32>>, &'static str>
{
    let layout = layout(source, mode)?;
    let mut stream = Vec::new();
    encode::begin(&mut stream, target);
    encode::clear(&mut stream, 0xFF0C_131F);
    if let Some([x, y, w, h]) = layout.border {
        encode::draw(&mut stream, abi::pipeline::SOLID, 0xFFFF_FFFF,
            &[[x as i32, y as i32, (x + w) as i32, (y + h) as i32]]);
    }
    let mut streams = vec![stream];
    let steps = abi::draw::steps(abi::pipeline::NEAREST_SCALED).ok_or("intent-pipeline")?;
    for (dst, src) in samples(source, layout) {
        let mut params = [0; 16];
        for (slot, value) in params.iter_mut().zip(dst.iter().chain(&src)) { *slot = *value; }
        let [x, y, w, h] = dst;
        let rows = (abi::limits::WORK / steps / u64::from(w)).max(1) as u32;
        for offset in (0..h).step_by(rows as usize) {
            let mut strip = Vec::new();
            encode::begin(&mut strip, target);
            // Clip only the draw rectangle; the source mapping stays the whole tile's.
            abi::draw::encode(&mut strip, abi::pipeline::NEAREST_SCALED, 0, texture, 0, 0,
                &[abi::draw::Instance { rect: [x as i32, (y + offset) as i32,
                    (x + w) as i32, (y + (offset + rows).min(h)) as i32], params }]);
            streams.push(strip);
        }
    }
    Ok(streams)
}

struct Frame { context: u64, texture: u64, target: u64, display: bool }
impl Drop for Frame {
    fn drop(&mut self) {
        if self.display { let _ = graphics::give_back_display(); }
        if self.target != 0 { let _ = gpu::resource_destroy(self.target); }
        if self.texture != 0 { let _ = gpu::resource_destroy(self.texture); }
        if self.context != 0 { let _ = gpu::context_destroy(self.context); }
    }
}

impl Frame {
    fn present(&mut self, picture: &super::JudgedFrame, mode: abi::Mode) -> Result<(), &'static str> {
        let source = [picture.width as u32, picture.height as u32];
        self.context = gpu::context_create().map_err(|_| "intent-context")?;
        let start = super::monotonic_ns().ok_or("intent-clock")?;
        loop {
            let (r, counters) = gpu::counters(self.context);
            if r != 0 { return Err("intent-counters"); }
            match counters.state {
                abi::context_state::LIVE => break,
                abi::context_state::WARMING => {},
                _ => return Err("intent-context-state"),
            }
            if super::monotonic_ns().ok_or("intent-clock")? - start >= 10_000_000_000 {
                return Err("intent-warmup-timeout");
            }
            time::sleep_ms(5).map_err(|_| "intent-warmup-sleep")?;
        }
        self.texture = gpu::texture_create(abi::format::B8G8R8A8, source[0], source[1]).map_err(|_| "intent-texture")?;
        self.target = gpu::scanout_target_create(abi::format::B8G8R8A8, mode.width, mode.height).map_err(|_| "intent-target")?;
        // Transport the verified handoff bytes unchanged; the GPU draws the screen.
        let (r, _) = gpu::upload(self.context, self.texture, [0, 0, source[0], source[1]],
            &picture.pixels, source[0] * 4, abi::upload_source::B8G8R8A8, abi::purpose::IMAGE);
        if r != 0 { return Err("intent-upload"); }
        for stream in streams(source, [mode.width, mode.height], self.target, self.texture)? {
            let (r, submitted) = gpu::submit(self.context, &stream);
            if r != 0 { return Err("intent-submit"); }
            if gpu::wait(self.context, submitted.fence, 10_000_000_000).0 != 0 { return Err("intent-wait"); }
        }
        graphics::take_over_display().map_err(|_| "intent-display")?;
        self.display = true;
        let (r, presented) = gpu::present(self.context, self.target);
        let success = crate::suite_gpu_backend::backend_values(gpu::caps().1.backend).ok_or("intent-backend")?.success;
        if r != 0 || presented.answer != success { return Err("intent-present"); }
        Ok(())
    }

    fn snapshot(&self, mode: abi::Mode) -> Result<([u8; 32], abi::Mode, abi::Mode), &'static str> {
        // Records name device IDs, not userspace handles. Resolve this target's
        // identity through its readback before comparing the latched binding.
        let mut probe = [0; 4];
        let mut expected = [0; abi::record::SIZE];
        if gpu::readback(self.target, [0, 0, 1, 1], &mut probe, 4, &mut expected) != 0 {
            return Err("intent-target-readback");
        }
        let mut pixels = Vec::new();
        let len = mode.width as usize * mode.height as usize * 4;
        pixels.try_reserve_exact(len).map_err(|_| "intent-allocation")?;
        pixels.resize(len, 0);
        let mut record = [0; abi::record::SIZE];
        if gpu::readback(0, [0, 0, mode.width, mode.height], &mut pixels, mode.width * 4, &mut record) != 0 {
            return Err("intent-readback");
        }
        super::screen::validate(&record, mode, gpu::caps().1.backend, 0)?;
        for at in [abi::record::CONTEXT, abi::record::RESOURCE, abi::record::GENERATION] {
            if record[at..at+8] != expected[at..at+8] { return Err("intent-binding"); }
        }
        Ok((super::screen::canonical_hash(&mut pixels, mode)?, mode, mode))
    }
}

pub(super) fn show(suite: &str, case: &str, picture: &super::JudgedFrame, pace: u64) {
    let mut frame = Frame { context: 0, texture: 0, target: 0, display: false };
    let result = gpu::current_mode().map_err(|_| "intent-mode").and_then(|mode|
        frame.present(picture, mode).and_then(|()| frame.snapshot(mode)));
    super::screen::emit(&super::screen::format_record(suite, case, "intent", result));
    if frame.display {
        if time::sleep_ms(pace).is_err() {
            // A hold failure is screen evidence, independent of the published CASE.
            super::emit(&std::format!("SUITE {suite} ERROR intent hold failed for {case}"));
        }
    }
}

//! Text-free intent frames, drawn after the case verdict and its end checkpoint.
use crate::{gpu::{self, abi::{self, stream::encode}}, graphics, time};
use std::vec::Vec;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Layout {
    pub scale: u32,
    pub picture: [u32; 4],
    pub border: Option<[u32; 4]>,
    tiles: Vec<([u32; 4], [u32; 4])>,
}

fn centered(mode: [u32; 2], size: [u32; 2], scale: u32,
    mut tiles: Vec<([u32; 4], [u32; 4])>) -> Layout
{
    let [w, h] = size;
    let (x, y) = ((mode[0] - w) / 2, (mode[1] - h) / 2);
    for (dst, _) in &mut tiles { dst[0] += x; dst[1] += y; }
    Layout { scale, picture: [x, y, w, h],
        border: Some([x - 2, y - 2, w + 4, h + 4]), tiles }
}

/// U11: pack whole source cells into bands, then pages, without resampling.
pub(super) fn layout(picture: [u32; 2], mode: [u32; 2], cells: &[[u32; 4]])
    -> Result<Vec<Layout>, &'static str>
{
    let [w, h] = picture;
    let [mw, mh] = mode;
    if w == 0 || h == 0 || mw <= 64 || mh <= 64 || [w, h, mw, mh].iter().any(|&v| v > 16384) {
        return Err("intent-dimensions");
    }
    if picture == mode {
        return Ok(vec![Layout { scale: 1, picture: [0, 0, w, h], border: None,
            tiles: vec![([0, 0, w, h], [0, 0, w, h])] }]);
    }
    let scale = ((mw - 64) / w).min((mh - 64) / h);
    if scale > 0 || (w <= mw - 4 && h <= mh - 4) {
        let scale = scale.max(1);
        return Ok(vec![centered(mode, [w*scale, h*scale], scale,
            vec![([0, 0, w*scale, h*scale], [0, 0, w, h])])]);
    }
    let (bw, bh) = (mw - 4, mh - 4);
    let mut source = cells.to_vec();
    if source.is_empty() {
        if w <= bw && u64::from(w)*u64::from(h) > u64::from(bw)*u64::from(bh) {
            // Without fixture boundaries, pages end on whole source rows.
            for y in (0..h).step_by(bh as usize) { source.push([0, y, w, bh.min(h-y)]); }
        } else {
            // Wide strips retain their full height. Other folds use small cells;
            // fixture panels supply their actual column/row boundaries instead.
            let (cw, ch) = if h <= bh && w > bw { (bw, h) } else { (32.min(w), 32.min(h)) };
            for y in (0..h).step_by(ch as usize) { for x in (0..w).step_by(cw as usize) {
                source.push([x, y, cw.min(w-x), ch.min(h-y)]);
            } }
        }
    }
    let mut result = Vec::new();
    let (mut x, mut y, mut row_h, mut width) = (0, 0, 0, 0);
    let mut tiles = Vec::new();
    for src in source {
        let [sx, sy, sw, sh] = src;
        if sw == 0 || sh == 0 || sx+sw > w || sy+sh > h || sw > bw || sh > bh {
            return Err("intent-cell");
        }
        if x + sw > bw { x = 0; y += row_h; row_h = 0; }
        if y + sh > bh {
            result.push(centered(mode, [width, y+row_h], 1, std::mem::take(&mut tiles)));
            x = 0; y = 0; row_h = 0; width = 0;
        }
        tiles.push(([x, y, sw, sh], src));
        x += sw; width = width.max(x); row_h = row_h.max(sh);
    }
    if !tiles.is_empty() { result.push(centered(mode, [width, y+row_h], 1, tiles)); }
    Ok(result)
}

pub(super) fn samples(_: [u32; 2], layout: Layout) -> Vec<([u32; 4], [u32; 4])> { layout.tiles }

const MANIFEST: &str = include_str!("../../../scripts/metal/intent-pictures.tsv");

pub(super) fn picture_pages(suite: &str, case: &str, source: [u32; 2], mode: [u32; 2])
    -> Result<Vec<Layout>, &'static str>
{
    let row = MANIFEST.lines().filter(|s| !s.starts_with('#')).find(|s| {
        let mut f = s.split('\t'); f.next(); f.next() == Some(suite) && f.next() == Some(case)
    }).ok_or("intent-inventory")?;
    let fields: Vec<_> = row.split('\t').collect();
    let mut cells = Vec::new();
    if fields[7] != "-" {
        let expected = [fields[3].parse().map_err(|_| "intent-inventory")?,
            fields[4].parse().map_err(|_| "intent-inventory")?];
        if source != expected { return Err("intent-panel-dimensions"); }
        let cuts = |s: &str| s.split(',').map(|n| n.parse::<u32>().map_err(|_| "intent-inventory"))
            .collect::<Result<Vec<_>, _>>();
        let (xs, ys) = (cuts(fields[7])?, cuts(fields[8])?);
        for y in ys.windows(2) { for x in xs.windows(2) {
            cells.push([x[0], y[0], x[1]-x[0], y[1]-y[0]]);
        } }
    }
    layout(source, mode, &cells)
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

pub(super) fn streams(source: [u32; 2], mode: [u32; 2], target: u64, texture: u64, layout: Layout)
    -> Result<Vec<Vec<u32>>, &'static str>
{
    let mut stream = Vec::new();
    encode::begin(&mut stream, target);
    encode::clear(&mut stream, 0xFF5A_5A5A);
    let mut light = Vec::new();
    for y in (0..mode[1]).step_by(16) {
        for x in (0..mode[0]).step_by(16) {
            if (x / 16 + y / 16) % 2 == 1 {
                light.push([x as i32, y as i32,
                    (x + 16).min(mode[0]) as i32, (y + 16).min(mode[1]) as i32]);
            }
        }
    }
    encode::draw(&mut stream, abi::pipeline::SOLID, 0xFF7A_7A7A, &light);
    if let Some([x, y, w, h]) = layout.border {
        // Paint only the border white. A folded page can have an unoccupied
        // tail inside its bounding box; keep that space the panel's navy.
        let (left, top, right, bottom) = (x as i32, y as i32, (x+w) as i32, (y+h) as i32);
        encode::draw(&mut stream, abi::pipeline::SOLID, 0xFFFF_FFFF, &[
            [left, top, right, top+2], [left, bottom-2, right, bottom],
            [left, top+2, left+2, bottom-2], [right-2, top+2, right, bottom-2],
        ]);
        encode::draw(&mut stream, abi::pipeline::SOLID, 0xFF0C_131F,
            &[[left+2, top+2, right-2, bottom-2]]);
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
    fn present(&mut self, picture: &super::JudgedFrame, mode: abi::Mode, layout: Layout) -> Result<(), &'static str> {
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
        for stream in streams(source, [mode.width, mode.height], self.target, self.texture, layout)? {
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

    fn snapshot(&self, suite: &str, case: &str, checkpoint: &str, mode: abi::Mode) -> Result<([u8; 32], abi::Mode, abi::Mode), &'static str> {
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
        let crc = if gpu::caps().1.backend == abi::backend::GK208 {
            super::screen::observe_checkpoint(suite,case,checkpoint,&mut pixels,&mut record).map_err(|_|"intent-readback")?
        } else {
            if gpu::readback(0, [0, 0, mode.width, mode.height], &mut pixels, mode.width * 4, &mut record) != 0 {
                return Err("intent-readback");
            }
            None
        };
        super::screen::validate(&record, mode, gpu::caps().1.backend, 0)?;
        for at in [abi::record::CONTEXT, abi::record::RESOURCE, abi::record::GENERATION] {
            if record[at..at+8] != expected[at..at+8] { return Err("intent-binding"); }
        }
        let hash = super::screen::canonical_hash(&mut pixels, mode)?;
        super::screen::capture(suite, case, checkpoint, mode, hash, &pixels)?;
        if let Some(bytes) = crc { super::screen::intent_crc(bytes)?; }
        Ok((hash, mode, mode))
    }
}

pub(super) fn show(suite: &str, case: &str, picture: &super::JudgedFrame, pace: u64) {
    let result = gpu::current_mode().map_err(|_| "intent-mode").and_then(|mode| {
        let plans = picture_pages(suite, case, [picture.width as u32, picture.height as u32], [mode.width, mode.height])?;
        let count = plans.len();
        for (i, plan) in plans.into_iter().enumerate() {
            let checkpoint = if count == 1 { "intent".into() } else { std::format!("intent-p{}", i+1) };
            let mut frame = Frame { context: 0, texture: 0, target: 0, display: false };
            let captured = frame.present(picture, mode, plan).and_then(|()| frame.snapshot(suite, case, &checkpoint, mode));
            super::screen::emit_intent(suite, case, &checkpoint, captured);
            if frame.display && time::sleep_ms(pace).is_err() {
                super::emit(&std::format!("SUITE {suite} ERROR intent hold failed for {case} {checkpoint}"));
            }
        }
        Ok(())
    });
    if let Err(error) = result {
        super::screen::emit(&super::screen::format_record(suite, case, "intent", Err(error)));
    }
}

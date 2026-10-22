//! bwm's composition on the GPU core (`docs/gui/breengel.md` section 18, ruling B13).
//!
//! bwm draws the desktop (background, taskbar, window frames and app bar) with BreenGL into
//! its own GPU target D. Each frame it reserves a scene snapshot (window op 29), which names
//! each shown window's published front by a sample-only alias, and encodes the scene in
//! userspace: within each damaged rectangle of its persistent scene target C, the desktop,
//! then each window bottom to top (its frame and shadow from D, its content from the alias),
//! the app bar from D and the pointer on top. The stream ends with the copy of C into
//! whichever of the two scanout targets A and B is not shown. COMPOSITE (window op 19)
//! validates and submits that stream, waits for it and presents the target. No client pixel
//! passes through the CPU.
//!
//! The first frame, and each client window's first shown frame, are read back from the
//! scanout and compared pixel for pixel with what the frame must show, built from sources
//! independent of the composition: D read back, each client's committed front read back by
//! CAPTURE (the commit the composite sampled), and the pointer's bitmap. `check_fills` reads
//! single scanout pixels and compares them with the desktop's colours.

use libbreenix::gpu::{self, abi};
use libbreenix::window::{self, abi as wabi};
use abi::stream::{cmd, encode};
use libgfx::surface::{Format, Surface};
use wabi::{op, Plain};

use super::session::cursor_shape;

/// How long bwm waits for the GPU core to admit its device before giving up.
const ADMISSION_WAIT_MS: u32 = 5000;

/// Rows per readback call when a frame is checked.
const CHECK_BAND_ROWS: u32 = 32;

/// How long one composite retries a snapshot that went stale or busy.
const RETRY_MS: u64 = 2000;

const CURSOR_WHITE: u32 = 0xFFFF_FFFF;
const CURSOR_BLACK: u32 = 0xFF00_0000;
const BLACK: u32 = 0xFF00_0000;

/// One client window as the frame shows it, bottom to top.
pub struct Layer {
    pub window_id: u64,
    /// The window's decorations and shadow, drawn from the desktop target beneath its
    /// content. Empty for chromeless windows.
    pub frame: [[i32; 4]; 3],
    pub frame_len: usize,
    /// Where the window's content goes: x, y, width, height.
    pub content: [i32; 4],
}

/// The pointer as the frame draws it.
#[derive(Clone, Copy)]
pub struct Pointer {
    pub x: i32,
    pub y: i32,
    pub shape: u32,
}

/// What a read-back of the scanout (target 0) showed.
pub struct Scanout {
    /// The device resource the display shows, from the read-back's record.
    pub resource: u64,
    pub width: u32,
    pub height: u32,
}

/// Why a frame was not composed.
pub enum FrameError {
    /// bwm is not the display owner holding the compositor claim (EPERM): another process
    /// took the display, and gives it back by giving it back or exiting.
    NotOwner,
    Failed(String),
}

impl core::fmt::Display for FrameError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            FrameError::NotOwner => f.write_str("bwm does not own the display"),
            FrameError::Failed(e) => f.write_str(e),
        }
    }
}

/// Why a frame check did not pass.
pub enum CheckError {
    /// A client committed again after the composite sampled it, so its front no longer
    /// holds the pixels the frame shows; the check is made again on a later frame.
    Moved(String),
    Failed(String),
}

/// A window the last composite sampled: its id, the published commit and client size.
#[derive(Clone, Copy)]
struct Sampled {
    id: u64,
    sample: u64,
    commit_id: u64,
    width: u32,
    height: u32,
}

pub struct Compositor {
    context: u64,
    width: u32,
    height: u32,
    scene: u64,
    scanout: [u64; 2],
    /// The scanout target the next frame writes: the one not shown.
    next: usize,
    /// The desktop: background, taskbar, window frames and app bar.
    pub desktop: Surface,
    /// The background alone, which each repaint of the desktop copies.
    pub background: Surface,
    sampled: Vec<Sampled>,
    appbar_top: i32,
    /// Pointer pixels per bitmap cell: the UI scale.
    scale: i32,
    /// The next frame repaints the whole of C: after opening, and after a failed frame.
    full: bool,
}

impl Drop for Compositor {
    fn drop(&mut self) {
        self.destroy_targets();
        gpu::context_destroy(self.context);
    }
}

impl Compositor {
    /// Open the GPU core for a `width` x `height` screen: a context, the scene target and two
    /// scanout targets, each cleared by a completed GPU clear, and the desktop and background
    /// targets, which the caller draws. The pointer is drawn at `scale` pixels per bitmap cell.
    pub fn open(width: u32, height: u32, appbar_height: u32, scale: u32) -> Result<Self, String> {
        wait_for_admission()?;
        let context = gpu::context_create().map_err(|e| format!("context: errno {}", -e))?;
        let (desktop, background) = match (Surface::new(width as usize, height as usize, Format::BGRX),
            Surface::new(width as usize, height as usize, Format::BGRX)) {
            (Ok(d), Ok(b)) => (d, b),
            (d, b) => {
                gpu::context_destroy(context);
                return Err(format!("desktop targets: {:?}", d.err().or(b.err())));
            }
        };
        let mut c = Self {
            context,
            width,
            height,
            scene: 0,
            scanout: [0; 2],
            next: 0,
            desktop,
            background,
            sampled: Vec::new(),
            appbar_top: height.saturating_sub(appbar_height) as i32,
            scale: scale as i32,
            full: true,
        };
        c.make_targets()?;
        Ok(c)
    }

    fn destroy_targets(&mut self) {
        for t in [self.scene, self.scanout[0], self.scanout[1]] {
            if t != 0 {
                gpu::resource_destroy(t);
            }
        }
        (self.scene, self.scanout) = (0, [0; 2]);
    }

    fn make_targets(&mut self) -> Result<(), String> {
        self.destroy_targets();
        // The scanout targets first: test point 26 binds the process's lowest other scanout.
        for slot in 0..2 {
            self.scanout[slot] = gpu::scanout_target_create(abi::format::B8G8R8A8, self.width, self.height)
                .map_err(|e| format!("scanout target: errno {}", -e))?;
        }
        self.scene = gpu::target_create(abi::format::B8G8R8A8, self.width, self.height)
            .map_err(|e| format!("scene target: errno {}", -e))?;
        self.next = 0;
        self.full = true;
        let mut words = Vec::new();
        for t in [self.scanout[0], self.scanout[1], self.scene] {
            encode::begin(&mut words, t);
            encode::clear(&mut words, BLACK);
        }
        let (r, s) = gpu::submit(self.context, &words);
        if r != 0 {
            return Err(format!("clear: errno {}", -r));
        }
        match gpu::wait(self.context, s.fence, 2_000_000_000) {
            (0, _) => Ok(()),
            (r, w) => Err(format!("clear: errno {}, status {}", -r, w.status)),
        }
    }

    /// Replace the context and its targets after a failed frame (a quarantined context or
    /// a device reset). The caller draws the background and the desktop again.
    pub fn reopen(&mut self) -> Result<(), String> {
        let appbar = self.height - self.appbar_top as u32;
        *self = Self::open(self.width, self.height, appbar, self.scale as u32)?;
        Ok(())
    }

    /// Compose a frame: redraw `damage` (screen rectangles `(x0, y0, x1, y1)`), and whatever
    /// the scene snapshot names as damaged, in C from the desktop, the windows in `layers`
    /// and the pointer, then show it. The whole of C is redrawn after opening or a failure.
    pub fn compose(&mut self, layers: &[Layer], pointer: Option<Pointer>, damage: &[[i32; 4]]) -> Result<(), FrameError> {
        const EPERM: i64 = -(wabi::errno::EPERM as i64);
        const EAGAIN: i64 = -(wabi::errno::EAGAIN as i64);
        const EBUSY: i64 = -(wabi::errno::EBUSY as i64);
        const EINVAL: i64 = -(wabi::errno::EINVAL as i64);
        const EIO: i64 = -(wabi::errno::EIO as i64);
        self.desktop.wait().map_err(|e| FrameError::Failed(format!("desktop drawing: errno {}", -e)))?;
        let deadline = monotonic_ms() + RETRY_MS;
        loop {
            let mut q = wabi::SceneSnapshot::ZERO;
            q.context = self.context;
            let mut r = call(op::COMPOSITE_SNAPSHOT, &mut q);
            let mut windows = vec![wabi::SceneWindow::ZERO; q.window_count as usize];
            let mut kernel_damage = vec![wabi::Rect::ZERO; q.damage_count as usize];
            let mut s = wabi::SceneSnapshot::ZERO;
            if r == 0 {
                if (q.width, q.height) != (self.width, self.height) {
                    return Err(FrameError::Failed(format!("the display changed to {}x{} under bwm's {}x{} desktop", q.width, q.height, self.width, self.height)));
                }
                // A request without room for any window or damage would be a sizing query.
                if windows.is_empty() && kernel_damage.is_empty() {
                    windows.push(wabi::SceneWindow::ZERO);
                }
                s.context = self.context;
                s.windows = windows.as_mut_ptr() as u64;
                s.window_capacity = windows.len() as u32;
                s.damage = kernel_damage.as_mut_ptr() as u64;
                s.damage_capacity = kernel_damage.len() as u32;
                r = call(op::COMPOSITE_SNAPSHOT, &mut s);
            }
            // Stale or busy metadata, or a count that grew since the sizing query.
            if matches!(r, EAGAIN | EBUSY | EINVAL) && monotonic_ms() < deadline {
                let _ = libbreenix::time::sleep_ms(1);
                continue;
            }
            if r == EPERM {
                return Err(FrameError::NotOwner);
            }
            if r != 0 {
                return Err(FrameError::Failed(format!("scene snapshot: errno {}", -r)));
            }
            windows.truncate(s.window_count as usize);
            kernel_damage.truncate(s.damage_count as usize);
            let full = self.full || s.flags & 1 != 0;
            let rects = self.damage_rects(full, damage, &kernel_damage);
            let mut words = Vec::new();
            self.encode(&mut words, &rects, layers, &windows, pointer);
            let mut c = wabi::Composite::ZERO;
            c.snapshot = s.snapshot;
            c.target = self.scanout[self.next];
            c.stream = words.as_ptr() as u64;
            c.length = (words.len() * 4) as u64;
            c.scene_target = self.scene;
            let r = call(op::COMPOSITE, &mut c);
            match r {
                0 => {
                    self.next ^= 1;
                    self.full = false;
                    self.sampled = windows.iter().map(|w| Sampled {
                        id: w.id, sample: w.sample, commit_id: w.commit_id, width: w.width, height: w.height,
                    }).collect();
                    return Ok(());
                }
                EAGAIN if monotonic_ms() < deadline => {
                    let _ = libbreenix::time::sleep_ms(1);
                }
                // The destination is still the scanout's binding: compose into the other one.
                EBUSY if monotonic_ms() < deadline => {
                    self.next ^= 1;
                    let _ = libbreenix::time::sleep_ms(1);
                }
                EPERM => {
                    self.full = true;
                    return Err(FrameError::NotOwner);
                }
                EIO => {
                    self.full = true;
                    return Err(FrameError::Failed(format!("composite: EIO, reason {}, phase {}", c.reason, c.phase)));
                }
                r => {
                    self.full = true;
                    return Err(FrameError::Failed(format!("composite: errno {}, stream error {} at {}", -r, c.error, c.error_index)));
                }
            }
        }
    }

    /// The rectangles to redraw: the whole screen, or bwm's own damage and the snapshot's,
    /// clipped to the screen. More than 16 become their bounding box.
    fn damage_rects(&self, full: bool, own: &[[i32; 4]], kernel: &[wabi::Rect]) -> Vec<[i32; 4]> {
        let (w, h) = (self.width as i32, self.height as i32);
        if full {
            return vec![[0, 0, w, h]];
        }
        let kernel = kernel.iter().map(|r| [r.x, r.y, r.x.saturating_add(r.w), r.y.saturating_add(r.h)]);
        let mut rects: Vec<[i32; 4]> = own.iter().copied().chain(kernel)
            .map(|r| [r[0].max(0), r[1].max(0), r[2].min(w), r[3].min(h)])
            .filter(|r| r[0] < r[2] && r[1] < r[3])
            .collect();
        if rects.len() > 16 {
            let b = rects.iter().fold([w, h, 0, 0], |a, r| [a[0].min(r[0]), a[1].min(r[1]), a[2].max(r[2]), a[3].max(r[3])]);
            rects = vec![b];
        }
        rects
    }

    /// The stream of one frame: within each of `rects` in C, the desktop, the layers bottom to
    /// top, the app bar and the pointer; then the whole of C into the next scanout target.
    fn encode(&self, out: &mut Vec<u32>, rects: &[[i32; 4]], layers: &[Layer], windows: &[wabi::SceneWindow], pointer: Option<Pointer>) {
        let desktop = self.desktop.handle();
        let (w, h) = (self.width as i32, self.height as i32);
        let copy = |out: &mut Vec<u32>, source: u64, origin: [i32; 2], area: [i32; 4], d: [i32; 4]| {
            if let Some(i) = intersect(area, d) {
                let src = [(i[0] - origin[0]) as u32, (i[1] - origin[1]) as u32, (i[2] - i[0]) as u32, (i[3] - i[1]) as u32];
                encode::copy(out, source, src, self.scene, [i[0] as u32, i[1] as u32]);
            }
        };
        if !rects.is_empty() {
            encode::begin(out, self.scene);
        }
        for &d in rects {
            out.extend([cmd::SCISSOR, 6, d[0] as u32, d[1] as u32, d[2] as u32, d[3] as u32]);
            copy(out, desktop, [0, 0], [0, 0, w, h], d);
            for layer in layers {
                for &r in &layer.frame[..layer.frame_len] {
                    copy(out, desktop, [0, 0], r, d);
                }
                if let Some((sample, r)) = content_rect(layer, windows) {
                    copy(out, sample, [r[0], r[1]], r, d);
                }
            }
            // The app bar stays above the windows.
            copy(out, desktop, [0, 0], [0, self.appbar_top, w, h], d);
            if let Some(p) = pointer {
                let (white, black) = pointer_rects(p, self.scale);
                let white: Vec<[i32; 4]> = white.into_iter().filter_map(|r| intersect(r, d)).collect();
                let black: Vec<[i32; 4]> = black.into_iter().filter_map(|r| intersect(r, d)).collect();
                if !white.is_empty() {
                    encode::draw(out, abi::pipeline::SOLID, CURSOR_WHITE, &white);
                }
                if !black.is_empty() {
                    encode::draw(out, abi::pipeline::SOLID, CURSOR_BLACK, &black);
                }
            }
        }
        encode::begin(out, self.scanout[self.next]);
        encode::copy(out, self.scene, [0, 0, self.width, self.height], self.scanout[self.next], [0, 0]);
    }

    /// The client windows a frame composed from `layers` shows, top first: each window's id
    /// and the screen area its decorations and content cover (x0, y0, x1, y1).
    pub fn shown_clients(&self, layers: &[Layer], pointer: Option<Pointer>) -> Vec<(u64, [u32; 4])> {
        layers.iter().enumerate().rev().filter_map(|(index, l)| {
            let content = self.sampled_content(l)?;
            let area = l.frame[..l.frame_len].iter().fold(content, |a, r| {
                [a[0].min(r[0]), a[1].min(r[1]), a[2].max(r[2]), a[3].max(r[3])]
            });
            let clip = |v: i32, max: u32| v.clamp(0, max as i32) as u32;
            let area = [clip(area[0], self.width), clip(area[1], self.height), clip(area[2], self.width), clip(area[3], self.height)];
            // Keep only client pixels that compose actually leaves on screen. A
            // bounding box alone admits clients hidden by other layers.
            let mut exposed = vec![[content[0].max(0), content[1].max(0),
                content[2].min(self.width as i32), content[3].min(self.appbar_top)]];
            exposed.retain(|r| r[0] < r[2] && r[1] < r[3]);
            for above in &layers[index + 1..] {
                for rect in &above.frame[..above.frame_len] {
                    subtract_rect(&mut exposed, *rect);
                }
                if let Some(rect) = self.sampled_content(above) {
                    subtract_rect(&mut exposed, rect);
                }
            }
            if let Some(p) = pointer {
                let (white, black) = pointer_rects(p, self.scale);
                for rect in white.into_iter().chain(black) {
                    subtract_rect(&mut exposed, rect);
                }
            }
            (!exposed.is_empty()).then_some((l.window_id, area))
        }).collect()
    }

    /// The content rectangle the last composite drew from a layer's window, when it sampled
    /// a published front: the content area, no larger than the client's frame.
    fn sampled_content(&self, layer: &Layer) -> Option<[i32; 4]> {
        let s = self.sampled.iter().find(|s| s.id == layer.window_id && s.sample != 0)?;
        let [x, y, cw, ch] = layer.content;
        let (w, h) = (cw.min(s.width as i32), ch.min(s.height as i32));
        (w > 0 && h > 0).then_some([x, y, x + w, y + h])
    }

    /// Read the displayed frame back from the device and compare each pixel with what the
    /// last frame composed from `layers` must show. Returns the number of pixels compared.
    pub fn check(&self, layers: &[Layer], pointer: Option<Pointer>) -> Result<u64, CheckError> {
        self.check_area(layers, pointer, [0, 0, self.width, self.height])
    }

    /// `check`, for the part of the frame inside `area` (x0, y0, x1, y1) only.
    pub fn check_area(&self, layers: &[Layer], pointer: Option<Pointer>, area: [u32; 4]) -> Result<u64, CheckError> {
        let [x0, y_top, x1, y1] = area;
        let w = x1.saturating_sub(x0);
        if w == 0 || y_top >= y1 {
            return Err(CheckError::Failed(format!("the area ({}, {})..({}, {}) is empty", x0, y_top, x1, y1)));
        }
        // The clients this area shows, read back first: each must still publish the commit
        // the composite sampled.
        let mut clients: Vec<(u64, [i32; 4], u32, Vec<u8>)> = Vec::new();
        for layer in layers {
            let Some(r) = self.sampled_content(layer) else { continue };
            if intersect(r, [x0 as i32, y_top as i32, x1 as i32, y1 as i32]).is_none() {
                continue;
            }
            let s = self.sampled.iter().find(|s| s.id == layer.window_id).expect("sampled_content found it");
            let mut px = prefaulted_bytes(s.width as usize * s.height as usize * 4);
            let c = window::capture(s.id, &mut px, None)
                .map_err(|e| CheckError::Failed(format!("CAPTURE of window {}: {}", s.id, e)))?;
            if c.commit_id != s.commit_id {
                return Err(CheckError::Moved(format!(
                    "window {} published commit {} after the composite sampled commit {}", s.id, c.commit_id, s.commit_id
                )));
            }
            clients.push((s.id, r, s.width, px));
        }
        let mut band = prefaulted_bytes((w * CHECK_BAND_ROWS * 4) as usize);
        let mut desk = prefaulted_bytes((w * CHECK_BAND_ROWS * 4) as usize);
        let mut record = Box::new([0u8; abi::record::SIZE]);
        // SAFETY: the record is one live allocation.
        unsafe { core::ptr::write_volatile(record.as_mut_ptr(), 0) };
        let pointer = pointer.map(|p| (p, pointer_bitmap(p.shape)));
        let mut compared = 0u64;
        let mut y0 = y_top;
        while y0 < y1 {
            let rows = CHECK_BAND_ROWS.min(y1 - y0);
            let len = (w * rows * 4) as usize;
            let (r, a) = gpu::readback_why(0, [x0, y0, w, rows], &mut band[..len], w * 4, &mut *record);
            if r != 0 {
                return Err(CheckError::Failed(format!("readback of rows {}..{}: errno {}, reason {}", y0, y0 + rows, -r, a.reason)));
            }
            self.desktop.read_rect([x0 as usize, y0 as usize, w as usize, rows as usize], &mut desk[..len], w as usize * 4)
                .map_err(|e| CheckError::Failed(format!("desktop readback of rows {}..{}: errno {}", y0, y0 + rows, -e)))?;
            for row in 0..rows {
                let y = (y0 + row) as i32;
                for x in x0 as i32..x1 as i32 {
                    let o = ((row * w) as usize + (x as u32 - x0) as usize) * 4;
                    let got = u32::from_le_bytes([band[o], band[o + 1], band[o + 2], band[o + 3]]);
                    let from_desktop = u32::from_le_bytes([desk[o], desk[o + 1], desk[o + 2], desk[o + 3]]);
                    let want = self.expected(from_desktop, layers, &clients, pointer, x, y);
                    // The X byte is not compared (breengel.md section 2).
                    if got & 0x00FF_FFFF != want & 0x00FF_FFFF {
                        return Err(CheckError::Failed(format!(
                            "pixel ({}, {}) is {:#010x}, the frame must show {:#010x} ({} pixels matched before it)",
                            x, y, got, want, compared
                        )));
                    }
                    compared += 1;
                }
            }
            y0 += rows;
        }
        Ok(compared)
    }

    /// Read the scanout itself (READBACK target 0) at each `(x, y, want)` sample and require
    /// the pixel `want`, and require each sample's record to describe the display's completed
    /// binding: the scanout flag set, a `width` x `height` resource, the same resource each
    /// time. Returns that resource and its size.
    pub fn check_fills(&self, samples: &[(u32, u32, u32)]) -> Result<Scanout, String> {
        let mut pixel = prefaulted_bytes(4);
        let mut record = Box::new([0u8; abi::record::SIZE]);
        // SAFETY: the record is one live allocation.
        unsafe { core::ptr::write_volatile(record.as_mut_ptr(), 0) };
        let mut shown: Option<Scanout> = None;
        for &(x, y, want) in samples {
            let (r, a) = gpu::readback_why(0, [x, y, 1, 1], &mut pixel, 4, &mut *record);
            if r != 0 {
                return Err(format!("scanout readback of ({}, {}): errno {}, reason {}", x, y, -r, a.reason));
            }
            let u32_at = |o: usize| u32::from_le_bytes([record[o], record[o + 1], record[o + 2], record[o + 3]]);
            let flags = u32_at(abi::record::FLAGS);
            let (width, height) = (u32_at(abi::record::TARGET_WIDTH), u32_at(abi::record::TARGET_HEIGHT));
            let resource = u32_at(abi::record::RESOURCE) as u64 | (u32_at(abi::record::RESOURCE + 4) as u64) << 32;
            if flags & 1 == 0 {
                return Err(format!("the read-back of ({}, {}) is not of the scanout (record flags {:#x})", x, y, flags));
            }
            if (width, height) != (self.width, self.height) {
                return Err(format!(
                    "the scanout is resource {} at {}x{}, bwm composed {}x{}",
                    resource, width, height, self.width, self.height
                ));
            }
            if let Some(s) = &shown {
                if s.resource != resource {
                    return Err(format!("the scanout changed from resource {} to {} during the check", s.resource, resource));
                }
            }
            let got = u32::from_le_bytes([pixel[0], pixel[1], pixel[2], pixel[3]]);
            if got & 0x00FF_FFFF != want & 0x00FF_FFFF {
                return Err(format!("scanout pixel ({}, {}) is {:#010x}, the desktop has {:#010x}", x, y, got, want));
            }
            shown = Some(Scanout { resource, width, height });
        }
        shown.ok_or_else(|| String::from("no samples"))
    }

    /// The pixel the frame must show at `(x, y)`: the pointer's, else the topmost layer's
    /// (its client's committed pixel over its content, the desktop's over its frame), else
    /// the desktop's; the app bar's above the windows.
    fn expected(&self, from_desktop: u32, layers: &[Layer], clients: &[(u64, [i32; 4], u32, Vec<u8>)],
        pointer: Option<(Pointer, &[&[u8]])>, x: i32, y: i32) -> u32 {
        if let Some((p, bitmap)) = pointer {
            let (hx, hy) = hotspot(p.shape);
            let s = self.scale;
            let (dx, dy) = (x - (p.x - hx * s), y - (p.y - hy * s));
            let (cx, cy) = (dx.div_euclid(s), dy.div_euclid(s));
            if cy >= 0 && (cy as usize) < bitmap.len() && cx >= 0 && (cx as usize) < bitmap[cy as usize].len() {
                match bitmap[cy as usize][cx as usize] {
                    b'#' => return CURSOR_BLACK,
                    b'o' => return CURSOR_WHITE,
                    _ => {}
                }
            }
        }
        if y >= self.appbar_top {
            return from_desktop;
        }
        for layer in layers.iter().rev() {
            if let Some((_, r, width, px)) = clients.iter().find(|c| c.0 == layer.window_id) {
                if x >= r[0] && x < r[2] && y >= r[1] && y < r[3] {
                    let o = ((y - r[1]) as usize * *width as usize + (x - r[0]) as usize) * 4;
                    return u32::from_le_bytes([px[o], px[o + 1], px[o + 2], px[o + 3]]);
                }
            }
            if layer.frame[..layer.frame_len].iter().any(|r| x >= r[0] && x < r[2] && y >= r[1] && y < r[3]) {
                return from_desktop;
            }
        }
        from_desktop
    }
}

/// The alias and destination rectangle of a layer's content, when the snapshot names a
/// published front for its window: the content area, no larger than the client's frame.
fn content_rect(layer: &Layer, windows: &[wabi::SceneWindow]) -> Option<(u64, [i32; 4])> {
    let s = windows.iter().find(|w| w.id == layer.window_id && w.sample != 0)?;
    let [x, y, cw, ch] = layer.content;
    let (w, h) = (cw.min(s.width as i32), ch.min(s.height as i32));
    (w > 0 && h > 0).then_some((s.sample, [x, y, x + w, y + h]))
}

fn intersect(a: [i32; 4], b: [i32; 4]) -> Option<[i32; 4]> {
    let r = [a[0].max(b[0]), a[1].max(b[1]), a[2].min(b[2]), a[3].min(b[3])];
    (r[2] > r[0] && r[3] > r[1]).then_some(r)
}

/// The window op `op` on `value`: the kernel's raw return.
fn call<T: Plain>(op: u32, value: &mut T) -> i64 {
    // SAFETY: `value` is a live plain structure of its own size.
    unsafe { window::raw_call(op, value as *mut T as *mut u8, core::mem::size_of::<T>()) }
}

fn monotonic_ms() -> u64 {
    libbreenix::time::now_monotonic().map_or(0, |t| (t.as_nanos() / 1_000_000) as u64)
}

/// Remove the part of each rectangle covered by an opaque rectangle.
fn subtract_rect(rects: &mut Vec<[i32; 4]>, cover: [i32; 4]) {
    let mut remaining = Vec::new();
    for r in rects.drain(..) {
        let cut = [r[0].max(cover[0]), r[1].max(cover[1]),
            r[2].min(cover[2]), r[3].min(cover[3])];
        if cut[0] >= cut[2] || cut[1] >= cut[3] {
            remaining.push(r);
            continue;
        }
        for part in [[r[0], r[1], r[2], cut[1]], [r[0], cut[3], r[2], r[3]],
            [r[0], cut[1], cut[0], cut[3]], [cut[2], cut[1], r[2], cut[3]]] {
            if part[0] < part[2] && part[1] < part[3] {
                remaining.push(part);
            }
        }
    }
    *rects = remaining;
}

/// The display the GPU core shows (`MODES`, gpu.md section 1.7): the current mode's index,
/// once the core has admitted its device.
pub fn display() -> Result<(u32, abi::Mode), String> {
    wait_for_admission()?;
    let (r, m) = gpu::modes();
    if r != 0 {
        return Err(format!("MODES: errno {}", -r));
    }
    let mode = m.modes.get(m.current as usize).copied().ok_or_else(|| format!("MODES: current mode {} of {}", m.current, m.count))?;
    Ok((m.current, mode))
}

/// The device resource the display shows, while its binding is known (`MODES`).
pub fn scanout_resource() -> Result<u64, String> {
    let (r, m) = gpu::modes();
    if r != 0 {
        return Err(format!("MODES: errno {}", -r));
    }
    if m.binding_known == 0 {
        return Err(String::from("MODES: the binding is not known"));
    }
    Ok(m.scanout_resource)
}

/// Wait, bounded, for the GPU core to admit its device.
fn wait_for_admission() -> Result<(), String> {
    let mut waited = 0;
    loop {
        let (r, caps) = gpu::caps();
        if r == 0 && caps.status == abi::status::ADMITTED {
            return Ok(());
        }
        if waited >= ADMISSION_WAIT_MS {
            let reason = caps.reason.iter().take_while(|&&b| b != 0).map(|&b| b as char).collect::<String>();
            return Err(if r != 0 {
                format!("CAPS: errno {}", -r)
            } else {
                format!("the GPU core has not admitted a device (status {}, {})", caps.status, reason)
            });
        }
        let _ = libbreenix::time::sleep_ms(100);
        waited += 100;
    }
}

/// A zeroed buffer whose pages are mapped, for the kernel's copy into it.
fn prefaulted_bytes(len: usize) -> Vec<u8> {
    let mut v = vec![0u8; len];
    for i in (0..len).step_by(4096) {
        // SAFETY: `i` is inside the vector.
        unsafe { core::ptr::write_volatile(v.as_mut_ptr().add(i), 0) };
    }
    v
}

// ─── Pointer ────────────────────────────────────────────────────────────────
// `#` is the outline, `o` the fill, anything else transparent.

const ARROW: &[&[u8]] = &[
    b"#",
    b"##",
    b"#o#",
    b"#oo#",
    b"#ooo#",
    b"#oooo#",
    b"#ooooo#",
    b"#oooooo#",
    b"#ooooooo#",
    b"#oooooooo#",
    b"#ooooooooo#",
    b"#oooooo#####",
    b"#oooo#",
    b"#oo#oo#",
    b"#o# #oo#",
    b"##  #oo#",
    b"#    #o#",
    b"     ##",
];

const RESIZE_NS: &[&[u8]] = &[
    b"    #",
    b"   #o#",
    b"  #ooo#",
    b" #ooooo#",
    b"####o####",
    b"   #o#",
    b"   #o#",
    b"   #o#",
    b"   #o#",
    b"   #o#",
    b"   #o#",
    b"####o####",
    b" #ooooo#",
    b"  #ooo#",
    b"   #o#",
    b"    #",
];

const RESIZE_EW: &[&[u8]] = &[
    b"    #      #",
    b"   ##      ##",
    b"  #o#      #o#",
    b" #oo########oo#",
    b"#oooooooooooooo#",
    b" #oo########oo#",
    b"  #o#      #o#",
    b"   ##      ##",
    b"    #      #",
];

const RESIZE_NWSE: &[&[u8]] = &[
    b"######",
    b"#oooo#",
    b"#ooo#",
    b"#oooo#",
    b"#o##oo#",
    b"##  #oo#",
    b"     #oo#",
    b"      #oo#",
    b"       #oo##",
    b"        #oo#o#",
    b"         #oooo#",
    b"          #ooo#",
    b"         #oooo#",
    b"         ######",
];

const RESIZE_NESW: &[&[u8]] = &[
    b"         ######",
    b"         #oooo#",
    b"          #ooo#",
    b"         #oooo#",
    b"        #oo##o#",
    b"       #oo#  ##",
    b"      #oo#",
    b"     #oo#",
    b"   ##oo#",
    b"  #o#oo#",
    b" #oooo#",
    b" #ooo#",
    b" #oooo#",
    b" ######",
];

fn pointer_bitmap(shape: u32) -> &'static [&'static [u8]] {
    match shape {
        cursor_shape::RESIZE_NS => RESIZE_NS,
        cursor_shape::RESIZE_EW => RESIZE_EW,
        cursor_shape::RESIZE_NWSE => RESIZE_NWSE,
        cursor_shape::RESIZE_NESW => RESIZE_NESW,
        _ => ARROW,
    }
}

/// The bitmap cell that sits at the pointer's position.
fn hotspot(shape: u32) -> (i32, i32) {
    match shape {
        cursor_shape::RESIZE_NS => (4, 8),
        cursor_shape::RESIZE_EW => (7, 4),
        cursor_shape::RESIZE_NWSE | cursor_shape::RESIZE_NESW => (7, 7),
        _ => (0, 0),
    }
}

/// The screen area the pointer covers at `p` (x0, y0, x1, y1).
pub fn pointer_footprint(p: Pointer, scale: i32) -> [i32; 4] {
    let (hx, hy) = hotspot(p.shape);
    let bitmap = pointer_bitmap(p.shape);
    let cols = bitmap.iter().map(|r| r.len()).max().unwrap_or(0) as i32;
    let (ox, oy) = (p.x - hx * scale, p.y - hy * scale);
    [ox, oy, ox + cols * scale, oy + bitmap.len() as i32 * scale]
}

/// The pointer's fill and outline as horizontal runs of rectangles, `scale` pixels per cell.
fn pointer_rects(p: Pointer, scale: i32) -> (Vec<[i32; 4]>, Vec<[i32; 4]>) {
    let (hx, hy) = hotspot(p.shape);
    let (ox, oy) = (p.x - hx * scale, p.y - hy * scale);
    let mut white = Vec::new();
    let mut black = Vec::new();
    for (row, cells) in pointer_bitmap(p.shape).iter().enumerate() {
        let y = oy + row as i32 * scale;
        let mut col = 0;
        while col < cells.len() {
            let c = cells[col];
            let start = col;
            while col < cells.len() && cells[col] == c {
                col += 1;
            }
            let rect = [ox + start as i32 * scale, y, ox + col as i32 * scale, y + scale];
            match c {
                b'#' => black.push(rect),
                b'o' => white.push(rect),
                _ => {}
            }
        }
    }
    (white, black)
}

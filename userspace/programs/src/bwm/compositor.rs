//! bwm's composition on the GPU core (syscall 416, `docs/gui/gpu.md`).
//!
//! bwm draws the desktop (background, taskbar, window frames and app bar) on the CPU into
//! one screen-sized buffer. Each frame uploads the changed part of that
//! buffer to a texture, uploads the client frames the kernel hands over
//! (`READ_WINDOW_BUFFER`, which also releases the client waiting on it), draws both into
//! one of two scanout targets in stacking order, draws the pointer, and presents the target.
//!
//! The first frame and the first frame that shows a client's pixels are read back from the
//! scanout and compared, pixel for pixel, with the frame bwm meant to show. `check_fills`
//! reads the scanout itself back (target 0) to show that the frame bwm composed is what the
//! display shows, whole.

use bgl::Context;
use libbreenix::gpu::{self, abi};
use libbreenix::graphics;

/// How long bwm waits for the GPU core to admit its device before giving up.
const ADMISSION_WAIT_MS: u32 = 5000;

/// Rows per readback call when a frame is checked.
const CHECK_BAND_ROWS: u32 = 32;

const CURSOR_WHITE: u32 = 0xFFFF_FFFF;
const CURSOR_BLACK: u32 = 0xFF00_0000;

/// One client window as the frame shows it, bottom to top.
pub struct Layer {
    pub window_id: u32,
    /// The window's decorations and shadow, drawn from the desktop texture beneath its
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

struct Surface {
    window_id: u32,
    texture: u64,
    width: u32,
    height: u32,
    pixels: Vec<u32>,
    /// The CPU snapshot holds a complete client frame, even if its upload failed.
    valid: bool,
    /// The texture holds the CPU snapshot.
    shown: bool,
}

impl Surface {
    fn upload(&mut self, context: u64) -> Result<(), String> {
        if self.texture == 0 {
            self.texture = gpu::texture_create(abi::format::B8G8R8A8, self.width, self.height)
                .map_err(|e| format!("window {} texture: errno {}", self.window_id, -e))?;
        }
        let (r, _) = gpu::upload(
            context,
            self.texture,
            [0, 0, self.width, self.height],
            as_bytes(&self.pixels),
            self.width * 4,
            abi::upload_source::B8G8R8A8,
            abi::purpose::IMAGE,
        );
        if r != 0 {
            return Err(format!("window {} upload: errno {}", self.window_id, -r));
        }
        self.shown = true;
        Ok(())
    }
}

pub struct Compositor {
    ctx: Context,
    width: u32,
    height: u32,
    targets: [u64; 2],
    next: usize,
    desktop: u64,
    surfaces: Vec<Surface>,
    read_pixels: Vec<u32>,
    appbar_top: i32,
    /// Pointer pixels per bitmap cell: the UI scale.
    scale: i32,
}

impl Compositor {
    /// Open the GPU core for a `width` x `height` screen: a context, two scanout targets and
    /// the desktop texture. The pointer is drawn at `scale` pixels per bitmap cell.
    pub fn open(width: u32, height: u32, appbar_height: u32, scale: u32) -> Result<Self, String> {
        wait_for_admission()?;
        let ctx = Context::new().map_err(|e| format!("context: errno {}", -e))?;
        let mut targets = [0u64; 2];
        for t in &mut targets {
            *t = gpu::scanout_target_create(abi::format::B8G8R8A8, width, height)
                .map_err(|e| format!("scanout target: errno {}", -e))?;
        }
        let desktop = gpu::texture_create(abi::format::B8G8R8A8, width, height)
            .map_err(|e| format!("desktop texture: errno {}", -e))?;
        Ok(Self {
            ctx,
            width,
            height,
            targets,
            next: 0,
            desktop,
            surfaces: Vec::new(),
            read_pixels: Vec::new(),
            appbar_top: height.saturating_sub(appbar_height) as i32,
            scale: scale as i32,
        })
    }

    /// Replace the context and its resources after a failed frame (a quarantined context
    /// or a device reset), and upload again the client frames already taken. The caller
    /// uploads the desktop again.
    pub fn reopen(&mut self) -> Result<(), String> {
        for t in self.targets.into_iter().chain([self.desktop]).chain(self.surfaces.iter().map(|s| s.texture)) {
            if t != 0 {
                let _ = gpu::resource_destroy(t);
            }
        }
        let mut fresh = Self::open(self.width, self.height, self.height - self.appbar_top as u32, self.scale as u32)?;
        fresh.surfaces = core::mem::take(&mut self.surfaces);
        fresh.read_pixels = core::mem::take(&mut self.read_pixels);
        for s in &mut fresh.surfaces {
            s.texture = 0;
            s.shown = false;
        }
        // Install first: a failed restoration must retain the snapshots for another retry.
        *self = fresh;
        for s in &mut self.surfaces {
            if s.valid {
                s.upload(self.ctx.handle())?;
            }
        }
        Ok(())
    }

    /// Upload the rectangle `(x, y, w, h)` of the desktop buffer.
    pub fn upload_desktop(&mut self, desktop: &[u32], rect: [u32; 4]) -> Result<(), String> {
        let [x, y, w, h] = rect;
        let x1 = (x + w).min(self.width);
        let y1 = (y + h).min(self.height);
        if x >= x1 || y >= y1 {
            return Ok(());
        }
        let stride = self.width * 4;
        let start = (y * self.width + x) as usize;
        let end = ((y1 - 1) * self.width + x1) as usize;
        let bytes = as_bytes(&desktop[start..end]);
        let (r, _) = gpu::upload(
            self.ctx.handle(),
            self.desktop,
            [x, y, x1 - x, y1 - y],
            bytes,
            stride,
            abi::upload_source::B8G8R8A8,
            abi::purpose::IMAGE,
        );
        if r != 0 {
            return Err(format!("desktop upload: errno {}", -r));
        }
        Ok(())
    }

    /// Take the client frames the kernel has for the registered windows and upload them.
    /// Returns whether any window's texture changed.
    pub fn refresh_windows(&mut self) -> Result<bool, String> {
        let mut infos = [graphics::WindowInfo::default(); 16];
        let count = graphics::list_windows(&mut infos).map_err(|e| format!("list windows: {}", e))?;
        let infos = &infos[..count];
        let mut gone = Vec::new();
        for s in &self.surfaces {
            if !infos.iter().any(|i| i.buffer_id == s.window_id) {
                gone.push(s.texture);
            }
        }
        for texture in gone {
            let _ = gpu::resource_destroy(texture);
        }
        self.surfaces.retain(|s| infos.iter().any(|i| i.buffer_id == s.window_id));

        let mut changed = false;
        for info in infos {
            if info.buffer_id == 0 || info.width == 0 || info.height == 0 {
                continue;
            }
            let index = match self.surfaces.iter().position(|s| s.window_id == info.buffer_id) {
                Some(i) => i,
                None => {
                    self.surfaces.push(Surface {
                        window_id: info.buffer_id,
                        texture: 0,
                        width: 0,
                        height: 0,
                        pixels: Vec::new(),
                        valid: false,
                        shown: false,
                    });
                    self.surfaces.len() - 1
                }
            };
            let s = &mut self.surfaces[index];
            let wanted = info.width as usize * info.height as usize;
            if self.read_pixels.len() < wanted {
                self.read_pixels = prefaulted(wanted);
            }
            let mut frame = None;
            for _ in 0..3 {
                let (w, h) = match graphics::read_window_buffer(s.window_id, as_bytes_mut(&mut self.read_pixels)) {
                    Ok(dims) => dims,
                    Err(_) => break,
                };
                if w == 0 && h == 0 {
                    break; // no new frame; keep the previous snapshot
                }
                let needed = w as usize * h as usize;
                if needed > self.read_pixels.len() {
                    // The kernel retained the generation and the blocked presenter.
                    self.read_pixels = prefaulted(needed);
                    continue;
                }
                self.read_pixels.truncate(needed);
                frame = Some((w, h));
                break;
            }
            let Some((w, h)) = frame else { continue };
            if s.width != w || s.height != h {
                if s.texture != 0 {
                    let _ = gpu::resource_destroy(s.texture);
                    s.texture = 0;
                }
            }
            core::mem::swap(&mut s.pixels, &mut self.read_pixels);
            s.width = w;
            s.height = h;
            s.valid = true;
            s.shown = false;
            s.upload(self.ctx.handle())?;
            changed = true;
        }
        Ok(changed)
    }

    /// Draw the frame and present it.
    pub fn compose(&mut self, layers: &[Layer], pointer: Option<Pointer>) -> Result<(), String> {
        let (w, h) = (self.width as i32, self.height as i32);
        let target = self.targets[self.next];
        self.ctx.begin(target, self.width, self.height);
        self.ctx.copy(self.desktop, &[[0, 0, w, h, 0, 0]]);
        for layer in layers {
            let frame: Vec<[i32; 6]> = layer.frame[..layer.frame_len]
                .iter()
                .map(|r| [r[0], r[1], r[2], r[3], r[0], r[1]])
                .collect();
            if !frame.is_empty() {
                self.ctx.copy(self.desktop, &frame);
            }
            if let Some((texture, rect)) = self.content_rect(layer) {
                self.ctx.copy(texture, &[[rect[0], rect[1], rect[2], rect[3], 0, 0]]);
            }
        }
        // The app bar stays above the windows.
        self.ctx.copy(self.desktop, &[[0, self.appbar_top, w, h, 0, self.appbar_top]]);
        if let Some(p) = pointer {
            let (white, black) = pointer_rects(p, self.scale);
            if !white.is_empty() {
                self.ctx.solid(CURSOR_WHITE, &white);
            }
            if !black.is_empty() {
                self.ctx.solid(CURSOR_BLACK, &black);
            }
        }
        self.ctx.submit().map_err(|e| format!("submit: errno {}", -e))?;
        self.ctx.present(target).map_err(|e| format!("present: errno {}", -e))?;
        self.next ^= 1;
        Ok(())
    }

    /// Whether a frame composed from `layers` shows any client pixels.
    pub fn shows_client(&self, layers: &[Layer]) -> Option<u32> {
        layers.iter().rev().find_map(|l| {
            let (_, r) = self.content_rect(l)?;
            let visible = r[0].max(0) < r[2].min(self.width as i32) && r[1].max(0) < r[3].min(self.height as i32);
            visible.then_some(l.window_id)
        })
    }

    /// Read the displayed frame back from the device and compare each pixel with the frame
    /// `compose` drew from the same inputs. Returns the number of pixels compared.
    pub fn check(&self, desktop: &[u32], layers: &[Layer], pointer: Option<Pointer>) -> Result<u64, String> {
        let (w, h) = (self.width, self.height);
        // READBACK refuses pages that are not yet mapped, so both buffers are touched first.
        let mut band = prefaulted((w * CHECK_BAND_ROWS) as usize);
        let band = as_bytes_mut(&mut band);
        let mut record = Box::new([0u8; abi::record::SIZE]);
        // SAFETY: the record is one live allocation.
        unsafe { core::ptr::write_volatile(record.as_mut_ptr(), 0) };
        let pointer = pointer.map(|p| (p, pointer_bitmap(p.shape)));
        let mut compared = 0u64;
        let mut y0 = 0;
        while y0 < h {
            let rows = CHECK_BAND_ROWS.min(h - y0);
            let len = (w * rows * 4) as usize;
            let (r, a) = gpu::readback_why(0, [0, y0, w, rows], &mut band[..len], w * 4, &mut *record);
            if r != 0 {
                return Err(format!("readback of rows {}..{}: errno {}, reason {}", y0, y0 + rows, -r, a.reason));
            }
            for row in 0..rows {
                let y = (y0 + row) as i32;
                for x in 0..w as i32 {
                    let o = ((row * w) as usize + x as usize) * 4;
                    let got = u32::from_le_bytes([band[o], band[o + 1], band[o + 2], band[o + 3]]);
                    let want = self.expected(desktop, layers, pointer, x, y);
                    if got != want {
                        return Err(format!(
                            "pixel ({}, {}) is {:#010x}, the composed frame has {:#010x} ({} pixels matched before it)",
                            x, y, got, want, compared
                        ));
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
        let mut pixel = prefaulted(1);
        let mut record = Box::new([0u8; abi::record::SIZE]);
        // SAFETY: the record is one live allocation.
        unsafe { core::ptr::write_volatile(record.as_mut_ptr(), 0) };
        let mut shown: Option<Scanout> = None;
        for &(x, y, want) in samples {
            let (r, a) = gpu::readback_why(0, [x, y, 1, 1], as_bytes_mut(&mut pixel), 4, &mut *record);
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
            if pixel[0] != want {
                return Err(format!("scanout pixel ({}, {}) is {:#010x}, bwm composed {:#010x}", x, y, pixel[0], want));
            }
            shown = Some(Scanout { resource, width, height });
        }
        shown.ok_or_else(|| String::from("no samples"))
    }

    /// The pixel `compose` draws at `(x, y)`, worked out on the CPU from the same inputs.
    fn expected(&self, desktop: &[u32], layers: &[Layer], pointer: Option<(Pointer, &[&[u8]])>, x: i32, y: i32) -> u32 {
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
        let from_desktop = desktop[y as usize * self.width as usize + x as usize];
        if y >= self.appbar_top {
            return from_desktop;
        }
        for layer in layers.iter().rev() {
            if let Some((_, r)) = self.content_rect(layer) {
                if x >= r[0] && x < r[2] && y >= r[1] && y < r[3] {
                    let s = self.surfaces.iter().find(|s| s.window_id == layer.window_id).unwrap();
                    return s.pixels[(y - r[1]) as usize * s.width as usize + (x - r[0]) as usize];
                }
            }
            if layer.frame[..layer.frame_len].iter().any(|r| x >= r[0] && x < r[2] && y >= r[1] && y < r[3]) {
                return from_desktop;
            }
        }
        from_desktop
    }

    /// The texture and destination rectangle of a layer's content, when its window has a
    /// frame to show: the content area, no larger than the client's frame.
    fn content_rect(&self, layer: &Layer) -> Option<(u64, [i32; 4])> {
        let s = self.surfaces.iter().find(|s| s.window_id == layer.window_id && s.shown)?;
        let [x, y, cw, ch] = layer.content;
        let (w, h) = (cw.min(s.width as i32), ch.min(s.height as i32));
        (w > 0 && h > 0).then_some((s.texture, [x, y, x + w, y + h]))
    }
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

/// A zeroed pixel buffer whose pages are mapped, for the kernel's copy into it.
fn prefaulted(len: usize) -> Vec<u32> {
    let mut v = vec![0u32; len];
    for i in (0..len).step_by(1024) {
        // SAFETY: `i` is inside the vector.
        unsafe { core::ptr::write_volatile(v.as_mut_ptr().add(i), 0) };
    }
    v
}

fn as_bytes(p: &[u32]) -> &[u8] {
    // SAFETY: u32 has no padding and u8 has alignment 1.
    unsafe { core::slice::from_raw_parts(p.as_ptr() as *const u8, p.len() * 4) }
}

fn as_bytes_mut(p: &mut [u32]) -> &mut [u8] {
    // SAFETY: as `as_bytes`; u32 has no invalid bit patterns.
    unsafe { core::slice::from_raw_parts_mut(p.as_mut_ptr() as *mut u8, p.len() * 4) }
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
        graphics::cursor_shape::RESIZE_NS => RESIZE_NS,
        graphics::cursor_shape::RESIZE_EW => RESIZE_EW,
        graphics::cursor_shape::RESIZE_NWSE => RESIZE_NWSE,
        graphics::cursor_shape::RESIZE_NESW => RESIZE_NESW,
        _ => ARROW,
    }
}

/// The bitmap cell that sits at the pointer's position.
fn hotspot(shape: u32) -> (i32, i32) {
    match shape {
        graphics::cursor_shape::RESIZE_NS => (4, 8),
        graphics::cursor_shape::RESIZE_EW => (7, 4),
        graphics::cursor_shape::RESIZE_NWSE | graphics::cursor_shape::RESIZE_NESW => (7, 7),
        _ => (0, 0),
    }
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

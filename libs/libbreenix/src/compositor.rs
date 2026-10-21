//! A userspace GPU compositor (`docs/gui/breengel.md` section 18, ruling B13).
//!
//! The process that owns the display and holds the compositor claim composes the windows
//! itself: it reserves a scene snapshot (414 op 29), encodes the scene as a BGL stream and
//! submits it with the snapshot's sample-only source aliases (414 op 19), which the kernel
//! validates, renders and presents. A persistent scene target C holds the whole scene; each
//! frame redraws only the snapshot's damage in C (black, the windows bottom to top, then the
//! cursor sprite), copies C into whichever of the two scanout targets A and B is not shown,
//! and presents it.

use crate::gpu::{self, abi as gpu_abi};
use crate::window::{self, abi};
use abi::{op, Plain};
use gpu_abi::stream::{cmd, encode};
use std::vec::Vec;

const BLACK: u32 = 0xFF00_0000;
const WHITE: u32 = 0xFFFF_FFFF;

/// How long one composite retries a snapshot that went stale, or waits for a busy one.
const RETRY_NS: i128 = 2_000_000_000;

/// The window op `op` on `value`: the kernel's raw return.
fn call<T: Plain>(op: u32, value: &mut T) -> i64 {
    // SAFETY: `value` is a live plain structure of its own size.
    unsafe { window::raw_call(op, value as *mut T as *mut u8, core::mem::size_of::<T>()) }
}

fn now_ns() -> i128 {
    crate::time::now_monotonic().map_or(0, |t| t.as_nanos())
}

/// One completed composite: its serial and the scene pixels it processed.
#[derive(Clone, Copy, Debug)]
pub struct Frame {
    pub serial: u64,
    pub pixels: u64,
}

/// The cursor sprite of `shape` with its pointer at `(x, y)`: outline rectangles, then fill
/// rectangles, each `(x0, y0, x1, y1)`, inside the cursor's footprint
/// (`abi::Cursor::footprint`).
pub fn sprite(shape: u32, x: i32, y: i32) -> (Vec<[i32; 4]>, Vec<[i32; 4]>) {
    let (mut outline, mut fill) = (Vec::new(), Vec::new());
    match shape {
        abi::cursor_shape::ARROW => {
            for i in 0..19 {
                let w = i.min(12) + 1;
                outline.push([x, y + i, x + w, y + i + 1]);
                if (2..18).contains(&i) && w > 2 {
                    fill.push([x + 1, y + i, x + w - 1, y + i + 1]);
                }
            }
        }
        abi::cursor_shape::RESIZE_NS => {
            outline.extend([[x - 2, y - 11, x + 3, y + 12], [x - 5, y - 11, x + 6, y - 7], [x - 5, y + 8, x + 6, y + 12]]);
            fill.extend([[x - 1, y - 10, x + 2, y + 11], [x - 4, y - 10, x + 5, y - 8], [x - 4, y + 9, x + 5, y + 11]]);
        }
        abi::cursor_shape::RESIZE_EW => {
            outline.extend([[x - 11, y - 2, x + 12, y + 3], [x - 11, y - 5, x - 7, y + 6], [x + 8, y - 5, x + 12, y + 6]]);
            fill.extend([[x - 10, y - 1, x + 11, y + 2], [x - 10, y - 4, x - 8, y + 5], [x + 9, y - 4, x + 11, y + 5]]);
        }
        abi::cursor_shape::RESIZE_NWSE | abi::cursor_shape::RESIZE_NESW => {
            let flip = if shape == abi::cursor_shape::RESIZE_NWSE { 1 } else { -1 };
            for k in -8..=8 {
                let (cx, cy) = (x + k, y + flip * k);
                outline.push([cx - 2, cy - 2, cx + 3, cy + 3]);
                fill.push([cx - 1, cy - 1, cx + 2, cy + 2]);
            }
        }
        _ => {}
    }
    (outline, fill)
}

fn intersect(a: [i64; 4], b: [i64; 4]) -> Option<[i64; 4]> {
    let r = [a[0].max(b[0]), a[1].max(b[1]), a[2].min(b[2]), a[3].min(b[3])];
    (r[2] > r[0] && r[3] > r[1]).then_some(r)
}

/// Encode one frame: within each damage rectangle of C, black, each window's committed
/// pixels bottom to top (a window without a commit stays black), and the cursor sprite on
/// top; then the whole of C into `scanout`.
pub fn encode_scene(out: &mut Vec<u32>, scene: u64, scanout: u64, size: (u32, u32), windows: &[abi::SceneWindow],
    damage: &[abi::Rect], cursor: Option<abi::Cursor>) {
    let (width, height) = size;
    if !damage.is_empty() {
        encode::begin(out, scene);
    }
    for d in damage {
        let d = [d.x as i64, d.y as i64, d.x as i64 + d.w as i64, d.y as i64 + d.h as i64];
        out.extend([cmd::SCISSOR, 6, d[0] as u32, d[1] as u32, d[2] as u32, d[3] as u32]);
        encode::clear(out, BLACK);
        for w in windows.iter().filter(|w| w.sample != 0) {
            let r = [w.x as i64, w.y as i64, w.x as i64 + w.width as i64, w.y as i64 + w.height as i64];
            if let Some(i) = intersect(d, r) {
                let source = [(i[0] - r[0]) as u32, (i[1] - r[1]) as u32, (i[2] - i[0]) as u32, (i[3] - i[1]) as u32];
                encode::copy(out, w.sample, source, scene, [i[0] as u32, i[1] as u32]);
            }
        }
        if let Some(c) = cursor {
            let Some(f) = c.footprint() else { continue };
            let f = [f.x as i64, f.y as i64, f.x as i64 + f.w as i64, f.y as i64 + f.h as i64];
            if intersect(d, f).is_some() {
                let (outline, fill) = sprite(c.shape, c.x, c.y);
                encode::draw(out, gpu_abi::pipeline::SOLID, BLACK, &outline);
                encode::draw(out, gpu_abi::pipeline::SOLID, WHITE, &fill);
            }
        }
    }
    encode::begin(out, scanout);
    encode::copy(out, scene, [0, 0, width, height], scanout, [0, 0]);
}

/// The compositor's GPU objects: its context, and the scene target C and the scanout
/// targets A and B at the display's current mode.
pub struct Compositor {
    context: u64,
    scene: u64,
    scanout: [u64; 2],
    /// The scanout target the next frame writes: the one not shown.
    next: usize,
    width: u32,
    height: u32,
}

impl Drop for Compositor {
    fn drop(&mut self) {
        self.destroy_targets();
        gpu::context_destroy(self.context);
    }
}

impl Compositor {
    /// A context and the three targets at the current mode, each cleared to black by a
    /// completed GPU clear.
    pub fn new() -> Result<Compositor, i64> {
        let context = gpu::context_create()?;
        let mut c = Compositor { context, scene: 0, scanout: [0; 2], next: 0, width: 0, height: 0 };
        c.make_targets()?;
        Ok(c)
    }

    pub fn context(&self) -> u64 {
        self.context
    }

    fn destroy_targets(&mut self) {
        for t in [self.scene, self.scanout[0], self.scanout[1]] {
            if t != 0 {
                gpu::resource_destroy(t);
            }
        }
        (self.scene, self.scanout) = (0, [0; 2]);
    }

    fn make_targets(&mut self) -> Result<(), i64> {
        self.destroy_targets();
        let (r, modes) = gpu::modes();
        if r != 0 {
            return Err(r);
        }
        let mode = modes.modes.get(modes.current as usize).copied().ok_or(-22)?;
        (self.width, self.height) = (mode.width, mode.height);
        // The scanout targets first: test point 26 binds the process's lowest other scanout.
        for slot in 0..2 {
            self.scanout[slot] = gpu::scanout_target_create(0, mode.width, mode.height)?;
        }
        self.scene = gpu::target_create(0, mode.width, mode.height)?;
        self.next = 0;
        self.clear(&[self.scanout[0], self.scanout[1], self.scene])
    }

    fn clear(&self, targets: &[u64]) -> Result<(), i64> {
        let mut words = Vec::new();
        for &t in targets {
            encode::begin(&mut words, t);
            encode::clear(&mut words, BLACK);
        }
        let (r, s) = gpu::submit(self.context, &words);
        if r != 0 {
            return Err(r);
        }
        match gpu::wait(self.context, s.fence, 2_000_000_000) {
            (0, _) => Ok(()),
            (r, _) => Err(r),
        }
    }

    /// The window snapshot sizing query: the numbers of windows and damage rectangles, and
    /// whether anything needs composing.
    pub fn pending(&self) -> Result<bool, i64> {
        let mut q = abi::SceneSnapshot::ZERO;
        q.context = self.context;
        match call(op::COMPOSITE_SNAPSHOT, &mut q) {
            0 => Ok(q.damage_count > 0 || q.flags & 1 != 0),
            r => Err(r),
        }
    }

    /// Compose once. A snapshot that goes stale or is busy is retried, within 2 s; a render
    /// that failed leaves the scene target to be made again, since its contents are invalid.
    pub fn compose(&mut self) -> Result<Frame, i64> {
        const EAGAIN: i64 = -(abi::errno::EAGAIN as i64);
        const EBUSY: i64 = -(abi::errno::EBUSY as i64);
        const EINVAL: i64 = -(abi::errno::EINVAL as i64);
        const EIO: i64 = -(abi::errno::EIO as i64);
        let deadline = now_ns() + RETRY_NS;
        loop {
            let mut q = abi::SceneSnapshot::ZERO;
            q.context = self.context;
            let mut r = call(op::COMPOSITE_SNAPSHOT, &mut q);
            if r == 0 && (q.width, q.height) != (self.width, self.height) {
                self.make_targets()?;
                continue;
            }
            let mut windows = std::vec![abi::SceneWindow::ZERO; q.window_count as usize];
            let mut damage = std::vec![abi::Rect::ZERO; q.damage_count as usize];
            let mut s = abi::SceneSnapshot::ZERO;
            if r == 0 {
                s.context = self.context;
                s.windows = windows.as_mut_ptr() as u64;
                s.window_capacity = windows.len() as u32;
                s.damage = damage.as_mut_ptr() as u64;
                s.damage_capacity = damage.len() as u32;
                // A request without room for any window or damage would be a sizing query.
                if s.window_capacity == 0 && s.damage_capacity == 0 {
                    windows.push(abi::SceneWindow::ZERO);
                    s.windows = windows.as_mut_ptr() as u64;
                    s.window_capacity = 1;
                }
                r = call(op::COMPOSITE_SNAPSHOT, &mut s);
            }
            // Stale or busy metadata, or a count that grew since the sizing query.
            if matches!(r, EAGAIN | EBUSY | EINVAL) && now_ns() < deadline {
                let _ = crate::time::sleep_ms(1);
                continue;
            }
            if r != 0 {
                return Err(r);
            }
            let mut words = Vec::new();
            let cursor = s.cursor;
            encode_scene(&mut words, self.scene, self.scanout[self.next], (self.width, self.height),
                &windows[..s.window_count as usize], &damage[..s.damage_count as usize], Some(cursor));
            let mut c = abi::Composite::ZERO;
            c.snapshot = s.snapshot;
            c.target = self.scanout[self.next];
            c.stream = words.as_ptr() as u64;
            c.length = (words.len() * 4) as u64;
            c.scene_target = self.scene;
            match call(op::COMPOSITE, &mut c) {
                0 => {
                    self.next ^= 1;
                    return Ok(Frame { serial: c.serial, pixels: c.pixels });
                }
                EAGAIN if now_ns() < deadline => {
                    let _ = crate::time::sleep_ms(1);
                }
                EIO => {
                    // The failed render left C and the destination without valid contents.
                    let scene = gpu::target_create(0, self.width, self.height)?;
                    gpu::resource_destroy(self.scene);
                    self.scene = scene;
                    self.clear(&[scene])?;
                    return Err(EIO);
                }
                r => return Err(r),
            }
        }
    }
}

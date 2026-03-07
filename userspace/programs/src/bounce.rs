//! Bouncing rectangles demo for Breenix — GPU-accelerated via VirGL DRAW_VBO.
//!
//! Renders colored rectangles at arbitrary positions using the VirGL 3D pipeline.
//! Each rectangle is a DRAW_VBO call with a constant-color fragment shader.
//! Falls back to mmap software rendering if VirGL is unavailable.
//!
//! Created for Gus!

use std::process;

use libbreenix::graphics;
use libbreenix::graphics::{FlushRect, VirglRect};
use libbreenix::time;

use libgfx::color::Color;
use libgfx::font;
use libgfx::framebuf::FrameBuf;
use libgfx::shapes;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn clock_monotonic_ns() -> u64 {
    let ts = time::now_monotonic().unwrap_or_default();
    (ts.tv_sec as u64) * 1_000_000_000 + (ts.tv_nsec as u64)
}

// ---------------------------------------------------------------------------
// Rectangle physics
// ---------------------------------------------------------------------------

struct AnimRect {
    x: i32,       // fixed point ×100
    y: i32,
    vx: i32,
    vy: i32,
    w: i32,       // pixel width
    h: i32,       // pixel height
    color: Color,
}

impl AnimRect {
    fn new(x: i32, y: i32, vx: i32, vy: i32, w: i32, h: i32, color: Color) -> Self {
        Self { x: x * 100, y: y * 100, vx, vy, w, h, color }
    }
    fn px(&self) -> i32 { self.x / 100 }
    fn py(&self) -> i32 { self.y / 100 }

    fn step(&mut self, substeps: i32) {
        self.x += self.vx / substeps;
        self.y += self.vy / substeps;
    }

    fn bounce_walls(&mut self, screen_w: i32, screen_h: i32) {
        let px = self.px();
        let py = self.py();
        if px < 0 { self.x = 0; self.vx = -self.vx; }
        if px + self.w >= screen_w { self.x = (screen_w - self.w - 1) * 100; self.vx = -self.vx; }
        if py < 0 { self.y = 0; self.vy = -self.vy; }
        if py + self.h >= screen_h { self.y = (screen_h - self.h - 1) * 100; self.vy = -self.vy; }
    }

    fn draw(&self, fb: &mut FrameBuf) {
        shapes::fill_rect(fb, self.px(), self.py(), self.w, self.h, self.color);
    }
}

// ---------------------------------------------------------------------------
// FPS counter
// ---------------------------------------------------------------------------

struct FpsCounter {
    last_time_ns: u64,
    frame_count: u32,
    display_fps: u32,
}

impl FpsCounter {
    fn new() -> Self {
        Self { last_time_ns: clock_monotonic_ns(), frame_count: 0, display_fps: 0 }
    }

    fn tick(&mut self) {
        self.frame_count += 1;
        if self.frame_count >= 16 {
            let now = clock_monotonic_ns();
            let elapsed = now.saturating_sub(self.last_time_ns);
            if elapsed > 0 {
                self.display_fps = (self.frame_count as u64 * 1_000_000_000 / elapsed) as u32;
            }
            println!("[bounce] FPS: {} ({}ms/frame)", self.display_fps,
                     elapsed / (self.frame_count as u64 * 1_000_000));
            self.frame_count = 0;
            self.last_time_ns = now;
        }
    }

    fn draw(&self, fb: &mut FrameBuf) {
        let y = fb.height.saturating_sub(20);
        let mut buf = [b' '; 12];
        buf[0] = b'F'; buf[1] = b'P'; buf[2] = b'S'; buf[3] = b':'; buf[4] = b' ';

        let mut fps = self.display_fps;
        if fps == 0 {
            buf[5] = b'0';
        } else {
            let mut pos = 8;
            while fps > 0 && pos >= 5 {
                buf[pos] = b'0' + (fps % 10) as u8;
                fps /= 10;
                if pos == 0 { break; }
                pos -= 1;
            }
        }
        font::draw_text(fb, &buf, 8, y, Color::GRAY, 2);
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

const NUM_RECTS: usize = 6;

fn main() {
    let boot_id = clock_monotonic_ns();
    println!("Bounce demo starting (for Gus!) [boot_id={:016x}]", boot_id);

    let info = match graphics::fbinfo() {
        Ok(info) => info,
        Err(_e) => { println!("Error: Could not get framebuffer info"); process::exit(1); }
    };

    let height = info.height as i32;
    let width = info.left_pane_width() as i32;

    let mut rects = [
        AnimRect::new( 50,  40,  900,  700,  120,  80, Color::rgb(255,  50,  50)),  // Red
        AnimRect::new(200, 150, -800,  600,   90, 110, Color::rgb( 50, 255,  50)),  // Green
        AnimRect::new(100, 300,  750, -850,  140,  60, Color::rgb( 50,  50, 255)),  // Blue
        AnimRect::new(300, 100, -700, -750,   70, 130, Color::rgb(255, 255,  50)),  // Yellow
        AnimRect::new(150, 400,  850,  500,  100, 100, Color::rgb(255,  50, 255)),  // Magenta
        AnimRect::new(350, 250, -650,  800,  110,  70, Color::rgb( 50, 255, 255)),  // Cyan
    ];

    let bg_packed = graphics::rgb(15, 15, 30);

    // Try VirGL GPU rendering first
    let test_rects = build_virgl_rects(&rects);
    let use_virgl = graphics::virgl_submit_rects(&test_rects[..NUM_RECTS], bg_packed).is_ok();

    if use_virgl {
        println!("Starting VirGL GPU-rendered rect demo ({} rects, {}x{}) [boot_id={:016x}]",
                 NUM_RECTS, width, height, boot_id);
        run_virgl_loop(&mut rects, width, height, bg_packed);
    } else {
        println!("VirGL unavailable, falling back to mmap rendering [boot_id={:016x}]", boot_id);
        run_mmap_loop(&mut rects, width, height, &info);
    }
}

fn build_virgl_rects(rects: &[AnimRect; NUM_RECTS]) -> [VirglRect; NUM_RECTS] {
    let mut vr = [VirglRect::default(); NUM_RECTS];
    for (i, rect) in rects.iter().enumerate() {
        let c = rect.color;
        vr[i] = VirglRect {
            x: rect.px() as f32,
            y: rect.py() as f32,
            w: rect.w as f32,
            h: rect.h as f32,
            r: c.r as f32 / 255.0,
            g: c.g as f32 / 255.0,
            b: c.b as f32 / 255.0,
            a: 1.0,
        };
    }
    vr
}

/// VirGL GPU rendering loop — all rendering on host GPU.
fn run_virgl_loop(rects: &mut [AnimRect; NUM_RECTS], width: i32, height: i32, bg_packed: u32) {
    const SUBSTEPS: i32 = 8;
    let mut fps = FpsCounter::new();

    loop {
        for _ in 0..SUBSTEPS {
            for rect in rects.iter_mut() { rect.step(SUBSTEPS); }
            for rect in rects.iter_mut() { rect.bounce_walls(width, height); }
        }

        let vr = build_virgl_rects(rects);
        let _ = graphics::virgl_submit_rects(&vr[..NUM_RECTS], bg_packed);

        fps.tick();
    }
}

/// Mmap software rendering loop — fallback when VirGL is unavailable.
fn run_mmap_loop(rects: &mut [AnimRect; NUM_RECTS], width: i32, height: i32, info: &graphics::FbInfo) {
    let bpp = info.bytes_per_pixel as usize;
    let bg = Color::rgb(15, 15, 30);

    let fb_ptr = match graphics::fb_mmap() {
        Ok(ptr) => ptr,
        Err(e) => { println!("Error: Could not mmap framebuffer ({})", e); process::exit(1); }
    };

    let mut fb = unsafe {
        FrameBuf::from_raw(
            fb_ptr,
            width as usize,
            height as usize,
            (width as usize) * bpp,
            bpp,
            info.is_bgr(),
        )
    };

    println!("Starting collision demo ({} rects, {}x{}, mmap, batch flush)", NUM_RECTS, width, height);

    const SUBSTEPS: i32 = 8;
    let mut fps = FpsCounter::new();
    let mut prev: [(i32, i32, i32, i32); NUM_RECTS] = [(0, 0, 0, 0); NUM_RECTS];
    let mut first_frame = true;
    const PAD: i32 = 2;

    loop {
        for _ in 0..SUBSTEPS {
            for rect in rects.iter_mut() { rect.step(SUBSTEPS); }
            for rect in rects.iter_mut() { rect.bounce_walls(width, height); }
        }

        if first_frame {
            fb.clear(bg);
            for rect in rects.iter() { rect.draw(&mut fb); }
            for (i, rect) in rects.iter().enumerate() {
                prev[i] = (rect.px(), rect.py(), rect.w, rect.h);
            }
            fps.tick();
            fps.draw(&mut fb);
            if let Some(dirty) = fb.take_dirty() {
                let _ = graphics::fb_flush_rect(dirty.x, dirty.y, dirty.w, dirty.h);
            }
            first_frame = false;
        } else {
            // Erase previous positions
            for &(px, py, w, h) in prev.iter() {
                shapes::fill_rect(&mut fb,
                    (px - PAD).max(0), (py - PAD).max(0),
                    w + PAD * 2, h + PAD * 2, bg);
            }
            let fps_y = (height - 40).max(0);
            shapes::fill_rect(&mut fb, 0, fps_y, 340, 40, bg);

            // Draw new positions
            for rect in rects.iter() { rect.draw(&mut fb); }
            fps.tick();
            fps.draw(&mut fb);

            let _ = fb.take_dirty();

            // Batch flush
            let mut flush_rects = [FlushRect { x: 0, y: 0, w: 0, h: 0 }; NUM_RECTS + 1];
            let mut rect_count = 0usize;
            for (i, rect) in rects.iter().enumerate() {
                let (opx, opy, ow, oh) = prev[i];
                let npx = rect.px();
                let npy = rect.py();
                let nw = rect.w;
                let nh = rect.h;
                let x1 = (opx - PAD).min(npx - PAD).max(0);
                let y1 = (opy - PAD).min(npy - PAD).max(0);
                let x2 = (opx + ow + PAD).max(npx + nw + PAD).min(width);
                let y2 = (opy + oh + PAD).max(npy + nh + PAD).min(height);
                if x2 > x1 && y2 > y1 {
                    flush_rects[rect_count] = FlushRect { x: x1, y: y1, w: x2 - x1, h: y2 - y1 };
                    rect_count += 1;
                }
            }
            flush_rects[rect_count] = FlushRect { x: 0, y: fps_y, w: 340, h: 40 };
            rect_count += 1;
            let _ = graphics::fb_flush_rects(&flush_rects[..rect_count]);

            for (i, rect) in rects.iter().enumerate() {
                prev[i] = (rect.px(), rect.py(), rect.w, rect.h);
            }
        }

        let _ = time::sleep_ms(1);
    }
}

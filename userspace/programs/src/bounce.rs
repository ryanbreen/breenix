//! Bouncing balls with collision detection demo for Breenix (std version)
//!
//! Two rendering paths:
//! - **VirGL GPU** (preferred): All rendering on host GPU via VirGL 3D pipeline.
//!   Guest sends ~1KB of draw commands, host renders, DMA copies to BAR0.
//!   Expected: 60+ FPS.
//! - **mmap fallback**: Software rendering to mmap'd framebuffer with per-ball flush.
//!   Guest CPU writes ~340KB to BAR0 per frame. Achieves ~12 FPS on Parallels.
//!
//! Created for Gus!

use std::process;

use libbreenix::graphics;
use libbreenix::graphics::{FlushRect, VirglBall};
use libbreenix::time;

use libgfx::color::Color;
use libgfx::font;
use libgfx::framebuf::FrameBuf;
use libgfx::math::isqrt_i64;
use libgfx::shapes;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn clock_monotonic_ns() -> u64 {
    let ts = time::now_monotonic().unwrap_or_default();
    (ts.tv_sec as u64) * 1_000_000_000 + (ts.tv_nsec as u64)
}

// ---------------------------------------------------------------------------
// Ball physics
// ---------------------------------------------------------------------------

struct Ball {
    x: i32,       // fixed point ×100
    y: i32,
    vx: i32,
    vy: i32,
    radius: i32,
    color: Color,
    mass: i32,
}

impl Ball {
    fn new(x: i32, y: i32, vx: i32, vy: i32, radius: i32, color: Color) -> Self {
        Self { x: x * 100, y: y * 100, vx, vy, radius, color, mass: radius }
    }
    fn px(&self) -> i32 { self.x / 100 }
    fn py(&self) -> i32 { self.y / 100 }

    fn step(&mut self, substeps: i32) {
        self.x += self.vx / substeps;
        self.y += self.vy / substeps;
    }

    fn bounce_walls(&mut self, w: i32, h: i32) {
        let px = self.px();
        let py = self.py();
        if px - self.radius < 0 { self.x = self.radius * 100; self.vx = -self.vx; }
        if px + self.radius >= w { self.x = (w - self.radius - 1) * 100; self.vx = -self.vx; }
        if py - self.radius < 0 { self.y = self.radius * 100; self.vy = -self.vy; }
        if py + self.radius >= h { self.y = (h - self.radius - 1) * 100; self.vy = -self.vy; }
    }

    fn draw(&self, fb: &mut FrameBuf) {
        shapes::fill_circle(fb, self.px(), self.py(), self.radius, self.color);
    }
}

/// Elastic collision: decompose into normal/tangential, swap normal by mass.
fn check_collision(b1: &mut Ball, b2: &mut Ball) {
    let dx = (b2.x - b1.x) as i64;
    let dy = (b2.y - b1.y) as i64;
    let touch = ((b1.radius + b2.radius) * 100) as i64;
    let dist_sq = dx * dx + dy * dy;
    if dist_sq >= touch * touch || dist_sq == 0 { return; }

    let dist = isqrt_i64(dist_sq);
    if dist == 0 { b1.x -= 100; b2.x += 100; return; }

    let nx = dx * 1024 / dist;
    let ny = dy * 1024 / dist;

    let v1n = (b1.vx as i64 * nx + b1.vy as i64 * ny) / 1024;
    let v2n = (b2.vx as i64 * nx + b2.vy as i64 * ny) / 1024;
    if v1n <= v2n { return; }

    let m1 = b1.mass as i64;
    let m2 = b2.mass as i64;
    let mt = m1 + m2;

    let v1n_new = ((m1 - m2) * v1n + 2 * m2 * v2n) / mt;
    let v2n_new = ((m2 - m1) * v2n + 2 * m1 * v1n) / mt;

    let dv1 = v1n_new - v1n;
    let dv2 = v2n_new - v2n;
    b1.vx += (dv1 * nx / 1024) as i32;
    b1.vy += (dv1 * ny / 1024) as i32;
    b2.vx += (dv2 * nx / 1024) as i32;
    b2.vy += (dv2 * ny / 1024) as i32;

    let overlap = touch - dist + 50;
    let push1 = overlap * m2 / mt;
    let push2 = overlap * m1 / mt;
    b1.x -= (push1 * nx / 1024) as i32;
    b1.y -= (push1 * ny / 1024) as i32;
    b2.x += (push2 * nx / 1024) as i32;
    b2.y += (push2 * ny / 1024) as i32;
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

    /// Call once per frame. Updates the displayed FPS every 16 frames.
    fn tick(&mut self) {
        self.frame_count += 1;
        if self.frame_count >= 16 {
            let now = clock_monotonic_ns();
            let elapsed = now.saturating_sub(self.last_time_ns);
            if elapsed > 0 {
                self.display_fps = (self.frame_count as u64 * 1_000_000_000 / elapsed) as u32;
            }
            // Log FPS to serial so we can verify from the log
            println!("[bounce] FPS: {} ({}ms/frame)", self.display_fps,
                     elapsed / (self.frame_count as u64 * 1_000_000));
            self.frame_count = 0;
            self.last_time_ns = now;
        }
    }

    /// Render "FPS: NNN" at the bottom-left of the framebuffer
    fn draw(&self, fb: &mut FrameBuf) {
        let y = fb.height.saturating_sub(20);
        let mut buf = [b' '; 12]; // "FPS: NNNN  "
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

fn main() {
    // Unique boot ID from monotonic clock — different every boot, proves we're
    // running the latest binary (check this value in serial logs).
    let boot_id = clock_monotonic_ns();
    println!("Bounce demo starting (for Gus!) [boot_id={:016x}]", boot_id);

    let info = match graphics::fbinfo() {
        Ok(info) => info,
        Err(_e) => { println!("Error: Could not get framebuffer info"); process::exit(1); }
    };

    let height = info.height as i32;

    // VirGL uses full viewport (GPU renders everything), mmap uses left pane only
    let virgl_width = info.width as i32;
    let mmap_width = info.left_pane_width() as i32;

    // 12 balls, fast velocities. Sub-stepping catches edge collisions.
    let mut balls = [
        Ball::new(100, 100,  1100,  800, 38, Color::rgb(255,  50,  50)),  // Red
        Ball::new(300, 200, -1000,  700, 33, Color::rgb( 50, 255,  50)),  // Green
        Ball::new(200, 400,   900, -950, 42, Color::rgb( 50,  50, 255)),  // Blue
        Ball::new(400, 300,  -850, -800, 28, Color::rgb(255, 255,  50)),  // Yellow
        Ball::new(150, 300,  1050,  600, 24, Color::rgb(255,  50, 255)),  // Magenta
        Ball::new(350, 150,  -900,  750, 26, Color::rgb( 50, 255, 255)),  // Cyan
        Ball::new(450, 500,   800, -700, 35, Color::rgb(255, 150,  50)),  // Orange
        Ball::new(250, 550,  -750,  850, 30, Color::rgb(150,  50, 255)),  // Purple
        Ball::new(500, 100,   950,  950, 22, Color::rgb(200, 200, 200)),  // White
        Ball::new(120, 500, -1100, -650, 20, Color::rgb(255, 100, 100)),  // Salmon
        Ball::new(380, 450,   700,  900, 32, Color::rgb(100, 255, 100)),  // Lime
        Ball::new(520, 350,  -800, -850, 27, Color::rgb(100, 150, 255)),  // Sky
    ];

    let bg = Color::rgb(15, 15, 30);
    let bg_packed = graphics::rgb(15, 15, 30);

    // Try VirGL GPU rendering first. If the first frame succeeds, use GPU path.
    let virgl_balls = build_virgl_balls(&balls);
    let use_virgl = graphics::virgl_submit_frame(&virgl_balls[..balls.len()], bg_packed).is_ok();

    if use_virgl {
        println!("Starting VirGL GPU-rendered demo (12 balls, {}x{}) [boot_id={:016x}]",
                 virgl_width, height, boot_id);
        run_virgl_loop(&mut balls, virgl_width, height, bg_packed);
    } else {
        println!("VirGL unavailable, falling back to mmap rendering [boot_id={:016x}]", boot_id);
        run_mmap_loop(&mut balls, mmap_width, height, &info, bg);
    }
}

/// Convert Ball array to VirglBall descriptors for GPU rendering.
fn build_virgl_balls(balls: &[Ball]) -> [VirglBall; 12] {
    let mut vb = [VirglBall::default(); 12];
    for (i, ball) in balls.iter().enumerate().take(12) {
        let c = ball.color;
        vb[i] = VirglBall {
            x: ball.px() as f32,
            y: ball.py() as f32,
            radius: ball.radius as f32,
            color: [
                c.r as f32 / 255.0,
                c.g as f32 / 255.0,
                c.b as f32 / 255.0,
                1.0,
            ],
        };
    }
    vb
}

/// VirGL GPU rendering loop — all rendering on host GPU, zero guest pixel writes.
fn run_virgl_loop(balls: &mut [Ball; 12], width: i32, height: i32, bg_packed: u32) {
    const SUBSTEPS: i32 = 16;
    let mut fps = FpsCounter::new();

    loop {
        // Sub-step physics
        for _ in 0..SUBSTEPS {
            for ball in balls.iter_mut() { ball.step(SUBSTEPS); }
            for ball in balls.iter_mut() { ball.bounce_walls(width, height); }
            for i in 0..balls.len() {
                for j in (i + 1)..balls.len() {
                    let (left, right) = balls.split_at_mut(j);
                    check_collision(&mut left[i], &mut right[0]);
                }
            }
        }

        // Build VirGL ball descriptors from current positions
        let vb = build_virgl_balls(balls);

        // Submit to GPU — one syscall renders everything
        let _ = graphics::virgl_submit_frame(&vb[..balls.len()], bg_packed);

        fps.tick();
    }
}

/// Mmap software rendering loop — fallback when VirGL is unavailable.
fn run_mmap_loop(balls: &mut [Ball; 12], width: i32, height: i32, info: &graphics::FbInfo, bg: Color) {
    let bpp = info.bytes_per_pixel as usize;

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

    println!("Starting collision demo (12 balls, {}x{}, mmap, batch flush)", width, height);

    // With velocities ~1000 (10 px/frame), 16 sub-steps = ~0.6 px per step.
    const SUBSTEPS: i32 = 16;

    let mut fps = FpsCounter::new();

    // Track previous frame ball positions for per-ball flushing.
    let mut prev: [(i32, i32, i32); 12] = [(0, 0, 0); 12];
    let mut first_frame = true;
    const PAD: i32 = 2;

    loop {
        // Sub-step physics
        for _ in 0..SUBSTEPS {
            for ball in balls.iter_mut() {
                ball.step(SUBSTEPS);
            }
            for ball in balls.iter_mut() {
                ball.bounce_walls(width, height);
            }
            for i in 0..balls.len() {
                for j in (i + 1)..balls.len() {
                    let (left, right) = balls.split_at_mut(j);
                    check_collision(&mut left[i], &mut right[0]);
                }
            }
        }

        if first_frame {
            // First frame: full clear + single flush
            fb.clear(bg);
            for ball in balls.iter() { ball.draw(&mut fb); }
            for (i, ball) in balls.iter().enumerate() {
                prev[i] = (ball.px(), ball.py(), ball.radius);
            }
            fps.tick();
            fps.draw(&mut fb);
            if let Some(dirty) = fb.take_dirty() {
                let _ = graphics::fb_flush_rect(dirty.x, dirty.y, dirty.w, dirty.h);
            }
            first_frame = false;
        } else {
            // Phase 1: Erase all previous ball positions
            for &(px, py, r) in prev.iter() {
                if r > 0 {
                    shapes::fill_rect(&mut fb,
                        (px - r - PAD).max(0), (py - r - PAD).max(0),
                        (r + PAD) * 2 + 1, (r + PAD) * 2 + 1, bg);
                }
            }
            let fps_y = (height - 40).max(0);
            shapes::fill_rect(&mut fb, 0, fps_y, 340, 40, bg);

            // Phase 2: Draw all new ball positions
            for ball in balls.iter() { ball.draw(&mut fb); }
            fps.tick();
            fps.draw(&mut fb);

            // Discard accumulated dirty rect — we use batch flush below
            let _ = fb.take_dirty();

            // Phase 3: Batch flush — all dirty rects in ONE syscall, ONE DSB barrier.
            // Saves 12 syscall round-trips + 12 DSB stalls vs per-ball flushing.
            let mut flush_rects = [FlushRect { x: 0, y: 0, w: 0, h: 0 }; 13];
            let mut rect_count = 0usize;
            for (i, ball) in balls.iter().enumerate() {
                let (opx, opy, or) = prev[i];
                let npx = ball.px();
                let npy = ball.py();
                let nr = ball.radius;
                let x1 = (opx - or - PAD).min(npx - nr - PAD).max(0);
                let y1 = (opy - or - PAD).min(npy - nr - PAD).max(0);
                let x2 = (opx + or + PAD + 1).max(npx + nr + PAD + 1).min(width);
                let y2 = (opy + or + PAD + 1).max(npy + nr + PAD + 1).min(height);
                if x2 > x1 && y2 > y1 {
                    flush_rects[rect_count] = FlushRect { x: x1, y: y1, w: x2 - x1, h: y2 - y1 };
                    rect_count += 1;
                }
            }
            // FPS region
            flush_rects[rect_count] = FlushRect { x: 0, y: fps_y, w: 340, h: 40 };
            rect_count += 1;
            let _ = graphics::fb_flush_rects(&flush_rects[..rect_count]);

            // Save new positions for next frame's erase
            for (i, ball) in balls.iter().enumerate() {
                prev[i] = (ball.px(), ball.py(), ball.radius);
            }
        }

        let _ = time::sleep_ms(1); // Yield CPU briefly without wasting frame time
    }
}

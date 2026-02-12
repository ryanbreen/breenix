//! Bouncing balls with collision detection demo for Breenix (std version)
//!
//! Uses mmap'd framebuffer for zero-syscall drawing via libgfx. All pixel
//! writes go directly to a userspace buffer; only flush (1 syscall/frame)
//! copies the dirty region to VRAM.
//!
//! Created for Gus!

use std::process;

use libgfx::color::Color;
use libgfx::font;
use libgfx::framebuf::FrameBuf;
use libgfx::math::isqrt_i64;
use libgfx::shapes;

// ---------------------------------------------------------------------------
// Syscall plumbing (minimal — only what libgfx doesn't handle)
// ---------------------------------------------------------------------------

#[repr(C)]
struct FbInfo {
    width: u64,
    height: u64,
    stride: u64,
    bytes_per_pixel: u64,
    pixel_format: u64,
}

impl FbInfo {
    fn zeroed() -> Self {
        Self { width: 0, height: 0, stride: 0, bytes_per_pixel: 0, pixel_format: 0 }
    }
    fn left_pane_width(&self) -> u64 { self.width / 2 }
    fn is_bgr(&self) -> bool { self.pixel_format == 1 }
}

#[repr(C)]
struct FbDrawCmd { op: u32, p1: i32, p2: i32, p3: i32, p4: i32, color: u32 }

#[repr(C)]
struct Timespec { tv_sec: i64, tv_nsec: i64 }

const SYS_FBINFO: u64 = 410;
const SYS_FBDRAW: u64 = 411;
const SYS_FBMMAP: u64 = 412;
const SYS_CLOCK_GETTIME: u64 = 228;
const CLOCK_MONOTONIC: u64 = 1;

#[cfg(target_arch = "x86_64")]
unsafe fn syscall0(num: u64) -> u64 {
    let ret: u64;
    core::arch::asm!("int 0x80", in("rax") num, lateout("rax") ret,
        options(nostack, preserves_flags));
    ret
}
#[cfg(target_arch = "aarch64")]
unsafe fn syscall0(num: u64) -> u64 {
    let ret: u64;
    core::arch::asm!("svc #0", in("x8") num, lateout("x0") ret, options(nostack));
    ret
}

#[cfg(target_arch = "x86_64")]
unsafe fn syscall1(num: u64, a1: u64) -> u64 {
    let ret: u64;
    core::arch::asm!("int 0x80", in("rax") num, in("rdi") a1, lateout("rax") ret,
        options(nostack, preserves_flags));
    ret
}
#[cfg(target_arch = "aarch64")]
unsafe fn syscall1(num: u64, a1: u64) -> u64 {
    let ret: u64;
    core::arch::asm!("svc #0", in("x8") num, inlateout("x0") a1 => ret, options(nostack));
    ret
}

#[cfg(target_arch = "x86_64")]
unsafe fn syscall2(num: u64, a1: u64, a2: u64) -> u64 {
    let ret: u64;
    core::arch::asm!("int 0x80", in("rax") num, in("rdi") a1, in("rsi") a2,
        lateout("rax") ret, options(nostack, preserves_flags));
    ret
}
#[cfg(target_arch = "aarch64")]
unsafe fn syscall2(num: u64, a1: u64, a2: u64) -> u64 {
    let ret: u64;
    core::arch::asm!("svc #0", in("x8") num, inlateout("x0") a1 => ret, in("x1") a2,
        options(nostack));
    ret
}

fn fbinfo() -> Result<FbInfo, i32> {
    let mut info = FbInfo::zeroed();
    let r = unsafe { syscall1(SYS_FBINFO, &mut info as *mut FbInfo as u64) };
    if (r as i64) < 0 { Err(-(r as i64) as i32) } else { Ok(info) }
}
fn fbmmap() -> Result<*mut u8, i32> {
    let r = unsafe { syscall0(SYS_FBMMAP) };
    if (r as i64) < 0 { Err(-(r as i64) as i32) } else { Ok(r as *mut u8) }
}
fn fb_flush_rect(x: i32, y: i32, w: i32, h: i32) -> Result<(), i32> {
    let cmd = FbDrawCmd { op: 6, p1: x, p2: y, p3: w, p4: h, color: 0 };
    let r = unsafe { syscall1(SYS_FBDRAW, &cmd as *const FbDrawCmd as u64) };
    if (r as i64) < 0 { Err(-(r as i64) as i32) } else { Ok(()) }
}
fn fb_flush_full() -> Result<(), i32> {
    let cmd = FbDrawCmd { op: 6, p1: 0, p2: 0, p3: 0, p4: 0, color: 0 };
    let r = unsafe { syscall1(SYS_FBDRAW, &cmd as *const FbDrawCmd as u64) };
    if (r as i64) < 0 { Err(-(r as i64) as i32) } else { Ok(()) }
}
fn clock_monotonic_ns() -> u64 {
    let mut ts = Timespec { tv_sec: 0, tv_nsec: 0 };
    unsafe { syscall2(SYS_CLOCK_GETTIME, CLOCK_MONOTONIC, &mut ts as *mut Timespec as u64) };
    (ts.tv_sec as u64) * 1_000_000_000 + (ts.tv_nsec as u64)
}

extern "C" { fn sleep_ms(ms: u64); }

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
    println!("Bounce demo starting (for Gus!)");

    let info = match fbinfo() {
        Ok(info) => info,
        Err(e) => { println!("Error: Could not get framebuffer info"); process::exit(e); }
    };

    let width = info.left_pane_width() as i32;
    let height = info.height as i32;
    let bpp = info.bytes_per_pixel as usize;

    let fb_ptr = match fbmmap() {
        Ok(ptr) => ptr,
        Err(e) => { println!("Error: Could not mmap framebuffer ({})", e); process::exit(e); }
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

    println!("Starting collision demo (12 balls, mmap mode)...");

    let bg = Color::rgb(15, 15, 30);

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

    // With velocities ~1000 (10 px/frame), 16 sub-steps = ~0.6 px per step.
    const SUBSTEPS: i32 = 16;

    let mut fps = FpsCounter::new();

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

        // Draw — libgfx tracks dirty rects automatically
        fb.clear(bg);
        for ball in balls.iter() {
            ball.draw(&mut fb);
        }
        fps.tick();
        fps.draw(&mut fb);

        // Flush only the dirty region
        if let Some(dirty) = fb.take_dirty() {
            let _ = fb_flush_rect(dirty.x, dirty.y, dirty.w, dirty.h);
        } else {
            let _ = fb_flush_full();
        }

        unsafe { sleep_ms(16); } // ~60 FPS target
    }
}

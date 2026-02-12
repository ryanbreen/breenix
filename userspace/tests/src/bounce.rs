//! Bouncing balls with collision detection demo for Breenix (std version)
//!
//! Uses mmap'd framebuffer for zero-syscall drawing. All pixel writes go
//! directly to a userspace buffer; only flush (1 syscall/frame) copies to VRAM.
//!
//! Created for Gus!

use std::process;

// ---------------------------------------------------------------------------
// Syscall plumbing
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
fn fb_flush() -> Result<(), i32> {
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
// Framebuffer
// ---------------------------------------------------------------------------

struct FrameBuf {
    ptr: *mut u8,
    width: usize,
    height: usize,
    stride: usize,
    bpp: usize,
    is_bgr: bool,
}

impl FrameBuf {
    fn clear(&mut self, r: u8, g: u8, b: u8) {
        let buf = unsafe { core::slice::from_raw_parts_mut(self.ptr, self.stride * self.height) };
        let (c0, c1, c2) = if self.is_bgr { (b, g, r) } else { (r, g, b) };
        if self.bpp == 4 {
            for x in 0..self.width {
                let o = x * 4;
                buf[o] = c0; buf[o+1] = c1; buf[o+2] = c2; buf[o+3] = 0;
            }
        } else {
            for x in 0..self.width {
                let o = x * 3;
                buf[o] = c0; buf[o+1] = c1; buf[o+2] = c2;
            }
        }
        for y in 1..self.height {
            unsafe {
                core::ptr::copy_nonoverlapping(buf.as_ptr(), buf.as_mut_ptr().add(y * self.stride), self.stride);
            }
        }
    }

    #[inline]
    fn put_pixel(&mut self, x: usize, y: usize, r: u8, g: u8, b: u8) {
        let off = y * self.stride + x * self.bpp;
        let (c0, c1, c2) = if self.is_bgr { (b, g, r) } else { (r, g, b) };
        unsafe {
            *self.ptr.add(off) = c0;
            *self.ptr.add(off + 1) = c1;
            *self.ptr.add(off + 2) = c2;
            if self.bpp == 4 { *self.ptr.add(off + 3) = 0; }
        }
    }

    fn fill_circle(&mut self, cx: i32, cy: i32, radius: i32, r: u8, g: u8, b: u8) {
        let r2 = (radius as i64) * (radius as i64);
        let (c0, c1, c2) = if self.is_bgr { (b, g, r) } else { (r, g, b) };
        for dy in -radius..=radius {
            let dx_max_sq = r2 - (dy as i64) * (dy as i64);
            if dx_max_sq < 0 { continue; }
            let dx_max = isqrt_i64(dx_max_sq) as i32;
            let y = cy + dy;
            if y < 0 || y >= self.height as i32 { continue; }
            let x_start = (cx - dx_max).max(0) as usize;
            let x_end = (cx + dx_max).min(self.width as i32 - 1) as usize;
            if x_start > x_end { continue; }
            let row = (y as usize) * self.stride;
            if self.bpp == 4 {
                for x in x_start..=x_end {
                    let o = row + x * 4;
                    unsafe {
                        *self.ptr.add(o) = c0; *self.ptr.add(o+1) = c1;
                        *self.ptr.add(o+2) = c2; *self.ptr.add(o+3) = 0;
                    }
                }
            } else {
                for x in x_start..=x_end {
                    let o = row + x * 3;
                    unsafe {
                        *self.ptr.add(o) = c0; *self.ptr.add(o+1) = c1;
                        *self.ptr.add(o+2) = c2;
                    }
                }
            }
        }
    }

    /// Draw a character from a 5x7 bitmap font, scaled 2x (10x14 pixels)
    fn draw_char(&mut self, ch: u8, x0: usize, y0: usize, r: u8, g: u8, b: u8) {
        let glyph = match ch {
            b'0' => [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E],
            b'1' => [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E],
            b'2' => [0x0E, 0x11, 0x01, 0x06, 0x08, 0x10, 0x1F],
            b'3' => [0x0E, 0x11, 0x01, 0x06, 0x01, 0x11, 0x0E],
            b'4' => [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02],
            b'5' => [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E],
            b'6' => [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E],
            b'7' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
            b'8' => [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E],
            b'9' => [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C],
            b'F' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x10],
            b'P' => [0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10, 0x10],
            b'S' => [0x0E, 0x11, 0x10, 0x0E, 0x01, 0x11, 0x0E],
            b':' => [0x00, 0x04, 0x04, 0x00, 0x04, 0x04, 0x00],
            b' ' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            _    => [0x1F, 0x1F, 0x1F, 0x1F, 0x1F, 0x1F, 0x1F], // block
        };
        for row in 0..7usize {
            let bits = glyph[row];
            for col in 0..5usize {
                if bits & (0x10 >> col) != 0 {
                    // 2x scale
                    let px = x0 + col * 2;
                    let py = y0 + row * 2;
                    for sy in 0..2usize {
                        for sx in 0..2usize {
                            let xx = px + sx;
                            let yy = py + sy;
                            if xx < self.width && yy < self.height {
                                self.put_pixel(xx, yy, r, g, b);
                            }
                        }
                    }
                }
            }
        }
    }

    /// Draw a string of ASCII characters
    fn draw_text(&mut self, text: &[u8], x: usize, y: usize, r: u8, g: u8, b: u8) {
        for (i, &ch) in text.iter().enumerate() {
            self.draw_char(ch, x + i * 12, y, r, g, b);
        }
    }
}

fn isqrt_i64(n: i64) -> i64 {
    if n < 0 { return 0; }
    if n < 2 { return n; }
    let mut x = n;
    let mut y = (x + 1) / 2;
    while y < x { x = y; y = (x + n / x) / 2; }
    x
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
    r: u8, g: u8, b: u8,
    mass: i32,
}

impl Ball {
    fn new(x: i32, y: i32, vx: i32, vy: i32, radius: i32, r: u8, g: u8, b: u8) -> Self {
        Self { x: x * 100, y: y * 100, vx, vy, radius, r, g, b, mass: radius }
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
        fb.fill_circle(self.px(), self.py(), self.radius, self.r, self.g, self.b);
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
                // fps = frame_count * 1_000_000_000 / elapsed
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
            // Write digits right-to-left
            let mut pos = 8;
            while fps > 0 && pos >= 5 {
                buf[pos] = b'0' + (fps % 10) as u8;
                fps /= 10;
                if pos == 0 { break; }
                pos -= 1;
            }
        }
        fb.draw_text(&buf, 8, y, 200, 200, 200);
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

    let mut fb = FrameBuf {
        ptr: fb_ptr,
        width: width as usize,
        height: height as usize,
        stride: (width as usize) * bpp,
        bpp,
        is_bgr: info.is_bgr(),
    };

    println!("Starting collision demo (12 balls, mmap mode)...");

    // 12 balls, fast velocities. Sub-stepping catches edge collisions.
    let mut balls = [
        Ball::new(100, 100,  1100,  800, 38, 255,  50,  50),  // Red
        Ball::new(300, 200, -1000,  700, 33,  50, 255,  50),  // Green
        Ball::new(200, 400,   900, -950, 42,  50,  50, 255),  // Blue
        Ball::new(400, 300,  -850, -800, 28, 255, 255,  50),  // Yellow
        Ball::new(150, 300,  1050,  600, 24, 255,  50, 255),  // Magenta
        Ball::new(350, 150,  -900,  750, 26,  50, 255, 255),  // Cyan
        Ball::new(450, 500,   800, -700, 35, 255, 150,  50),  // Orange
        Ball::new(250, 550,  -750,  850, 30, 150,  50, 255),  // Purple
        Ball::new(500, 100,   950,  950, 22, 200, 200, 200),  // White
        Ball::new(120, 500, -1100, -650, 20, 255, 100, 100),  // Salmon
        Ball::new(380, 450,   700,  900, 32, 100, 255, 100),  // Lime
        Ball::new(520, 350,  -800, -850, 27, 100, 150, 255),  // Sky
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

        // Draw
        fb.clear(15, 15, 30);
        for ball in balls.iter() {
            ball.draw(&mut fb);
        }
        fps.tick();
        fps.draw(&mut fb);

        let _ = fb_flush();
        unsafe { sleep_ms(16); } // ~60 FPS target
    }
}

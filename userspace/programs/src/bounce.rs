//! Bouncing spheres demo for Breenix.
//!
//! Renders 3D-looking spheres with specular highlights that bounce off screen
//! edges and collide with each other using elastic collision physics.
//!
//! Created for Gus!

use std::process;

use breengel::{CachedFont, Window, Event};
use libbreenix::graphics;
use libbreenix::time;

use libgfx::color::Color;
use libgfx::font;
use libgfx::framebuf::FrameBuf;
use libgfx::ttf_font;
use libgfx::math::isqrt_i64;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn clock_monotonic_ns() -> u64 {
    let ts = time::now_monotonic().unwrap_or_default();
    (ts.tv_sec as u64) * 1_000_000_000 + (ts.tv_nsec as u64)
}

// ---------------------------------------------------------------------------
// Sphere rendering
// ---------------------------------------------------------------------------

/// Draw a 3D-looking sphere with specular highlight and shading.
///
/// The light source is at upper-left. Each pixel's brightness is computed from
/// the dot product of the surface normal with the light direction, plus a
/// specular highlight for the glossy spot.
fn draw_sphere(fb: &mut FrameBuf, cx: i32, cy: i32, radius: i32, base_color: Color) {
    let r2 = (radius as i64) * (radius as i64);
    let w = fb.width as i32;
    let h = fb.height as i32;

    // Light direction (upper-left, normalized ×1024 for fixed-point)
    // (-0.5, -0.5, 0.707) normalized ≈ (-512, -512, 724)
    const LIGHT_X: i64 = -512;
    const LIGHT_Y: i64 = -512;
    const LIGHT_Z: i64 = 724;

    let ptr = fb.raw_ptr();
    let stride = fb.stride;
    let bpp = fb.bpp;
    let is_bgr = fb.is_bgr;

    for dy in -radius..=radius {
        let y = cy + dy;
        if y < 0 || y >= h { continue; }

        let dy64 = dy as i64;
        let row_r2 = r2 - dy64 * dy64;
        if row_r2 < 0 { continue; }
        let dx_max = isqrt_i64(row_r2) as i32;

        let x_start = (cx - dx_max).max(0);
        let x_end = (cx + dx_max).min(w - 1);
        if x_start > x_end { continue; }

        let row_off = (y as usize) * stride;

        for x in x_start..=x_end {
            let dx = (x - cx) as i64;

            // Surface normal z component (sphere equation: nx=dx, ny=dy, nz=sqrt(r²-dx²-dy²))
            let nz_sq = r2 - dx * dx - dy64 * dy64;
            if nz_sq < 0 { continue; }
            let nz = isqrt_i64(nz_sq);

            // Diffuse: dot(normal, light) / (|normal| * 1024)
            // |normal| = radius (since nx²+ny²+nz²=r²)
            let dot = dx * LIGHT_X + dy64 * LIGHT_Y + nz * LIGHT_Z;
            let diffuse = (dot * 1024 / (radius as i64 * 1024)).max(0).min(1024);

            // Specular: reflected light dot view direction (0,0,1), raised to power
            // reflect = 2 * dot(n,l) * n - l, we want reflect.z
            // Simplified: specular based on how close the reflection is to viewer
            let reflect_z = (2 * dot * nz / (r2) - LIGHT_Z).max(0);
            // Approximate pow(reflect_z/1024, 8) for shininess
            let spec_norm = reflect_z * 1024 / 1024; // already in 0..1024 range
            let spec2 = spec_norm * spec_norm / 1024;
            let spec4 = spec2 * spec2 / 1024;
            let spec8 = spec4 * spec4 / 1024;

            // Ambient + diffuse + specular
            let ambient: i64 = 80; // out of 1024
            let intensity = (ambient + diffuse * 700 / 1024).min(1024);

            let cr = ((base_color.r as i64 * intensity / 1024) + spec8 * 255 / 1024).min(255) as u8;
            let cg = ((base_color.g as i64 * intensity / 1024) + spec8 * 255 / 1024).min(255) as u8;
            let cb = ((base_color.b as i64 * intensity / 1024) + spec8 * 255 / 1024).min(255) as u8;

            let (c0, c1, c2) = if is_bgr { (cb, cg, cr) } else { (cr, cg, cb) };

            let o = row_off + (x as usize) * bpp;
            unsafe {
                *ptr.add(o) = c0;
                *ptr.add(o + 1) = c1;
                *ptr.add(o + 2) = c2;
                if bpp == 4 {
                    *ptr.add(o + 3) = 0;
                }
            }
        }
    }

    let bx = (cx - radius).max(0);
    let by = (cy - radius).max(0);
    let bw = ((cx + radius + 1).min(w) - bx).max(0);
    let bh = ((cy + radius + 1).min(h) - by).max(0);
    fb.mark_dirty(bx, by, bw, bh);
}

// ---------------------------------------------------------------------------
// Sphere physics
// ---------------------------------------------------------------------------

const NUM_SPHERES: usize = 6;

struct Sphere {
    x: i64,   // fixed-point ×1024
    y: i64,
    vx: i64,  // velocity ×1024
    vy: i64,
    radius: i32,
    mass: i64, // proportional to radius² for 2D collision
    color: Color,
}

impl Sphere {
    fn new(x: i32, y: i32, vx: i32, vy: i32, radius: i32, color: Color) -> Self {
        Self {
            x: (x as i64) << 10,
            y: (y as i64) << 10,
            vx: (vx as i64) << 10,
            vy: (vy as i64) << 10,
            radius,
            mass: (radius as i64) * (radius as i64),
            color,
        }
    }

    fn px(&self) -> i32 { (self.x >> 10) as i32 }
    fn py(&self) -> i32 { (self.y >> 10) as i32 }

    fn step_scaled(&mut self, substeps: i32, dt_scale: i64) {
        self.x += self.vx * dt_scale / (substeps as i64 * 1024);
        self.y += self.vy * dt_scale / (substeps as i64 * 1024);
    }

    fn bounce_walls(&mut self, w: i32, h: i32) {
        let px = self.px();
        let py = self.py();
        let r = self.radius;
        if px - r < 0 {
            self.x = (r as i64) << 10;
            self.vx = self.vx.abs();
        }
        if px + r >= w {
            self.x = ((w - r - 1) as i64) << 10;
            self.vx = -(self.vx.abs());
        }
        if py - r < 0 {
            self.y = (r as i64) << 10;
            self.vy = self.vy.abs();
        }
        if py + r >= h {
            self.y = ((h - r - 1) as i64) << 10;
            self.vy = -(self.vy.abs());
        }
    }

    fn draw(&self, fb: &mut FrameBuf) {
        draw_sphere(fb, self.px(), self.py(), self.radius, self.color);
    }

    /// Check if a point (in pixel coords) is inside this sphere.
    fn hit_test(&self, px: i32, py: i32) -> bool {
        let dx = px - self.px();
        let dy = py - self.py();
        dx * dx + dy * dy <= self.radius * self.radius
    }

    /// Impel: boost velocity by 50% in current direction of travel.
    /// If nearly stationary, give a random-ish kick based on position.
    fn impel(&mut self) {
        let speed_sq = self.vx * self.vx + self.vy * self.vy;
        if speed_sq > (50 << 10) {
            self.vx = self.vx * 3 / 2;
            self.vy = self.vy * 3 / 2;
        } else {
            let kick = 300i64 << 10;
            let hash = (self.x ^ self.y) >> 10;
            self.vx = if hash & 1 == 0 { kick } else { -kick };
            self.vy = if hash & 2 == 0 { kick } else { -kick };
        }
    }

}

/// Elastic collision between two spheres. Modifies velocities in-place.
/// Uses 2D elastic collision formula with mass (proportional to r²).
fn collide_spheres(a: &mut Sphere, b: &mut Sphere) {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let dist_sq = dx * dx + dy * dy;

    let min_dist = ((a.radius + b.radius) as i64) << 10;
    let min_dist_sq = min_dist * min_dist;

    if dist_sq >= min_dist_sq || dist_sq == 0 {
        return;
    }

    // Relative velocity
    let dvx = a.vx - b.vx;
    let dvy = a.vy - b.vy;

    // Dot product of relative velocity and displacement
    let dot = dvx * dx + dvy * dy;

    // Only collide if spheres are approaching each other
    if dot <= 0 {
        return;
    }

    // Mass factor: 2 * m_other / (m_a + m_b), in ×1024 fixed point
    let total_mass = a.mass + b.mass;
    let factor_a = 2 * b.mass * 1024 / total_mass;
    let factor_b = 2 * a.mass * 1024 / total_mass;

    // Velocity change = factor * dot(dv, dx) / |dx|² * dx
    let impulse_a_x = factor_a * dot / dist_sq * dx / 1024;
    let impulse_a_y = factor_a * dot / dist_sq * dy / 1024;
    let impulse_b_x = factor_b * dot / dist_sq * dx / 1024;
    let impulse_b_y = factor_b * dot / dist_sq * dy / 1024;

    a.vx -= impulse_a_x;
    a.vy -= impulse_a_y;
    b.vx += impulse_b_x;
    b.vy += impulse_b_y;

    // Separate overlapping spheres
    let dist = isqrt_i64(dist_sq);
    if dist > 0 {
        let overlap = min_dist - dist;
        if overlap > 0 {
            let sep_x = overlap * dx / dist / 2;
            let sep_y = overlap * dy / dist / 2;
            a.x -= sep_x;
            a.y -= sep_y;
            b.x += sep_x;
            b.y += sep_y;
        }
    }
}

// ---------------------------------------------------------------------------
// FPS counter
// ---------------------------------------------------------------------------

struct FpsCounter {
    last_time_ns: u64,
    frame_count: u32,
    display_fps: u32,
    ttf_font: Option<CachedFont>,
    font_size: f32,
}

impl FpsCounter {
    fn new() -> Self {
        Self { last_time_ns: clock_monotonic_ns(), frame_count: 0, display_fps: 0, ttf_font: None, font_size: 20.0 }
    }

    fn with_ttf(mut self, font: Option<CachedFont>, size: f32) -> Self {
        self.ttf_font = font;
        self.font_size = size.max(10.0);
        self
    }

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

    fn draw(&mut self, fb: &mut FrameBuf) {
        let y = fb.height.saturating_sub(20);
        let mut buf = [b' '; 12];
        buf[0] = b'F'; buf[1] = b'P'; buf[2] = b'S'; buf[3] = b':'; buf[4] = b' ';

        let mut fps = self.display_fps;
        let mut len = 6; // default: "FPS: 0"
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
            len = 9; // digits occupy positions 5..=8
        }
        if let Some(ref mut f) = self.ttf_font {
            let text = core::str::from_utf8(&buf[..len]).unwrap_or("FPS: ?");
            ttf_font::draw_text(fb, f, text, 8, y as i32, self.font_size * 2.0, Color::GRAY);
        } else {
            font::draw_text(fb, &buf[..len], 8, y, Color::GRAY, 2);
        }
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

/// Window buffer dimensions.
const WIN_W: u32 = 400;
const WIN_H: u32 = 300;

fn main() {
    let boot_id = clock_monotonic_ns();
    println!("Bounce spheres demo starting (for Gus!) [boot_id={:016x}]", boot_id);

    // Try window-buffer mode first: BWM composites us as a floating window
    match Window::new(b"Bounce", WIN_W, WIN_H) {
        Ok(mut win) => {
            println!("[bounce] Window mode: id={} {}x{} [boot_id={:016x}]",
                     win.id(), WIN_W, WIN_H, boot_id);
            let mut spheres = make_spheres(WIN_W as i32, WIN_H as i32);
            run_window_loop(&mut win, &mut spheres);
            return;
        }
        Err(e) => {
            println!("[bounce] Window::new failed: {} — falling back to mmap", e);
        }
    }

    let info = match graphics::fbinfo() {
        Ok(info) => info,
        Err(_e) => { println!("Error: Could not get framebuffer info"); process::exit(1); }
    };

    let height = info.height as i32;
    let width = info.left_pane_width() as i32;

    let mut spheres = make_spheres(width, height);

    let fb_ptr = match graphics::fb_mmap() {
        Ok(ptr) => ptr,
        Err(e) => { println!("Error: Could not mmap framebuffer ({})", e); process::exit(1); }
    };

    let bpp = info.bytes_per_pixel as usize;
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

    println!("Starting bounce spheres demo ({} spheres, {}x{}, mmap)", NUM_SPHERES, width, height);
    run_mmap_loop(&mut fb, &mut spheres, width, height);
}

fn make_spheres(w: i32, h: i32) -> [Sphere; NUM_SPHERES] {
    // Spread spheres across the area, scaling radii to fit
    let scale_x = w as f32 / 400.0;
    let scale_y = h as f32 / 300.0;
    let scale = if scale_x < scale_y { scale_x } else { scale_y };
    let r = |base: i32| -> i32 { ((base as f32 * scale) as i32).max(10) };

    [
        Sphere::new((50.0 * scale_x) as i32,  (40.0 * scale_y) as i32,   300,  250, r(30), Color::rgb(220, 50, 50)),   // Red
        Sphere::new((200.0 * scale_x) as i32, (120.0 * scale_y) as i32, -280,  200, r(25), Color::rgb(50, 200, 50)),   // Green
        Sphere::new((100.0 * scale_x) as i32, (200.0 * scale_y) as i32,  250, -300, r(35), Color::rgb(60, 80, 255)),   // Blue
        Sphere::new((300.0 * scale_x) as i32, (80.0 * scale_y) as i32,  -220, -250, r(20), Color::rgb(240, 220, 50)),  // Yellow
        Sphere::new((150.0 * scale_x) as i32, (250.0 * scale_y) as i32,  280,  180, r(28), Color::rgb(220, 60, 220)),  // Magenta
        Sphere::new((320.0 * scale_x) as i32, (180.0 * scale_y) as i32, -200,  270, r(22), Color::rgb(50, 220, 220)),  // Cyan
    ]
}

/// Render bouncing spheres into a window buffer for BWM compositing.
fn run_window_loop(win: &mut Window, spheres: &mut [Sphere; NUM_SPHERES]) {
    let mut width = win.width() as i32;
    let mut height = win.height() as i32;
    let bg = Color::rgb(10, 10, 25);

    let ttf_font_opt = win.take_mono_font();
    let font_size = win.mono_size().max(10.0);
    let mut fps = FpsCounter::new().with_ttf(ttf_font_opt, font_size);
    let mut last_ns = clock_monotonic_ns();

    const TARGET_DT_NS: u64 = 16_666_667; // 60 FPS target

    loop {
        let now = clock_monotonic_ns();
        let elapsed_ns = now.saturating_sub(last_ns);
        last_ns = now;

        let dt_scale = if elapsed_ns > 0 {
            ((elapsed_ns * 1024) / TARGET_DT_NS) as i64
        } else {
            1024
        }.min(4096);

        // Poll input events — coordinates are already window-local
        for event in win.poll_events() {
            match event {
                Event::MouseButton { button: 1, pressed: true, x, y } => {
                    let mut hit = false;
                    for sphere in spheres.iter_mut() {
                        if sphere.hit_test(x, y) {
                            sphere.impel();
                            hit = true;
                            break;
                        }
                    }
                    if !hit {
                        for sphere in spheres.iter_mut() {
                            sphere.impel();
                        }
                    }
                }
                Event::KeyPress { ascii, .. } => {
                    match ascii {
                        b'+' | b'=' => {
                            // Scale all velocities up by ~15%
                            for sphere in spheres.iter_mut() {
                                sphere.vx = sphere.vx * 23 / 20;
                                sphere.vy = sphere.vy * 23 / 20;
                            }
                        }
                        b'-' => {
                            // Scale all velocities down by ~15%
                            for sphere in spheres.iter_mut() {
                                sphere.vx = sphere.vx * 20 / 23;
                                sphere.vy = sphere.vy * 20 / 23;
                            }
                        }
                        _ => {}
                    }
                }
                Event::Resized { width: w, height: h } => {
                    width = w as i32;
                    height = h as i32;
                }
                _ => {}
            }
        }

        physics_step(spheres, width, height, dt_scale);

        let fb = win.framebuf();
        fb.clear(bg);
        for sphere in spheres.iter() { sphere.draw(fb); }
        fps.tick();
        fps.draw(fb);

        let _ = win.present();
    }
}

/// Mmap software rendering loop.
fn run_mmap_loop(fb: &mut FrameBuf, spheres: &mut [Sphere; NUM_SPHERES], width: i32, height: i32) {
    let bg = Color::rgb(10, 10, 25);
    let mut fps = FpsCounter::new();
    let mut last_ns = clock_monotonic_ns();
    let mut prev_buttons: u32 = 0;

    const TARGET_DT_NS: u64 = 16_666_667;

    loop {
        let now = clock_monotonic_ns();
        let elapsed_ns = now.saturating_sub(last_ns);
        last_ns = now;

        let dt_scale = if elapsed_ns > 0 {
            ((elapsed_ns * 1024) / TARGET_DT_NS) as i64
        } else {
            1024
        }.min(4096);

        // Poll mouse for click detection (mmap uses left pane at origin)
        if let Ok((mx, my, buttons)) = graphics::mouse_state() {
            let left_pressed = (buttons & 1) != 0;
            let left_was_pressed = (prev_buttons & 1) != 0;

            if left_pressed && !left_was_pressed {
                let lx = mx as i32;
                let ly = my as i32;
                if lx >= 0 && lx < width && ly >= 0 && ly < height {
                    let mut hit = false;
                    for sphere in spheres.iter_mut() {
                        if sphere.hit_test(lx, ly) {
                            sphere.impel();
                            hit = true;
                            break;
                        }
                    }
                    if !hit {
                        for sphere in spheres.iter_mut() {
                            sphere.impel();
                        }
                    }
                }
            }
            prev_buttons = buttons;
        }

        physics_step(spheres, width, height, dt_scale);

        fb.clear(bg);
        for sphere in spheres.iter() { sphere.draw(fb); }
        fps.tick();
        fps.draw(fb);

        if let Some(dirty) = fb.take_dirty() {
            let _ = graphics::fb_flush_rect(dirty.x, dirty.y, dirty.w, dirty.h);
        }

        let _ = time::sleep_ms(1);
    }
}

/// Run one physics frame: substep movement, wall bounce, sphere-sphere collision.
fn physics_step(spheres: &mut [Sphere; NUM_SPHERES], w: i32, h: i32, dt_scale: i64) {
    const SUBSTEPS: i32 = 8;
    for _ in 0..SUBSTEPS {
        for s in spheres.iter_mut() {
            s.step_scaled(SUBSTEPS, dt_scale);
        }
        for s in spheres.iter_mut() {
            s.bounce_walls(w, h);
        }
        // Sphere-sphere collisions (all pairs)
        for i in 0..NUM_SPHERES {
            for j in (i + 1)..NUM_SPHERES {
                let (left, right) = spheres.split_at_mut(j);
                collide_spheres(&mut left[i], &mut right[0]);
            }
        }
    }
}

//! Bouncing spheres demo for Breenix.
//!
//! Renders 3D-looking spheres with specular highlights that bounce off screen
//! edges and collide with each other using elastic collision physics.
//!
//! Created for Gus!

use std::process;

use breengel::{CachedFont, Window, Event};
use libbreenix::time;

use libgfx::color::Color;
use libgfx::font;
use libgfx::shapes;
use libgfx::surface::Surface;
use libgfx::ttf_font;

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

/// Light direction (upper-left, normalized ×1024 for fixed-point):
/// (-0.5, -0.5, 0.707) normalized ≈ (-512, -512, 724).
const LIGHT_X: i64 = -512;
const LIGHT_Y: i64 = -512;
const LIGHT_Z: i64 = 724;

/// The integer square root of `n`: the largest `r` with `r * r <= n`.
fn isqrt_i64(n: i64) -> i64 {
    if n <= 0 {
        return 0;
    }
    let mut r = (n as f64).sqrt() as i64;
    while r * r > n {
        r -= 1;
    }
    while (r + 1) * (r + 1) <= n {
        r += 1;
    }
    r
}

/// The sphere's colour at `(dx, dy)` from its centre: the dot product of the surface normal
/// with the light direction, plus a specular highlight for the glossy spot.
fn shade(dx: i64, dy: i64, radius: i32, base_color: Color) -> Color {
    let r2 = (radius as i64) * (radius as i64);
    // Surface normal z component (sphere equation: nx=dx, ny=dy, nz=sqrt(r²-dx²-dy²))
    let nz = isqrt_i64((r2 - dx * dx - dy * dy).max(0));

    // Diffuse: dot(normal, light) / (|normal| * 1024)
    // |normal| = radius (since nx²+ny²+nz²=r²)
    let dot = dx * LIGHT_X + dy * LIGHT_Y + nz * LIGHT_Z;
    let diffuse = (dot * 1024 / (radius as i64 * 1024)).max(0).min(1024);

    // Specular: reflect = 2 * dot(n,l) * n - l, how close its z is to the viewer,
    // raised to the 8th power for shininess.
    let reflect_z = (2 * dot * nz / r2 - LIGHT_Z).max(0);
    let spec2 = reflect_z * reflect_z / 1024;
    let spec4 = spec2 * spec2 / 1024;
    let spec8 = spec4 * spec4 / 1024;

    // Ambient + diffuse + specular
    let ambient: i64 = 80; // out of 1024
    let intensity = (ambient + diffuse * 700 / 1024).min(1024);

    let channel = |c: u8| ((c as i64 * intensity / 1024) + spec8 * 255 / 1024).min(255) as u8;
    Color::rgb(channel(base_color.r), channel(base_color.g), channel(base_color.b))
}

/// How many nested circles draw one sphere.
const SHADE_STEPS: i32 = 14;

/// Draw a 3D-looking sphere with specular highlight and shading, light at upper-left.
///
/// The sphere is a stack of filled circles on the GPU: the whole disc, then circles that
/// shrink toward the specular highlight as they step from the far limb to the highlight,
/// each in the shading at its step's point. Each circle lies inside the one before it.
fn draw_sphere(fb: &mut Surface, cx: i32, cy: i32, radius: i32, base_color: Color) {
    let r = radius as f32;
    // The highlight, where the normal is halfway between the light and the viewer.
    let (hx, hy) = (-0.27 * r, -0.27 * r);
    // The far limb, opposite the highlight.
    let (ex, ey) = (0.67 * r, 0.67 * r);
    for i in 0..SHADE_STEPS {
        let t = i as f32 / (SHADE_STEPS - 1) as f32;
        let ring = (r * (1.0 - 0.85 * t)) as i32;
        let (ox, oy) = ((hx * t) as i32, (hy * t) as i32);
        let (qx, qy) = (ex + (hx - ex) * t, ey + (hy - ey) * t);
        let colour = shade(qx as i64, qy as i64, radius, base_color);
        shapes::fill_circle(fb, cx + ox, cy + oy, ring.max(1), colour);
    }
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

    fn draw(&self, fb: &mut Surface) {
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
    speed_pct: i32,
    ttf_font: Option<CachedFont>,
    font_size: f32,
}

impl FpsCounter {
    fn new() -> Self {
        Self { last_time_ns: clock_monotonic_ns(), frame_count: 0, display_fps: 0, speed_pct: 100, ttf_font: None, font_size: 20.0 }
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

    fn draw(&mut self, fb: &mut Surface) {
        let y = fb.height().saturating_sub(20);
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

        // Speed percentage in bottom-right
        let mut sbuf = [0u8; 16];
        let slen = format_speed(&mut sbuf, self.speed_pct);
        if let Some(ref mut f) = self.ttf_font {
            let text = core::str::from_utf8(&sbuf[..slen]).unwrap_or("?%");
            let tw = ttf_font::text_width(f, text, self.font_size * 2.0);
            let sx = (fb.width() as i32 - tw - 8).max(0);
            ttf_font::draw_text(fb, f, text, sx, y as i32, self.font_size * 2.0, Color::GRAY);
        } else {
            let tw = font::text_width(&sbuf[..slen], 2);
            let sx = fb.width().saturating_sub(tw + 8);
            font::draw_text(fb, &sbuf[..slen], sx, y, Color::GRAY, 2);
        }
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

fn format_speed(buf: &mut [u8], pct: i32) -> usize {
    let mut i = 0;
    let mut n = if pct < 0 { 0u32 } else { pct as u32 };
    if n == 0 {
        buf[i] = b'0'; i += 1;
    } else {
        let mut tmp = [0u8; 6];
        let mut tlen = 0;
        while n > 0 {
            tmp[tlen] = b'0' + (n % 10) as u8;
            n /= 10;
            tlen += 1;
        }
        for j in (0..tlen).rev() {
            buf[i] = tmp[j]; i += 1;
        }
    }
    buf[i] = b'%'; i += 1;
    i
}

/// Window buffer dimensions.
const WIN_W: u32 = 400;
const WIN_H: u32 = 300;

fn main() {
    let boot_id = clock_monotonic_ns();
    println!("Bounce spheres demo starting (for Gus!) [boot_id={:016x}]", boot_id);

    // Try window-buffer mode first: BWM composites us as a floating window
    match Window::new_without_fonts(b"Bounce", WIN_W, WIN_H) {
        Ok(mut win) => {
            println!("[bounce] Window mode: id={} {}x{} [boot_id={:016x}]",
                     win.id(), WIN_W, WIN_H, boot_id);
            let mut spheres = make_spheres(WIN_W as i32, WIN_H as i32);
            run_window_loop(&mut win, &mut spheres);
            return;
        }
        Err(e) => {
            println!("[bounce] Window::new failed: {}", e);
            process::exit(1);
        }
    }
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
    let mut first_frame = true;

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
                Event::MouseButton { button: breengel::buttons::LEFT, pressed: true, x, y } => {
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
                            for sphere in spheres.iter_mut() {
                                sphere.vx = sphere.vx * 23 / 20;
                                sphere.vy = sphere.vy * 23 / 20;
                            }
                            fps.speed_pct = (fps.speed_pct * 23 / 20).min(9999);
                        }
                        b'-' => {
                            for sphere in spheres.iter_mut() {
                                sphere.vx = sphere.vx * 20 / 23;
                                sphere.vy = sphere.vy * 20 / 23;
                            }
                            fps.speed_pct = (fps.speed_pct * 20 / 23).max(1);
                        }
                        _ => {}
                    }
                }
                Event::Scroll { delta_y, .. } => {
                    // Scroll up = faster, scroll down = slower
                    // Each tick scales by ~15%
                    for _ in 0..delta_y.abs() {
                        if delta_y > 0 {
                            for sphere in spheres.iter_mut() {
                                sphere.vx = sphere.vx * 23 / 20;
                                sphere.vy = sphere.vy * 23 / 20;
                            }
                            fps.speed_pct = (fps.speed_pct * 23 / 20).min(9999);
                        } else {
                            for sphere in spheres.iter_mut() {
                                sphere.vx = sphere.vx * 20 / 23;
                                sphere.vy = sphere.vy * 20 / 23;
                            }
                            fps.speed_pct = (fps.speed_pct * 20 / 23).max(1);
                        }
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

        let fb = match win.surface() {
            Ok(fb) => fb,
            Err(e) => {
                println!("[bounce] ERROR: no window surface: {}", e);
                process::exit(1);
            }
        };
        fb.batch(|fb| {
            fb.clear(bg);
            for sphere in spheres.iter() { sphere.draw(fb); }
        });
        fps.tick();
        fps.draw(fb);

        let presented = win.present();
        if first_frame {
            first_frame = false;
            match presented {
                Ok(()) => println!("[bounce] first frame presented: {}x{}", win.width(), win.height()),
                Err(e) => println!("[bounce] first frame present failed: {}", e),
            }
        }
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

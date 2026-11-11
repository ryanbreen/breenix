//! Bouncing spheres demo for Breenix.
//!
//! Renders 3D-looking spheres with specular highlights that bounce off screen
//! edges and collide with each other using elastic collision physics.
//!
//! Created for Gus!

use std::process;

#[path = "bounce/sim.rs"]
mod sim;
#[path = "demos/status.rs"]
mod status;
use sim::Simulation;

use breengel::{CachedFont, Window, Event, Handled};

use libgfx::color::Color;
use libgfx::font;
use libgfx::shapes;
use libgfx::surface::Surface;
use libgfx::ttf_font;

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
// FPS counter
// ---------------------------------------------------------------------------

struct FpsCounter {
    last_time_ms: u64,
    frame_count: u32,
    display_fps: u32,
    speed_pct: i32,
    ttf_font: Option<CachedFont>,
    font_size: f32,
}

impl FpsCounter {
    fn new() -> Self {
        Self { last_time_ms: 0, frame_count: 0, display_fps: 0, speed_pct: 100, ttf_font: None, font_size: 20.0 }
    }

    fn with_ttf(mut self, font: Option<CachedFont>, size: f32) -> Self {
        self.ttf_font = font;
        self.font_size = size.max(10.0);
        self
    }

    fn tick(&mut self, now: u64) {
        self.frame_count += 1;
        if self.frame_count >= 16 {
            let elapsed = now.saturating_sub(self.last_time_ms);
            if elapsed > 0 {
                self.display_fps = (self.frame_count as u64 * 1_000 / elapsed) as u32;
            }
            self.frame_count = 0;
            self.last_time_ms = now;
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
    println!("Bounce spheres demo starting (for Gus!)");

    // Try window-buffer mode first: BWM composites us as a floating window
    match Window::new_without_fonts(b"Bounce", WIN_W, WIN_H) {
        Ok(mut win) => {
            println!("[bounce] Window mode: id={} {}x{}", win.id(), WIN_W, WIN_H);
            let mut simulation = Simulation::new(WIN_W as i32, WIN_H as i32);
            run_window_loop(&mut win, &mut simulation);
            return;
        }
        Err(e) => {
            println!("[bounce] Window::new failed: {}", e);
            process::exit(1);
        }
    }
}

/// Render bouncing spheres into a window buffer for BWM compositing.
fn run_window_loop(win: &mut Window, simulation: &mut Simulation) {
    let bg = Color::rgb(10, 10, 25);

    let ttf_font_opt = win.take_mono_font();
    let font_size = win.mono_size().max(10.0);
    let mut fps = FpsCounter::new().with_ttf(ttf_font_opt, font_size);
    let mut last_tick = None;
    let mut first_frame = true;
    let mut redraw = true;
    win.set_animating(true);

    loop {
        if !redraw {
            let mut close = false;
            let mut resize = None;
            let mut handled_input = false;
            let count = win.wait_dispatch(100_000_000, |delivered| {
                handled_input |= delivered.serial != 0;
                match delivered.event {
                    Event::AnimationTick { time_ms } => {
                        simulation.advance(last_tick.map_or(1, |last| time_ms.saturating_sub(last)));
                        last_tick = Some(time_ms);
                        fps.tick(time_ms);
                        redraw = true;
                    }
                    Event::CloseRequested => close = true,
                    Event::MouseButton { button: breengel::buttons::LEFT, pressed: true, x, y } => {
                        simulation.click(x, y); redraw = true;
                    }
                    Event::KeyPress { ascii, .. } => match ascii {
                        b'+' | b'=' => { simulation.change_speed(true); redraw = true; }
                        b'-' => { simulation.change_speed(false); redraw = true; }
                        _ => {}
                    },
                    Event::Scroll { delta_y, .. } => {
                        for _ in 0..delta_y.unsigned_abs() { simulation.change_speed(delta_y > 0); }
                        redraw = true;
                    }
                    Event::ResizeRequested { width, height } => { resize = Some((width, height)); redraw = true; }
                    Event::Resized { width, height } => { simulation.resize(width as i32, height as i32); redraw = true; }
                    _ => {}
                }
                if redraw { Handled::Redraw } else { Handled::NoChange }
            }).unwrap_or_else(|e| { println!("[bounce] dispatch failed: {}", e); process::exit(1) });
            if close { return; }
            if let Some((width, height)) = resize {
                match win.apply_resize(width, height) {
                    Ok(()) => simulation.resize(width as i32, height as i32),
                    Err(e) => println!("[bounce] resize refused: {}x{}: {}", width, height, e),
                }
            }
            if !redraw {
                if count != 0 && handled_input { let _ = win.commit_no_change(); }
                continue;
            }
        }
        redraw = false;
        fps.speed_pct = simulation.speed_pct();
        let objects: Vec<_> = simulation.spheres.iter().map(|s| status::Object {
            bounds: [(s.px() - s.radius) as i64, (s.py() - s.radius) as i64,
                (s.radius * 2) as i64, (s.radius * 2) as i64],
            value: serde_json::json!({"vx": s.vx, "vy": s.vy}),
        }).collect();
        win.set_tree(status::build("bounce", simulation.speed_pct(), simulation.epochs(), simulation.all_inside(), win.width(), win.height(), "sphere", &objects));

        let fb = match win.surface() {
            Ok(fb) => fb,
            Err(e) => {
                println!("[bounce] ERROR: no window surface: {}", e);
                process::exit(1);
            }
        };
        fb.batch(|fb| {
            fb.clear(bg);
            for sphere in &simulation.spheres {
                let (r, g, b) = sphere.color;
                draw_sphere(fb, sphere.px(), sphere.py(), sphere.radius, Color::rgb(r, g, b));
            }
        });
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

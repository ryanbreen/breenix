//! Colorful confetti driven by window pointer events.
use breengel::{Event, Handled, Window};
use libgfx::{color::Color, surface::Surface};
#[path = "demos/status.rs"] mod status;

/// Fixed-point scale factor (8 bits of fractional precision)
const FP_SCALE: i32 = 256;

/// Convert integer to fixed-point
const fn to_fp(n: i32) -> i32 {
    n * FP_SCALE
}

/// Convert fixed-point to integer (truncate)
const fn from_fp(fp: i32) -> i32 {
    fp / FP_SCALE
}

/// Maximum number of confetti particles
const MAX_PARTICLES: usize = 200;

/// How many particles to spawn per frame when mouse is present
const SPAWN_RATE: usize = 6;

/// Particle lifetime in frames
const PARTICLE_LIFETIME: u16 = 90;

/// A single confetti particle
#[derive(Clone, Copy)]
struct Particle {
    x: i32,       // fixed-point position
    y: i32,
    vx: i32,      // fixed-point velocity
    vy: i32,
    life: u16,    // frames remaining
    max_life: u16,
    size: i32,    // pixel size (1-4)
    color: Color,
    spin: u8,     // rotation phase for visual variety
}

impl Particle {
    const DEAD: Self = Self {
        x: 0, y: 0, vx: 0, vy: 0,
        life: 0, max_life: 0, size: 0,
        color: Color::BLACK, spin: 0,
    };

    fn is_alive(&self) -> bool {
        self.life > 0
    }

    fn px(&self) -> i32 {
        from_fp(self.x)
    }

    fn py(&self) -> i32 {
        from_fp(self.y)
    }

    /// Alpha factor 0-255 based on remaining life
    fn alpha(&self) -> u8 {
        if self.max_life == 0 { return 0; }
        let frac = (self.life as u32 * 255) / self.max_life as u32;
        frac as u8
    }
}

/// Confetti color palette - bright, festive colors
const PALETTE: [Color; 12] = [
    Color::rgb(255, 50, 50),    // Red
    Color::rgb(50, 255, 50),    // Green
    Color::rgb(50, 100, 255),   // Blue
    Color::rgb(255, 220, 50),   // Yellow
    Color::rgb(255, 100, 200),  // Pink
    Color::rgb(50, 220, 255),   // Cyan
    Color::rgb(255, 150, 50),   // Orange
    Color::rgb(200, 50, 255),   // Purple
    Color::rgb(255, 255, 255),  // White
    Color::rgb(50, 255, 200),   // Turquoise
    Color::rgb(255, 80, 150),   // Hot pink
    Color::rgb(180, 255, 50),   // Lime
];

/// Simple deterministic pseudo-RNG (xorshift32)
struct Rng {
    state: u32,
}

impl Rng {
    fn new(seed: u32) -> Self {
        Self { state: if seed == 0 { 1 } else { seed } }
    }

    fn next(&mut self) -> u32 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.state = x;
        x
    }

    /// Random i32 in [-range, range]
    fn range_signed(&mut self, range: i32) -> i32 {
        let r = (self.next() % (range as u32 * 2 + 1)) as i32;
        r - range
    }

    /// Random u32 in [0, max)
    fn range(&mut self, max: u32) -> u32 {
        self.next() % max
    }
}

/// Confetti particle system
struct ConfettiSystem {
    particles: [Particle; MAX_PARTICLES],
    next_slot: usize,
    bounds_w: i32,
    bounds_h: i32,
    rng: Rng,
    gravity: i32,
    bg_color: Color,
}

impl ConfettiSystem {
    fn new(width: i32, height: i32, seed: u32) -> Self {
        Self {
            particles: [Particle::DEAD; MAX_PARTICLES],
            next_slot: 0,
            bounds_w: width,
            bounds_h: height,
            rng: Rng::new(seed),
            gravity: 12, // gentle gravity in FP units
            bg_color: Color::rgb(10, 12, 25),
        }
    }

    /// Spawn a burst of confetti particles at (cx, cy)
    fn spawn_burst(&mut self, cx: i32, cy: i32, count: usize) {
        for _ in 0..count {
            let vx = self.rng.range_signed(600);
            let vy = -200 - (self.rng.range(500) as i32); // upward bias
            let size = 2 + (self.rng.range(3) as i32);    // 2-4 pixels
            let lifetime = (PARTICLE_LIFETIME as u32 - 20 + self.rng.range(40)) as u16;
            let color_idx = self.rng.range(PALETTE.len() as u32) as usize;
            let spin = self.rng.range(256) as u8;

            let p = Particle {
                x: to_fp(cx),
                y: to_fp(cy),
                vx,
                vy,
                life: lifetime,
                max_life: lifetime,
                size,
                color: PALETTE[color_idx],
                spin,
            };

            self.particles[self.next_slot] = p;
            self.next_slot = (self.next_slot + 1) % MAX_PARTICLES;
        }
    }

    fn update(&mut self) {
        for p in self.particles.iter_mut() {
            if !p.is_alive() {
                continue;
            }

            // Apply gravity
            p.vy += self.gravity;

            // Air resistance (very slight damping)
            p.vx = (p.vx * 253) / FP_SCALE;
            p.vy = (p.vy * 254) / FP_SCALE;

            // Update position
            p.x += p.vx;
            p.y += p.vy;

            // Spin animation
            p.spin = p.spin.wrapping_add(7);

            // Age the particle
            p.life = p.life.saturating_sub(1);

            // Kill particles that fall off screen
            let px = from_fp(p.x);
            let py = from_fp(p.y);
            if px < 0 || py < 0 || px + p.size > self.bounds_w || py + p.size > self.bounds_h {
                p.life = 0;
            }
        }
    }

    fn resize(&mut self, width: i32, height: i32) {
        self.bounds_w = width; self.bounds_h = height;
        for p in &mut self.particles {
            if p.px() < 0 || p.py() < 0 || p.px() + p.size > width || p.py() + p.size > height { p.life = 0; }
        }
    }

    fn render(&self, fb: &mut Surface) {
        fb.clear(self.bg_color);

        for p in self.particles.iter() {
            if !p.is_alive() {
                continue;
            }

            let px = p.px();
            let py = p.py();
            let alpha = p.alpha();

            // Fade color based on remaining life
            let r = ((p.color.r as u16 * alpha as u16) / 255) as u8;
            let g = ((p.color.g as u16 * alpha as u16) / 255) as u8;
            let b = ((p.color.b as u16 * alpha as u16) / 255) as u8;
            let color = Color::rgb(r, g, b);

            // Draw confetti as small rectangles with spin-based aspect ratio
            let spin_phase = p.spin as i32;
            // Use spin to vary width/height for a tumbling effect
            let w = if spin_phase < 128 {
                p.size
            } else {
                (p.size + 1) / 2
            };
            let h = p.size;

            // Draw the confetti piece
            for dy in 0..h {
                for dx in 0..w {
                    fb.put_pixel((px + dx) as usize, (py + dy) as usize, color);
                }
            }

            // Draw a bright highlight on living particles for sparkle
            if alpha > 180 && p.size >= 3 {
                fb.put_pixel(px as usize, py as usize, Color::WHITE);
            }
        }
    }
}


fn main() {
    let mut win = Window::new_without_fonts(b"Confetti", 400, 300).expect("window");
    let mut system = ConfettiSystem::new(400, 300, 1);
    let mut last = None; let mut remainder = 0u64; let mut epochs = 0u64;
    let mut mouse = None; let mut redraw = true;
    win.set_animating(true);
    loop {
        if !redraw {
            let mut close = false; let mut resize = None; let mut input = false;
            let count = win.wait_dispatch(100_000_000, |d| {
                input |= d.serial != 0;
                match d.event {
                    Event::AnimationTick { time_ms } => {
                        let elapsed = remainder + last.map_or(1, |t: u64| time_ms.saturating_sub(t));
                        last = Some(time_ms); remainder = elapsed % 16;
                        for _ in 0..(elapsed / 16).min(1000) {
                            if let Some((x,y)) = mouse { if x + 4 <= system.bounds_w && y + 4 <= system.bounds_h { system.spawn_burst(x,y,SPAWN_RATE); } }
                            if epochs % 60 == 0 { system.spawn_burst(system.bounds_w/2, 20.min(system.bounds_h-1), 8); }
                            system.update(); epochs = epochs.saturating_add(1);
                        }
                        redraw = true;
                    }
                    Event::MouseMove { x, y } => { if x >= 0 && y >= 0 && x + 4 <= system.bounds_w && y + 4 <= system.bounds_h { mouse = Some((x,y)); system.spawn_burst(x,y,2); } else { mouse = None; } redraw = true; }
                    Event::CloseRequested => close = true,
                    Event::ResizeRequested { width, height } => { resize = Some((width,height)); redraw = true; }
                    Event::Resized { width, height } => { system.resize(width as i32, height as i32); redraw = true; }
                    _ => {}
                }
                if redraw { Handled::Redraw } else { Handled::NoChange }
            }).expect("dispatch");
            if close { return; }
            if let Some((w,h)) = resize { if win.apply_resize(w,h).is_ok() { system.resize(w as i32, h as i32); } }
            if !redraw { if count != 0 && input { let _ = win.commit_no_change(); } continue; }
        }
        redraw = false;
        let objects: Vec<_> = system.particles.iter().filter(|p| p.is_alive()).map(|p| status::Object { bounds: [p.px() as i64,p.py() as i64,p.size as i64,p.size as i64], value: serde_json::json!({}) }).collect();
        let inside = objects.iter().all(|o| o.bounds[0] >= 0 && o.bounds[1] >= 0 && o.bounds[0]+o.bounds[2] <= system.bounds_w as i64 && o.bounds[1]+o.bounds[3] <= system.bounds_h as i64);
        win.set_tree(status::build("confetti",100,epochs,inside,win.width(),win.height(),"particle",&objects));
        system.render(win.surface().expect("surface"));
        win.present().expect("present");
    }
}

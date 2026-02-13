//! Confetti particle demo for Breenix
//!
//! Showers exploding colorful confetti particles from the mouse pointer.
//! Uses mmap'd framebuffer for zero-syscall drawing via libgfx.
//! Run it from the shell with: confetti

use std::process;

use libbreenix::graphics;
use libbreenix::time;

use libgfx::color::Color;
use libgfx::font;
use libgfx::framebuf::FrameBuf;

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

/// Confetti system managing all particles
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
            if px < -20 || px > self.bounds_w + 20 || py > self.bounds_h + 20 {
                p.life = 0;
            }
        }
    }

    fn render(&self, fb: &mut FrameBuf) {
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

// ---------------------------------------------------------------------------
// FPS counter
// ---------------------------------------------------------------------------

fn clock_monotonic_ns() -> u64 {
    let ts = time::now_monotonic().unwrap_or_default();
    (ts.tv_sec as u64) * 1_000_000_000 + (ts.tv_nsec as u64)
}

struct FpsCounter {
    last_time_ns: u64,
    frame_count: u32,
    display_fps: u32,
}

impl FpsCounter {
    fn new() -> Self {
        Self {
            last_time_ns: clock_monotonic_ns(),
            frame_count: 0,
            display_fps: 0,
        }
    }

    fn tick(&mut self) {
        self.frame_count += 1;
        if self.frame_count >= 16 {
            let now = clock_monotonic_ns();
            let elapsed = now.saturating_sub(self.last_time_ns);
            if elapsed > 0 {
                self.display_fps =
                    (self.frame_count as u64 * 1_000_000_000 / elapsed) as u32;
            }
            self.frame_count = 0;
            self.last_time_ns = now;
        }
    }

    fn draw(&self, fb: &mut FrameBuf) {
        let y = fb.height.saturating_sub(20);
        let mut buf = [b' '; 12];
        buf[0] = b'F';
        buf[1] = b'P';
        buf[2] = b'S';
        buf[3] = b':';
        buf[4] = b' ';

        let mut fps = self.display_fps;
        if fps == 0 {
            buf[5] = b'0';
        } else {
            let mut pos = 8;
            while fps > 0 && pos >= 5 {
                buf[pos] = b'0' + (fps % 10) as u8;
                fps /= 10;
                if pos == 0 {
                    break;
                }
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
    println!("Confetti demo starting...");

    let info = match graphics::fbinfo() {
        Ok(info) => info,
        Err(_e) => {
            println!("Error: Could not get framebuffer info");
            process::exit(1);
        }
    };

    let width = info.left_pane_width() as i32;
    let height = info.height as i32;
    let bpp = info.bytes_per_pixel as usize;

    let fb_ptr = match graphics::fb_mmap() {
        Ok(ptr) => ptr,
        Err(e) => {
            println!("Error: Could not mmap framebuffer ({})", e);
            process::exit(1);
        }
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

    // Seed RNG from monotonic clock
    let seed = clock_monotonic_ns() as u32;
    let mut system = ConfettiSystem::new(width, height, seed);

    println!("Starting confetti animation loop...");

    let mut fps = FpsCounter::new();
    let mut frame: u32 = 0;

    // Track last mouse position to detect movement
    let mut last_mx: u32 = 0;
    let mut last_my: u32 = 0;

    loop {
        // Query mouse position via syscall
        let (mx, my) = graphics::mouse_pos().unwrap_or((0, 0));

        // Scale mouse position to left pane coordinates
        // Mouse is in full-screen coordinates; left pane is the left half
        let pane_mx = mx as i32;
        let pane_my = my as i32;

        // Spawn confetti at the mouse position if it's within the left pane
        if pane_mx < width && pane_mx >= 0 && pane_my >= 0 && pane_my < height {
            // Spawn on every frame for continuous confetti shower
            system.spawn_burst(pane_mx, pane_my, SPAWN_RATE);

            // Extra burst on mouse movement
            if mx != last_mx || my != last_my {
                system.spawn_burst(pane_mx, pane_my, 2);
            }
        }

        last_mx = mx;
        last_my = my;

        // Also spawn a periodic burst from center as ambient confetti
        // every ~60 frames (roughly once per second)
        if frame % 60 == 0 {
            let cx = width / 2;
            system.spawn_burst(cx, 20, 8);
        }

        system.update();
        system.render(&mut fb);

        fps.tick();
        fps.draw(&mut fb);

        // Draw a small crosshair at mouse position (if in pane)
        if pane_mx < width && pane_mx >= 0 && pane_my >= 0 && pane_my < height {
            let cx = pane_mx as u32;
            let cy = pane_my as u32;
            for d in 1..=4_u32 {
                fb.put_pixel(cx.wrapping_add(d) as usize, cy as usize, Color::WHITE);
                fb.put_pixel(cx.wrapping_sub(d) as usize, cy as usize, Color::WHITE);
                fb.put_pixel(cx as usize, cy.wrapping_add(d) as usize, Color::WHITE);
                fb.put_pixel(cx as usize, cy.wrapping_sub(d) as usize, Color::WHITE);
            }
        }

        if let Some(dirty) = fb.take_dirty() {
            let _ = graphics::fb_flush_rect(dirty.x, dirty.y, dirty.w, dirty.h);
        } else {
            let _ = graphics::fb_flush();
        }

        // ~60 FPS
        let _ = time::sleep_ms(16);
        frame = frame.wrapping_add(1);
    }
}

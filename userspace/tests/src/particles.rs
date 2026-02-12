//! Particle physics simulation for Breenix (std version)
//!
//! Bouncing particles with gravity, damping, and collision detection.
//! Run it from the shell with: particles

use std::process;

use libbreenix::graphics::{self, rgb};
use libbreenix::time;

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

/// A single particle with position, velocity, and visual properties
#[derive(Clone, Copy)]
struct Particle {
    x: i32,
    y: i32,
    vx: i32,
    vy: i32,
    radius: i32,
    color: u32,
    mass: i32,
    trail: u8,
}

impl Particle {
    fn new(x: i32, y: i32, radius: i32, color: u32) -> Self {
        Self {
            x: to_fp(x),
            y: to_fp(y),
            vx: 0,
            vy: 0,
            radius,
            color,
            mass: radius,
            trail: 0,
        }
    }

    fn with_velocity(mut self, vx: i32, vy: i32) -> Self {
        self.vx = vx;
        self.vy = vy;
        self
    }

    fn px(&self) -> i32 {
        from_fp(self.x)
    }

    fn py(&self) -> i32 {
        from_fp(self.y)
    }
}

/// Physics configuration
struct Config {
    gravity: i32,
    damping: i32,
    restitution: i32,
    min_velocity: i32,
}

impl Config {
    fn default_config() -> Self {
        Self {
            gravity: 32,
            damping: 255,
            restitution: 250,
            min_velocity: 3,
        }
    }
}

/// Maximum number of particles
const MAX_PARTICLES: usize = 16;

/// Particle system
struct ParticleSystem {
    particles: [Particle; MAX_PARTICLES],
    count: usize,
    bounds: (i32, i32, i32, i32),
    config: Config,
    bg_color: u32,
}

impl ParticleSystem {
    fn new(left: i32, top: i32, right: i32, bottom: i32) -> Self {
        Self {
            particles: [Particle::new(0, 0, 0, 0); MAX_PARTICLES],
            count: 0,
            bounds: (left, top, right, bottom),
            config: Config::default_config(),
            bg_color: rgb(15, 20, 35),
        }
    }

    fn add(&mut self, particle: Particle) {
        if self.count < MAX_PARTICLES {
            self.particles[self.count] = particle;
            self.count += 1;
        }
    }

    fn update(&mut self) {
        // Phase 1: Apply forces and update velocities
        for i in 0..self.count {
            let p = &mut self.particles[i];
            p.vy += self.config.gravity;
            p.vx = (p.vx * self.config.damping) / FP_SCALE;
            p.vy = (p.vy * self.config.damping) / FP_SCALE;
        }

        // Phase 2: Update positions
        for i in 0..self.count {
            let p = &mut self.particles[i];
            p.x += p.vx;
            p.y += p.vy;
        }

        // Phase 3: Handle boundary collisions
        self.handle_boundary_collisions();

        // Phase 4: Handle particle-particle collisions
        self.handle_particle_collisions();

        // Phase 5: Update trail effects
        for i in 0..self.count {
            let p = &mut self.particles[i];
            let speed = isqrt((p.vx * p.vx + p.vy * p.vy) as u32);
            p.trail = (speed.min(255) as u8).saturating_mul(2).min(255);
        }
    }

    fn handle_boundary_collisions(&mut self) {
        let (left, top, right, bottom) = self.bounds;

        for i in 0..self.count {
            let p = &mut self.particles[i];
            let r = to_fp(p.radius);
            let min_x = to_fp(left) + r;
            let max_x = to_fp(right) - r;
            let min_y = to_fp(top) + r;
            let max_y = to_fp(bottom) - r;

            if p.x < min_x {
                p.x = min_x;
                p.vx = (-p.vx * self.config.restitution) / FP_SCALE;
            }
            if p.x > max_x {
                p.x = max_x;
                p.vx = (-p.vx * self.config.restitution) / FP_SCALE;
            }
            if p.y < min_y {
                p.y = min_y;
                p.vy = (-p.vy * self.config.restitution) / FP_SCALE;
            }
            if p.y > max_y {
                p.y = max_y;
                p.vy = (-p.vy * self.config.restitution) / FP_SCALE;

                if p.vy.abs() < self.config.min_velocity {
                    p.vy = 0;
                }
            }
        }
    }

    fn handle_particle_collisions(&mut self) {
        if self.count < 2 {
            return;
        }

        for i in 0..self.count {
            for j in (i + 1)..self.count {
                let (p1_x, p1_y, p1_vx, p1_vy, p1_r, p1_m) = {
                    let p = &self.particles[i];
                    (p.x, p.y, p.vx, p.vy, p.radius, p.mass)
                };
                let (p2_x, p2_y, p2_vx, p2_vy, p2_r, p2_m) = {
                    let p = &self.particles[j];
                    (p.x, p.y, p.vx, p.vy, p.radius, p.mass)
                };

                let dx = p2_x - p1_x;
                let dy = p2_y - p1_y;
                let dist_sq = (dx / 16) * (dx / 16) + (dy / 16) * (dy / 16);

                let min_dist = to_fp(p1_r + p2_r) / 16;
                let min_dist_sq = min_dist * min_dist;

                if dist_sq < min_dist_sq && dist_sq > 0 {
                    let dist = isqrt(dist_sq as u32) as i32 * 16;
                    if dist == 0 {
                        continue;
                    }

                    let nx = (dx * FP_SCALE) / dist;
                    let ny = (dy * FP_SCALE) / dist;

                    let dvx = p1_vx - p2_vx;
                    let dvy = p1_vy - p2_vy;

                    let dvn = (dvx * nx + dvy * ny) / FP_SCALE;

                    if dvn > 0 {
                        let total_mass = p1_m + p2_m;
                        let impulse = (dvn * self.config.restitution * 2) / (FP_SCALE * total_mass / p1_m);

                        let impulse1 = (impulse * p2_m) / total_mass;
                        let impulse2 = (impulse * p1_m) / total_mass;

                        self.particles[i].vx -= (impulse1 * nx) / FP_SCALE;
                        self.particles[i].vy -= (impulse1 * ny) / FP_SCALE;

                        self.particles[j].vx += (impulse2 * nx) / FP_SCALE;
                        self.particles[j].vy += (impulse2 * ny) / FP_SCALE;

                        let overlap = to_fp(p1_r + p2_r) - dist;
                        if overlap > 0 {
                            let sep = overlap / 2 + FP_SCALE;
                            self.particles[i].x -= (sep * nx) / FP_SCALE;
                            self.particles[i].y -= (sep * ny) / FP_SCALE;
                            self.particles[j].x += (sep * nx) / FP_SCALE;
                            self.particles[j].y += (sep * ny) / FP_SCALE;
                        }
                    }
                }
            }
        }
    }

    fn render(&self) {
        let (left, top, _right, _bottom) = self.bounds;

        let _ = graphics::fb_clear(self.bg_color);

        for i in 0..self.count {
            let p = &self.particles[i];
            let px = p.px();
            let py = p.py();

            // Draw glow (larger, dimmer circle)
            if p.trail > 30 {
                let glow_radius = p.radius + 3;
                let glow_intensity = p.trail / 4;
                let (r, g, b) = unpack_rgb(p.color);
                let glow_color = rgb(
                    ((r as u16 * glow_intensity as u16) / 255) as u8,
                    ((g as u16 * glow_intensity as u16) / 255) as u8,
                    ((b as u16 * glow_intensity as u16) / 255) as u8,
                );
                let _ = graphics::fb_fill_circle(px, py, glow_radius, glow_color);
            }

            // Draw main particle
            let _ = graphics::fb_fill_circle(px, py, p.radius, p.color);

            // Draw highlight (small white dot for 3D effect)
            if p.radius > 6 {
                let highlight_x = px - p.radius / 3;
                let highlight_y = py - p.radius / 3;
                let highlight_r = (p.radius / 4).max(2);
                let _ = graphics::fb_fill_circle(highlight_x, highlight_y, highlight_r, rgb(255, 255, 255));
            }
        }

        // Draw boundary indicator
        let _ = graphics::fb_fill_circle(left + 5, top + 5, 3, rgb(100, 100, 100));
    }

    fn spawn_demo_particles(&mut self) {
        let (left, top, right, bottom) = self.bounds;
        let width = right - left;
        let cx = left + width / 2;
        let cy = top + (bottom - top) / 3;

        let colors = [
            rgb(255, 100, 100),  // Coral red
            rgb(100, 255, 150),  // Mint green
            rgb(100, 150, 255),  // Sky blue
            rgb(255, 200, 100),  // Golden yellow
            rgb(255, 100, 255),  // Magenta
            rgb(100, 255, 255),  // Cyan
            rgb(255, 150, 100),  // Orange
            rgb(200, 100, 255),  // Purple
        ];

        let num_particles: i32 = 12;
        for i in 0..num_particles {
            let angle_idx = ((i * 256) / num_particles) as usize;
            let (sin_val, cos_val) = sin_cos_table(angle_idx);

            let radius_offset = 40 + (i % 3) * 20;
            let px = cx + (cos_val * radius_offset) / FP_SCALE;
            let py = cy + (sin_val * radius_offset) / FP_SCALE;

            let particle_radius = 8 + (i % 4) * 4;

            let speed = 300 + (i % 5) * 100;
            let vx = (cos_val * speed) / FP_SCALE;
            let vy = (sin_val * speed) / FP_SCALE - 200;

            let color = colors[(i as usize) % colors.len()];
            let particle = Particle::new(px, py, particle_radius, color).with_velocity(vx, vy);
            self.add(particle);
        }

        self.add(
            Particle::new(cx - 60, cy - 80, 20, rgb(255, 220, 100)).with_velocity(150, -400),
        );
        self.add(
            Particle::new(cx + 60, cy - 80, 18, rgb(100, 200, 255)).with_velocity(-150, -350),
        );
    }
}

/// Integer square root
fn isqrt(n: u32) -> u32 {
    if n == 0 {
        return 0;
    }
    let mut x = n;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

/// Unpack RGB color
fn unpack_rgb(color: u32) -> (u8, u8, u8) {
    let r = ((color >> 16) & 0xFF) as u8;
    let g = ((color >> 8) & 0xFF) as u8;
    let b = (color & 0xFF) as u8;
    (r, g, b)
}

/// Simple sin/cos lookup table (256 entries = full circle)
fn sin_cos_table(angle: usize) -> (i32, i32) {
    const SIN_TABLE: [i32; 64] = [
        0, 6, 13, 19, 25, 31, 37, 44, 50, 56, 62, 68, 74, 80, 86, 92, 97, 103, 109, 114, 120, 125,
        131, 136, 141, 147, 152, 157, 162, 166, 171, 176, 180, 185, 189, 193, 197, 201, 205, 209,
        212, 216, 219, 222, 225, 228, 231, 233, 236, 238, 240, 242, 244, 246, 247, 249, 250, 251,
        252, 253, 254, 254, 255, 255,
    ];

    let angle = angle & 0xFF;
    let quadrant = angle / 64;
    let idx = angle % 64;

    let (sin_val, cos_val) = match quadrant {
        0 => (SIN_TABLE[idx], SIN_TABLE[63 - idx]),
        1 => (SIN_TABLE[63 - idx], -SIN_TABLE[idx]),
        2 => (-SIN_TABLE[idx], -SIN_TABLE[63 - idx]),
        _ => (-SIN_TABLE[63 - idx], SIN_TABLE[idx]),
    };

    (sin_val, cos_val)
}

fn main() {
    println!("Particle Physics Simulation starting...");

    // Get framebuffer info
    let info = match graphics::fbinfo() {
        Ok(info) => info,
        Err(e) => {
            println!("Error: Could not get framebuffer info");
            process::exit(e);
        }
    };

    let width = info.left_pane_width() as i32;
    let height = info.height as i32;

    println!("Creating particle system...");

    let mut system = ParticleSystem::new(0, 0, width, height);
    system.spawn_demo_particles();

    println!("Starting animation loop...");

    let mut frame = 0u32;

    // Animation loop
    loop {
        if frame == 0 {
            println!("Frame 0: updating...");
        }

        system.update();

        if frame == 0 {
            println!("Frame 0: rendering...");
        }

        system.render();

        if frame == 0 {
            println!("Frame 0: flushing...");
        }

        let _ = graphics::fb_flush();

        if frame == 0 {
            println!("Frame 0: sleeping...");
        }

        // ~60 FPS
        time::sleep_ms(16);

        if frame == 0 {
            println!("Frame 0: complete!");
        }

        frame = frame.wrapping_add(1);
    }
}

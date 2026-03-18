//! Particle system for burst effects on icon click, hover sparks, etc.

use alloc::vec::Vec;

use libgfx::color::Color;
use libgfx::framebuf::FrameBuf;
use libgfx::shapes;

/// A single animated particle.
#[derive(Clone, Copy)]
pub struct Particle {
    pub x: f32,
    pub y: f32,
    pub vx: f32,
    pub vy: f32,
    /// Remaining life fraction: 1.0 = just born, 0.0 = dead.
    pub life: f32,
    /// Total lifetime in seconds.
    pub max_life: f32,
    /// Radius in pixels at full life.
    pub size: f32,
    pub color: Color,
    /// Downward acceleration (pixels/s²). Use 0 for floating particles.
    pub gravity: f32,
    /// Velocity damping per second (0.0 = frictionless, 1.0 = instant stop).
    pub friction: f32,
}

impl Particle {
    pub fn alive(&self) -> bool {
        self.life > 0.0
    }

    /// Step the particle forward by `dt` seconds.
    pub fn update(&mut self, dt: f32) {
        if self.life <= 0.0 {
            return;
        }
        self.vy += self.gravity * dt;
        let damp = 1.0 - self.friction * dt;
        self.vx *= damp;
        self.vy *= damp;
        self.x += self.vx * dt;
        self.y += self.vy * dt;
        self.life -= dt / self.max_life;
        if self.life < 0.0 {
            self.life = 0.0;
        }
    }

    /// Draw this particle into `fb`, offset by (offset_x, offset_y).
    pub fn draw(&self, fb: &mut FrameBuf, offset_x: i32, offset_y: i32) {
        if self.life <= 0.0 {
            return;
        }
        // Fade colour with remaining life.
        let alpha = self.life;
        let r = (self.color.r as f32 * alpha) as u8;
        let g = (self.color.g as f32 * alpha) as u8;
        let b = (self.color.b as f32 * alpha) as u8;
        let c = Color::rgb(r, g, b);

        let px = self.x as i32 + offset_x;
        let py = self.y as i32 + offset_y;
        // Shrink radius as the particle dies.
        let sz = (self.size * (0.5 + 0.5 * self.life)) as i32;

        if sz <= 1 {
            if px >= 0
                && py >= 0
                && (px as usize) < fb.width
                && (py as usize) < fb.height
            {
                shapes::fill_rect(fb, px, py, 1, 1, c);
            }
        } else {
            shapes::fill_rect(fb, px - sz / 2, py - sz / 2, sz, sz, c);
        }
    }
}

/// An automatically-recycling pool of particles.
pub struct ParticlePool {
    pub particles: Vec<Particle>,
    max_particles: usize,
}

impl ParticlePool {
    pub fn new(max_particles: usize) -> Self {
        Self {
            particles: Vec::with_capacity(max_particles),
            max_particles,
        }
    }

    /// Emit a particle. Finds a dead slot first; falls back to the oldest entry
    /// when the pool is at capacity.
    pub fn emit(&mut self, p: Particle) {
        // Try to reuse a dead slot.
        for slot in self.particles.iter_mut() {
            if !slot.alive() {
                *slot = p;
                return;
            }
        }
        // Under the cap — just push.
        if self.particles.len() < self.max_particles {
            self.particles.push(p);
        } else if !self.particles.is_empty() {
            // Overwrite the first slot (oldest by insertion order).
            self.particles[0] = p;
        }
    }

    /// Step all particles forward by `dt` seconds.
    pub fn update(&mut self, dt: f32) {
        for p in self.particles.iter_mut() {
            p.update(dt);
        }
    }

    /// Draw all live particles.
    pub fn draw(&self, fb: &mut FrameBuf, offset_x: i32, offset_y: i32) {
        for p in &self.particles {
            p.draw(fb, offset_x, offset_y);
        }
    }

    /// Kill all particles immediately.
    pub fn clear(&mut self) {
        for p in self.particles.iter_mut() {
            p.life = 0.0;
        }
    }

    /// Number of currently live particles.
    pub fn active_count(&self) -> usize {
        self.particles.iter().filter(|p| p.alive()).count()
    }
}

/// Fast xorshift32 pseudo-random number generator.
///
/// Not cryptographically secure — used only for visual variety.
pub struct Rng {
    state: u32,
}

impl Rng {
    pub fn new(seed: u32) -> Self {
        Self {
            state: if seed == 0 { 0xDEAD_BEEF } else { seed },
        }
    }

    /// Advance and return the next u32.
    pub fn next_u32(&mut self) -> u32 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.state = x;
        x
    }

    /// Random f32 in 0.0..=1.0.
    pub fn next_f32(&mut self) -> f32 {
        (self.next_u32() & 0xFFFF) as f32 / 65535.0
    }

    /// Random f32 in `min..max`.
    pub fn range(&mut self, min: f32, max: f32) -> f32 {
        min + self.next_f32() * (max - min)
    }

    /// Random i32 in `min..=max`.
    pub fn range_i32(&mut self, min: i32, max: i32) -> i32 {
        if max <= min {
            return min;
        }
        min + (self.next_u32() % (max - min + 1) as u32) as i32
    }
}

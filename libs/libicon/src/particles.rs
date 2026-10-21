//! Particle system for burst effects on icon click, hover sparks, etc.

use alloc::vec::Vec;

use libgfx::color::Color;
use libgfx::surface::Surface as FrameBuf;
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

        fn floor_pixel(v: f32) -> i32 {
            let i = v as i32;
            if v < i as f32 { i.saturating_sub(1) } else { i }
        }
        let px = floor_pixel(self.x).saturating_add(offset_x);
        let py = floor_pixel(self.y).saturating_add(offset_y);
        // Shrink radius as the particle dies.
        let sz = (self.size * (0.5 + 0.5 * self.life)) as i32;

        if sz <= 1 {
            if px >= 0
                && py >= 0
                && (px as usize) < fb.width()
                && (py as usize) < fb.height()
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
    particles: Vec<Particle>,
    max_particles: usize,
    // Lifetime is elapsed time, not repeated subtraction from an f32 fraction.
    remaining_ns: Vec<f64>,
    bounds: Option<(f32,f32,f32,f32)>,
    remainder_ns: u64,
}

impl ParticlePool {
    pub fn new(max_particles: usize) -> Self {
        Self {
            particles: Vec::with_capacity(max_particles),
            max_particles,
            remaining_ns: Vec::with_capacity(max_particles),
            bounds: None, remainder_ns: 0,
        }
    }

    /// Confine particle footprints to [left,right) × [top,bottom).
    pub fn with_bounds(capacity: usize, left: f32, top: f32, right: f32, bottom: f32) -> Self {
        assert!(left.is_finite() && top.is_finite() && right.is_finite() && bottom.is_finite()
            && right > left && bottom > top);
        let mut pool = Self::new(capacity);
        pool.bounds = Some((left,top,right,bottom));
        pool
    }
    fn confine(bounds: Option<(f32,f32,f32,f32)>, p: &mut Particle) {
        if let Some((l,t,r,b)) = bounds {
            // Confine to full pixel cells, including fractional negative edges.
            let ceil = |v: f32| { let i = v as i32; if v > i as f32 { i.saturating_add(1) } else { i } };
            let floor = |v: f32| { let i = v as i32; if v < i as f32 { i.saturating_sub(1) } else { i } };
            let (l,t,r,b) = (ceil(l) as f32, ceil(t) as f32, floor(r) as f32, floor(b) as f32);
            // No whole pixel fits; retaining a live particle would violate the box.
            if r-l < 1.0 || b-t < 1.0 { p.life = 0.0; return; }
            p.size = p.size.max(0.0).min(r-l).min(b-t);
            let half = p.size.max(1.0)/2.0;
            let x = p.x.clamp(l+half,r-half);
            let y = p.y.clamp(t+half,b-half);
            if x != p.x { p.vx = 0.0; }
            if y != p.y { p.vy = 0.0; }
            p.x = x; p.y = y;
        }
    }
    /// Inspect particles without invalidating the pool's lifetime accounting.
    pub fn particles(&self) -> &[Particle] { &self.particles }

    /// Emit a particle. Reuse a dead slot, or the oldest entry at capacity.
    pub fn emit(&mut self, mut p: Particle) {
        if !p.max_life.is_finite() || p.max_life <= 0.0 || !p.x.is_finite() || !p.y.is_finite()
            || !p.vx.is_finite() || !p.vy.is_finite() || !p.size.is_finite()
            || !p.gravity.is_finite() || !p.friction.is_finite() || !p.life.is_finite() { return; }
        p.life = p.life.clamp(0.0,1.0);
        Self::confine(self.bounds,&mut p);
        // Try to reuse a dead slot.
        let remaining = p.life as f64 * p.max_life as f64 * 1_000_000_000.0;
        for (i, slot) in self.particles.iter_mut().enumerate() {
            if !slot.alive() {
                *slot = p;
                self.remaining_ns[i] = remaining;
                return;
            }
        }
        // Under the cap — just push.
        if self.particles.len() < self.max_particles {
            self.particles.push(p);
            self.remaining_ns.push(remaining);
        } else if !self.particles.is_empty() {
            // Overwrite the first slot (oldest by insertion order).
            self.particles[0] = p;
            self.remaining_ns[0] = remaining;
        }
    }

    /// Step all particles forward by `dt` seconds.
    pub fn update(&mut self, dt: f32) {
        if !dt.is_finite() || dt <= 0.0 { return; }
        self.remainder_ns = self.remainder_ns.saturating_add((dt as f64 * 1_000_000_000.0 + 0.5) as u64);
        while self.remainder_ns >= 1_000_000 {
            self.remainder_ns -= 1_000_000;
            let mut changed = false;
            for (p, remaining) in self.particles.iter_mut().zip(self.remaining_ns.iter_mut()) {
                let state = |p: &Particle| (p.x.to_bits(), p.y.to_bits(), p.vx.to_bits(), p.vy.to_bits(), p.life.to_bits());
                let before = state(p);
                p.update(0.001);
                *remaining = (*remaining - 1_000_000.0).max(0.0);
                p.life = (*remaining / (p.max_life as f64 * 1_000_000_000.0)) as f32;
                Self::confine(self.bounds,p);
                changed |= before != state(p);
            }
            // Dead particles and exact f32 fixed points cannot change in any
            // later quantum until the caller emits or mutates a particle.
            if !changed {
                let skipped = (self.remainder_ns / 1_000_000 * 1_000_000) as f64;
                for (p, remaining) in self.particles.iter_mut().zip(self.remaining_ns.iter_mut()) {
                    *remaining = (*remaining - skipped).max(0.0);
                    if p.alive() { p.life = (*remaining / (p.max_life as f64 * 1_000_000_000.0)) as f32; }
                }
                self.remainder_ns %= 1_000_000;
                break;
            }
        }
    }

    /// Draw all live particles.
    pub fn draw(&self, fb: &mut FrameBuf, offset_x: i32, offset_y: i32) {
        // Frames are drawn often: their shapes go to the GPU together (Surface::defer_shapes).
        fb.defer_shapes();
        if let Some((l,t,r,b)) = self.bounds {
            let ceil = |v: f32| { let i = v as i32; if v > i as f32 { i.saturating_add(1) } else { i } };
            let floor = |v: f32| { let i = v as i32; if v < i as f32 { i.saturating_sub(1) } else { i } };
            let left = ceil(l).saturating_add(offset_x);
            let top = ceil(t).saturating_add(offset_y);
            let right = floor(r).saturating_add(offset_x);
            let bottom = floor(b).saturating_add(offset_y);
            fb.push_clip(libgfx::surface::Rect::new(left, top, right.saturating_sub(left).max(0), bottom.saturating_sub(top).max(0)));
        }
        for p in &self.particles { p.draw(fb, offset_x, offset_y); }
        if self.bounds.is_some() { fb.pop_clip(); }
    }

    /// Kill all particles immediately.
    pub fn clear(&mut self) {
        self.remainder_ns = 0;
        for p in self.particles.iter_mut() {
            p.life = 0.0;
        }
        self.remaining_ns.fill(0.0);
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

//! Particle physics system with collision detection
//!
//! Uses fixed-point math (scale factor 256) to avoid floating-point operations.
//! Provides bouncing particles with gravity, damping, and particle-particle collisions.

use super::arm64_fb;
use super::primitives::{fill_circle, fill_rect, Canvas, Color, Rect};
use alloc::vec::Vec;
use spin::{Mutex, Once};

/// Global particle system for the animation thread
pub static PARTICLE_SYSTEM: Once<Mutex<ParticleSystem>> = Once::new();

/// Initialize and start the particle animation
pub fn start_animation(left: i32, top: i32, right: i32, bottom: i32) {
    PARTICLE_SYSTEM.call_once(|| {
        let mut system = ParticleSystem::new(left, top, right, bottom);
        system.spawn_demo_particles();
        Mutex::new(system)
    });
}

/// Animation thread entry point - runs the particle physics loop
///
/// CRITICAL: No logging allowed here! Logger locks can deadlock with timer interrupts.
/// Use raw_serial_char() for debugging only.
///
/// NOTE: Do NOT call yield_current() here - it can corrupt scheduler state during boot.
/// Timer interrupts will naturally preempt this thread.
pub fn animation_thread_entry() {
    // Raw serial output - no locks, safe in any context
    fn raw_char(c: u8) {
        const HHDM_BASE: u64 = 0xFFFF_0000_0000_0000;
        const PL011_BASE: u64 = 0x0900_0000;
        let addr = (HHDM_BASE + PL011_BASE) as *mut u32;
        unsafe { core::ptr::write_volatile(addr, c as u32); }
    }

    raw_char(b'<'); // Thread entry point reached

    // Verify systems are initialized
    if PARTICLE_SYSTEM.get().is_none() {
        raw_char(b'!');
        return;
    }
    if arm64_fb::SHELL_FRAMEBUFFER.get().is_none() {
        raw_char(b'?');
        return;
    }

    raw_char(b'>'); // Systems OK, entering loop

    let mut frame_counter: u64 = 0;

    loop {
        frame_counter = frame_counter.wrapping_add(1);

        // Update physics (if we can get the lock)
        if let Some(system) = PARTICLE_SYSTEM.get() {
            if let Some(mut sys) = system.try_lock() {
                sys.update();
            }
        }

        // Render to framebuffer (if we can get the lock)
        if let Some(fb) = arm64_fb::SHELL_FRAMEBUFFER.get() {
            if let Some(mut fb_guard) = fb.try_lock() {
                if let Some(system) = PARTICLE_SYSTEM.get() {
                    if let Some(sys) = system.try_lock() {
                        sys.render(&mut *fb_guard);
                    }
                }
                fb_guard.flush();
            }
        }

        // Progress marker every 500 frames
        if frame_counter % 500 == 0 {
            raw_char(b'*');
        }

        // Spin delay - timer interrupts will preempt us naturally
        // ~200k iterations at 1GHz ≈ 200µs per frame ≈ 5000 fps max
        // (but we'll be preempted by timer every 5ms)
        for _ in 0..200_000 {
            core::hint::spin_loop();
        }
    }
}

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
pub struct Particle {
    /// X position in fixed-point
    pub x: i32,
    /// Y position in fixed-point
    pub y: i32,
    /// X velocity in fixed-point
    pub vx: i32,
    /// Y velocity in fixed-point
    pub vy: i32,
    /// Radius in pixels
    pub radius: i32,
    /// Color
    pub color: Color,
    /// Mass (affects collision response)
    pub mass: i32,
    /// Trail intensity (0-255, for glow effect)
    pub trail: u8,
}

impl Particle {
    /// Create a new particle at the given position
    pub fn new(x: i32, y: i32, radius: i32, color: Color) -> Self {
        Self {
            x: to_fp(x),
            y: to_fp(y),
            vx: 0,
            vy: 0,
            radius,
            color,
            mass: radius, // Mass proportional to radius
            trail: 0,
        }
    }

    /// Set velocity
    pub fn with_velocity(mut self, vx: i32, vy: i32) -> Self {
        self.vx = vx;
        self.vy = vy;
        self
    }

    /// Get pixel X position
    pub fn px(&self) -> i32 {
        from_fp(self.x)
    }

    /// Get pixel Y position
    pub fn py(&self) -> i32 {
        from_fp(self.y)
    }
}

/// Particle system configuration
pub struct ParticleConfig {
    /// Gravity (positive = down) in fixed-point units per frame^2
    pub gravity: i32,
    /// Damping factor (256 = no damping, 250 = slight damping)
    pub damping: i32,
    /// Coefficient of restitution for collisions (256 = perfectly elastic)
    pub restitution: i32,
    /// Minimum velocity threshold (below this, particle stops)
    pub min_velocity: i32,
}

impl Default for ParticleConfig {
    fn default() -> Self {
        Self {
            gravity: 25,            // Gentle gravity
            damping: 253,           // Slight air resistance
            restitution: 230,       // ~90% energy retained on bounce
            min_velocity: 5,        // Stop jittering at low velocity
        }
    }
}

/// Particle system managing multiple particles
pub struct ParticleSystem {
    /// All particles
    pub particles: Vec<Particle>,
    /// Bounding box (left, top, right, bottom) in pixels
    pub bounds: (i32, i32, i32, i32),
    /// Physics configuration
    pub config: ParticleConfig,
    /// Frame counter for animations
    pub frame: u64,
    /// Background color
    pub bg_color: Color,
}

impl ParticleSystem {
    /// Create a new particle system with given bounds
    pub fn new(left: i32, top: i32, right: i32, bottom: i32) -> Self {
        Self {
            particles: Vec::new(),
            bounds: (left, top, right, bottom),
            config: ParticleConfig::default(),
            frame: 0,
            bg_color: Color::rgb(15, 20, 35),
        }
    }

    /// Add a particle to the system
    pub fn add(&mut self, particle: Particle) {
        self.particles.push(particle);
    }

    /// Update physics for one frame
    pub fn update(&mut self) {
        self.frame += 1;

        // Phase 1: Apply forces and update velocities
        for p in &mut self.particles {
            // Apply gravity
            p.vy += self.config.gravity;

            // Apply damping
            p.vx = (p.vx * self.config.damping) / FP_SCALE;
            p.vy = (p.vy * self.config.damping) / FP_SCALE;
        }

        // Phase 2: Update positions
        for p in &mut self.particles {
            p.x += p.vx;
            p.y += p.vy;
        }

        // Phase 3: Handle boundary collisions
        self.handle_boundary_collisions();

        // Phase 4: Handle particle-particle collisions
        self.handle_particle_collisions();

        // Phase 5: Update trail effects
        for p in &mut self.particles {
            // Trail intensity based on velocity
            let speed = isqrt((p.vx * p.vx + p.vy * p.vy) as u32);
            p.trail = (speed.min(255) as u8).saturating_mul(2).min(255);
        }
    }

    /// Handle collisions with boundaries
    fn handle_boundary_collisions(&mut self) {
        let (left, top, right, bottom) = self.bounds;

        for p in &mut self.particles {
            let r = to_fp(p.radius);
            let min_x = to_fp(left) + r;
            let max_x = to_fp(right) - r;
            let min_y = to_fp(top) + r;
            let max_y = to_fp(bottom) - r;

            // Left boundary
            if p.x < min_x {
                p.x = min_x;
                p.vx = (-p.vx * self.config.restitution) / FP_SCALE;
            }
            // Right boundary
            if p.x > max_x {
                p.x = max_x;
                p.vx = (-p.vx * self.config.restitution) / FP_SCALE;
            }
            // Top boundary
            if p.y < min_y {
                p.y = min_y;
                p.vy = (-p.vy * self.config.restitution) / FP_SCALE;
            }
            // Bottom boundary
            if p.y > max_y {
                p.y = max_y;
                p.vy = (-p.vy * self.config.restitution) / FP_SCALE;

                // Stop jittering at bottom
                if p.vy.abs() < self.config.min_velocity {
                    p.vy = 0;
                }
            }
        }
    }

    /// Handle particle-particle collisions with elastic response
    fn handle_particle_collisions(&mut self) {
        let n = self.particles.len();
        if n < 2 {
            return;
        }

        // Check all pairs
        for i in 0..n {
            for j in (i + 1)..n {
                // Get particles
                let (p1, p2) = unsafe {
                    let ptr = self.particles.as_mut_ptr();
                    (&mut *ptr.add(i), &mut *ptr.add(j))
                };

                // Calculate distance between centers
                let dx = p2.x - p1.x;
                let dy = p2.y - p1.y;
                let dist_sq = (dx / 16) * (dx / 16) + (dy / 16) * (dy / 16); // Scale down to avoid overflow

                // Minimum distance (sum of radii)
                let min_dist = to_fp(p1.radius + p2.radius) / 16;
                let min_dist_sq = min_dist * min_dist;

                if dist_sq < min_dist_sq && dist_sq > 0 {
                    // Collision detected!
                    let dist = isqrt(dist_sq as u32) as i32 * 16;
                    if dist == 0 {
                        continue;
                    }

                    // Normalized collision vector
                    let nx = (dx * FP_SCALE) / dist;
                    let ny = (dy * FP_SCALE) / dist;

                    // Relative velocity
                    let dvx = p1.vx - p2.vx;
                    let dvy = p1.vy - p2.vy;

                    // Relative velocity along collision normal
                    let dvn = (dvx * nx + dvy * ny) / FP_SCALE;

                    // Only resolve if particles are approaching
                    if dvn > 0 {
                        // Calculate impulse with restitution
                        let total_mass = p1.mass + p2.mass;
                        let impulse = (dvn * self.config.restitution * 2) / (FP_SCALE * total_mass / p1.mass);

                        // Apply impulse
                        let impulse1 = (impulse * p2.mass) / total_mass;
                        let impulse2 = (impulse * p1.mass) / total_mass;

                        p1.vx -= (impulse1 * nx) / FP_SCALE;
                        p1.vy -= (impulse1 * ny) / FP_SCALE;
                        p2.vx += (impulse2 * nx) / FP_SCALE;
                        p2.vy += (impulse2 * ny) / FP_SCALE;

                        // Separate overlapping particles
                        let overlap = to_fp(p1.radius + p2.radius) - dist;
                        if overlap > 0 {
                            let sep = overlap / 2 + FP_SCALE;
                            p1.x -= (sep * nx) / FP_SCALE;
                            p1.y -= (sep * ny) / FP_SCALE;
                            p2.x += (sep * nx) / FP_SCALE;
                            p2.y += (sep * ny) / FP_SCALE;
                        }
                    }
                }
            }
        }
    }

    /// Render all particles to a canvas
    pub fn render(&self, canvas: &mut impl Canvas) {
        let (left, top, right, bottom) = self.bounds;

        // Clear background
        fill_rect(
            canvas,
            Rect {
                x: left,
                y: top,
                width: (right - left) as u32,
                height: (bottom - top) as u32,
            },
            self.bg_color,
        );

        // Draw particles with glow effect
        for p in &self.particles {
            let px = p.px();
            let py = p.py();

            // Draw glow (larger, dimmer circle)
            if p.trail > 30 {
                let glow_radius = p.radius + 3;
                let glow_intensity = p.trail / 4;
                let glow_color = Color::rgb(
                    (p.color.r as u16 * glow_intensity as u16 / 255) as u8,
                    (p.color.g as u16 * glow_intensity as u16 / 255) as u8,
                    (p.color.b as u16 * glow_intensity as u16 / 255) as u8,
                );
                fill_circle(canvas, px, py, glow_radius as u32, glow_color);
            }

            // Draw main particle
            fill_circle(canvas, px, py, p.radius as u32, p.color);

            // Draw highlight (small white dot for 3D effect)
            if p.radius > 6 {
                let highlight_x = px - p.radius / 3;
                let highlight_y = py - p.radius / 3;
                let highlight_r = (p.radius / 4).max(2);
                fill_circle(
                    canvas,
                    highlight_x,
                    highlight_y,
                    highlight_r as u32,
                    Color::rgb(255, 255, 255),
                );
            }
        }

        // Draw frame counter in corner (subtle)
        // (Skip for now - would need text rendering)
    }

    /// Spawn particles in an interesting initial configuration
    pub fn spawn_demo_particles(&mut self) {
        let (left, top, right, bottom) = self.bounds;
        let width = right - left;
        let height = bottom - top;
        let cx = left + width / 2;
        let cy = top + height / 3;

        // Color palette - vibrant colors
        let colors = [
            Color::rgb(255, 100, 100),  // Coral red
            Color::rgb(100, 255, 150),  // Mint green
            Color::rgb(100, 150, 255),  // Sky blue
            Color::rgb(255, 200, 100),  // Golden yellow
            Color::rgb(255, 100, 255),  // Magenta
            Color::rgb(100, 255, 255),  // Cyan
            Color::rgb(255, 150, 100),  // Orange
            Color::rgb(200, 100, 255),  // Purple
        ];

        // Spawn particles in a circular pattern with outward velocity
        let num_particles: i32 = 12;
        for i in 0..num_particles {
            // Calculate angle (using integer approximation of sin/cos)
            let angle_idx = ((i * 256) / num_particles) as usize;
            let (sin_val, cos_val) = sin_cos_table(angle_idx);

            // Position in circle around center
            let radius_offset = 40 + (i % 3) * 20;
            let px = cx + (cos_val * radius_offset) / FP_SCALE;
            let py = cy + (sin_val * radius_offset) / FP_SCALE;

            // Particle size varies
            let particle_radius = 8 + (i % 4) * 4;

            // Outward velocity
            let speed = 300 + (i % 5) * 100;
            let vx = (cos_val * speed) / FP_SCALE;
            let vy = (sin_val * speed) / FP_SCALE - 200; // Bias upward

            let color = colors[(i as usize) % colors.len()];
            let particle = Particle::new(px, py, particle_radius, color)
                .with_velocity(vx, vy);
            self.add(particle);
        }

        // Add a few extra large particles
        self.add(Particle::new(cx - 60, cy - 80, 20, Color::rgb(255, 220, 100))
            .with_velocity(150, -400));
        self.add(Particle::new(cx + 60, cy - 80, 18, Color::rgb(100, 200, 255))
            .with_velocity(-150, -350));
        self.add(Particle::new(cx, cy + 50, 25, Color::rgb(255, 150, 200))
            .with_velocity(0, -500));
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

/// Simple sin/cos lookup table (256 entries = full circle)
/// Returns (sin, cos) in fixed-point (scaled by FP_SCALE)
fn sin_cos_table(angle: usize) -> (i32, i32) {
    // Pre-computed sin values for 0-63 (first quadrant)
    // Scaled by 256 (FP_SCALE)
    const SIN_TABLE: [i32; 64] = [
        0, 6, 13, 19, 25, 31, 37, 44, 50, 56, 62, 68, 74, 80, 86, 92,
        97, 103, 109, 114, 120, 125, 131, 136, 141, 147, 152, 157, 162, 166, 171, 176,
        180, 185, 189, 193, 197, 201, 205, 209, 212, 216, 219, 222, 225, 228, 231, 233,
        236, 238, 240, 242, 244, 246, 247, 249, 250, 251, 252, 253, 254, 254, 255, 255,
    ];

    let angle = angle & 0xFF; // Wrap to 0-255

    // Determine quadrant
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

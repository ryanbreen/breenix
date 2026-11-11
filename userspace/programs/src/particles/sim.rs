#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color { pub r: u8, pub g: u8, pub b: u8 }
impl Color { const BLACK: Self = Self::rgb(0, 0, 0); const fn rgb(r: u8, g: u8, b: u8) -> Self { Self { r, g, b } } }
fn isqrt_i64(n: i64) -> i64 { (n as f64).sqrt() as i64 }

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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Particle {
    pub x: i32,
    pub y: i32,
    pub vx: i32,
    pub vy: i32,
    pub radius: i32,
    pub color: Color,
    mass: i32,
    trail: u8,
}

impl Particle {
    fn new(x: i32, y: i32, radius: i32, color: Color) -> Self {
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

    pub fn px(&self) -> i32 {
        from_fp(self.x)
    }

    pub fn py(&self) -> i32 {
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
}

impl ParticleSystem {
    fn new(left: i32, top: i32, right: i32, bottom: i32) -> Self {
        Self {
            particles: [Particle::new(0, 0, 0, Color::BLACK); MAX_PARTICLES],
            count: 0,
            bounds: (left, top, right, bottom),
            config: Config::default_config(),
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
        self.handle_boundary_collisions();

        // Phase 5: Update trail effects
        for i in 0..self.count {
            let p = &mut self.particles[i];
            let speed_sq = (p.vx as i64) * (p.vx as i64) + (p.vy as i64) * (p.vy as i64);
            let speed = isqrt_i64(speed_sq) as u32;
            p.trail = (speed.min(255) as u8).saturating_mul(2).min(255);
        }
    }

    fn handle_boundary_collisions(&mut self) {
        let (left, top, right, bottom) = self.bounds;

        for i in 0..self.count {
            let p = &mut self.particles[i];
            let r = to_fp(p.radius);
            let min_x = to_fp(left) + r;
            let max_x = to_fp(right - 1) - r;
            let min_y = to_fp(top) + r;
            let max_y = to_fp(bottom - 1) - r;

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
                    let dist = isqrt_i64(dist_sq as i64) as i32 * 16;
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
                        let impulse = (dvn * self.config.restitution * 2)
                            / (FP_SCALE * total_mass / p1_m);

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

    fn spawn_demo_particles(&mut self) {
        let (left, top, right, bottom) = self.bounds;
        let width = right - left;
        let cx = left + width / 2;
        let cy = top + (bottom - top) / 3;

        let colors = [
            Color::rgb(255, 100, 100), // Coral red
            Color::rgb(100, 255, 150), // Mint green
            Color::rgb(100, 150, 255), // Sky blue
            Color::rgb(255, 200, 100), // Golden yellow
            Color::rgb(255, 100, 255), // Magenta
            Color::rgb(100, 255, 255), // Cyan
            Color::rgb(255, 150, 100), // Orange
            Color::rgb(200, 100, 255), // Purple
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
            Particle::new(cx - 60, cy - 80, 20, Color::rgb(255, 220, 100))
                .with_velocity(150, -400),
        );
        self.add(
            Particle::new(cx + 60, cy - 80, 18, Color::rgb(100, 200, 255))
                .with_velocity(-150, -350),
        );
    }
}

/// Simple sin/cos lookup table (256 entries = full circle)
fn sin_cos_table(angle: usize) -> (i32, i32) {
    const SIN_TABLE: [i32; 64] = [
        0, 6, 13, 19, 25, 31, 37, 44, 50, 56, 62, 68, 74, 80, 86, 92, 97, 103, 109, 114, 120,
        125, 131, 136, 141, 147, 152, 157, 162, 166, 171, 176, 180, 185, 189, 193, 197, 201, 205,
        209, 212, 216, 219, 222, 225, 228, 231, 233, 236, 238, 240, 242, 244, 246, 247, 249, 250,
        251, 252, 253, 254, 254, 255, 255,
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


pub struct Simulation {
    system: ParticleSystem,
    pub particles: Vec<Particle>,
    remainder_ms: u64, epochs: u64, bounded: bool,
}
impl Simulation {
    pub fn new(width: i32, height: i32) -> Self {
        let mut system = ParticleSystem::new(0, 0, width, height);
        system.spawn_demo_particles();
        let mut sim = Self { system, particles: Vec::new(), remainder_ms: 0, epochs: 0, bounded: true };
        sim.resize(width, height); sim
    }
    fn sync(&mut self) { self.particles = self.system.particles[..self.system.count].to_vec(); }
    pub fn resize(&mut self, width: i32, height: i32) {
        self.system.bounds = (0, 0, width, height);
        for p in &mut self.system.particles[..self.system.count] { p.radius = p.radius.min((width.min(height).saturating_sub(1) / 2).max(0)); }
        self.system.handle_boundary_collisions(); self.sync();
    }
    pub fn advance(&mut self, elapsed_ms: u64) {
        let elapsed = self.remainder_ms.saturating_add(elapsed_ms);
        let epochs = (elapsed / 16).min(1000); self.remainder_ms = elapsed % 16;
        self.epochs = self.epochs.saturating_add(epochs);
        for _ in 0..epochs {
            self.system.update();
            let (_, _, w, h) = self.system.bounds;
            self.bounded &= self.system.particles[..self.system.count].iter().all(|p| p.px()-p.radius >= 0 && p.py()-p.radius >= 0 && p.px()+p.radius < w && p.py()+p.radius < h);
        }
        self.sync();
    }
    pub fn all_inside(&self) -> bool { self.bounded }
    pub fn epochs(&self) -> u64 { self.epochs }
}

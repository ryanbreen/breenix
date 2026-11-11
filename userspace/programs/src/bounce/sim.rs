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

pub const NUM_SPHERES: usize = 6;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sphere {
    pub x: i64,   // fixed-point ×1024
    pub y: i64,
    pub vx: i64,  // velocity ×1024
    pub vy: i64,
    pub radius: i32,
    pub mass: i64, // proportional to radius² for 2D collision
    pub color: (u8, u8, u8),
}

impl Sphere {
    fn new(x: i32, y: i32, vx: i32, vy: i32, radius: i32, color: (u8, u8, u8)) -> Self {
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

    pub fn px(&self) -> i32 { (self.x >> 10) as i32 }
    pub fn py(&self) -> i32 { (self.y >> 10) as i32 }

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

    /// Check if a point (in pixel coords) is inside this sphere.
    pub fn hit_test(&self, px: i32, py: i32) -> bool {
        let dx = px - self.px();
        let dy = py - self.py();
        dx * dx + dy * dy <= self.radius * self.radius
    }

    /// Impel: boost velocity by 50% in current direction of travel.
    /// If nearly stationary, give a random-ish kick based on position.
    pub fn impel(&mut self) {
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

fn make_spheres(w: i32, h: i32) -> [Sphere; NUM_SPHERES] {
    // Spread spheres across the area, scaling radii to fit
    let scale_x = w as f32 / 400.0;
    let scale_y = h as f32 / 300.0;
    let scale = if scale_x < scale_y { scale_x } else { scale_y };
    let r = |base: i32| -> i32 { ((base as f32 * scale) as i32).max(10) };

    [
        Sphere::new((50.0 * scale_x) as i32,  (40.0 * scale_y) as i32,   300,  250, r(30), (220, 50, 50)),   // Red
        Sphere::new((200.0 * scale_x) as i32, (120.0 * scale_y) as i32, -280,  200, r(25), (50, 200, 50)),   // Green
        Sphere::new((100.0 * scale_x) as i32, (200.0 * scale_y) as i32,  250, -300, r(35), (60, 80, 255)),   // Blue
        Sphere::new((300.0 * scale_x) as i32, (80.0 * scale_y) as i32,  -220, -250, r(20), (240, 220, 50)),  // Yellow
        Sphere::new((150.0 * scale_x) as i32, (250.0 * scale_y) as i32,  280,  180, r(28), (220, 60, 220)),  // Magenta
        Sphere::new((320.0 * scale_x) as i32, (180.0 * scale_y) as i32, -200,  270, r(22), (50, 220, 220)),  // Cyan
    ]
}

/// Run one physics frame: substep movement, wall bounce, sphere-sphere collision.
pub fn physics_step(spheres: &mut [Sphere; NUM_SPHERES], w: i32, h: i32, dt_scale: i64) {
    const SUBSTEPS: i32 = 8;
    for _ in 0..SUBSTEPS {
        for s in spheres.iter_mut() {
            s.step_scaled(SUBSTEPS, dt_scale);
        }
        for s in spheres.iter_mut() {
            s.bounce_walls(w, h);
        }
        // Sphere-sphere collisions
        for i in 0..NUM_SPHERES {
            for j in (i + 1)..NUM_SPHERES {
                let (left, right) = spheres.split_at_mut(j);
                collide_spheres(&mut left[i], &mut right[0]);
            }
        }
        for s in spheres.iter_mut() { s.bounce_walls(w, h); }
    }
}

pub struct Simulation {
    pub spheres: [Sphere; NUM_SPHERES],
    width: i32,
    height: i32,
    speed: i64,
    remainder_ms: u64,
    bounded: bool,
    epochs: u64,
}

impl Simulation {
    pub fn new(width: i32, height: i32) -> Self {
        Self { spheres: make_spheres(width, height), width, height, speed: 10_000, remainder_ms: 0, bounded: true, epochs: 0 }
    }
    pub fn speed_pct(&self) -> i32 { (self.speed / 100) as i32 }
    pub fn change_speed(&mut self, faster: bool) {
        let next = if faster { (self.speed * 23 / 20).min(40_000) } else { (self.speed * 20 / 23).max(2_500) };
        if next != self.speed {
            for sphere in &mut self.spheres {
                sphere.vx = sphere.vx * next / self.speed;
                sphere.vy = sphere.vy * next / self.speed;
            }
            self.speed = next;
        }
    }
    pub fn click(&mut self, x: i32, y: i32) -> bool {
        if let Some(sphere) = self.spheres.iter_mut().find(|s| s.hit_test(x, y)) {
            sphere.impel();
            true
        } else {
            for sphere in &mut self.spheres { sphere.impel(); }
            false
        }
    }
    pub fn resize(&mut self, width: i32, height: i32) {
        self.width = width;
        self.height = height;
        for s in &mut self.spheres { s.bounce_walls(width, height); }
    }
    pub fn advance(&mut self, elapsed_ms: u64) {
        let elapsed = self.remainder_ms.saturating_add(elapsed_ms);
        let epochs = (elapsed / 16).min(1000);
        self.remainder_ms = elapsed % 16;
        self.epochs = self.epochs.saturating_add(epochs);
        for _ in 0..epochs {
            physics_step(&mut self.spheres, self.width, self.height, 1024);
            self.bounded &= self.spheres.iter().all(|s| s.px() - s.radius >= 0 && s.py() - s.radius >= 0 && s.px() + s.radius < self.width && s.py() + s.radius < self.height);
        }
    }
    pub fn epochs(&self) -> u64 { self.epochs }
    pub fn all_inside(&self) -> bool { self.bounded }
}

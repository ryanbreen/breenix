//! Bouncing balls with collision detection demo for Breenix
//!
//! Balls bounce off walls and each other with elastic collisions.
//! Created for Gus!

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::graphics::{fb_clear, fb_fill_circle, fb_flush, fbinfo, rgb};
use libbreenix::io::println;
use libbreenix::process::exit;
use libbreenix::time::sleep_ms;

/// Ball state
struct Ball {
    x: i32,       // Position (fixed point, scaled by 100)
    y: i32,
    vx: i32,      // Velocity (fixed point, scaled by 100)
    vy: i32,
    radius: i32,  // Actual radius in pixels
    color: u32,
    mass: i32,    // For collision response (proportional to radius)
}

impl Ball {
    fn new(x: i32, y: i32, vx: i32, vy: i32, radius: i32, color: u32) -> Self {
        Self {
            x: x * 100,  // Scale up for fixed-point math
            y: y * 100,
            vx,
            vy,
            radius,
            color,
            mass: radius,  // Mass proportional to radius
        }
    }

    /// Get pixel position
    fn px(&self) -> i32 {
        self.x / 100
    }

    fn py(&self) -> i32 {
        self.y / 100
    }

    /// Update position based on velocity
    fn update_position(&mut self) {
        self.x += self.vx;
        self.y += self.vy;
    }

    /// Bounce off walls
    fn bounce_walls(&mut self, width: i32, height: i32) {
        let px = self.px();
        let py = self.py();

        // Left wall
        if px - self.radius < 0 {
            self.x = self.radius * 100;
            self.vx = -self.vx;
        }
        // Right wall
        if px + self.radius >= width {
            self.x = (width - self.radius - 1) * 100;
            self.vx = -self.vx;
        }
        // Top wall
        if py - self.radius < 0 {
            self.y = self.radius * 100;
            self.vy = -self.vy;
        }
        // Bottom wall
        if py + self.radius >= height {
            self.y = (height - self.radius - 1) * 100;
            self.vy = -self.vy;
        }
    }

    fn draw(&self) {
        let _ = fb_fill_circle(self.px(), self.py(), self.radius, self.color);
    }
}

/// Integer square root (for collision detection)
fn isqrt(n: i32) -> i32 {
    if n < 0 {
        return 0;
    }
    if n < 2 {
        return n;
    }

    let mut x = n;
    let mut y = (x + 1) / 2;

    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

/// Check if two balls are colliding and handle the collision
fn check_collision(ball1: &mut Ball, ball2: &mut Ball) {
    // Calculate distance between centers
    let dx = ball2.px() - ball1.px();
    let dy = ball2.py() - ball1.py();
    let dist_sq = dx * dx + dy * dy;

    let min_dist = ball1.radius + ball2.radius;
    let min_dist_sq = min_dist * min_dist;

    // Check if colliding
    if dist_sq < min_dist_sq && dist_sq > 0 {
        let dist = isqrt(dist_sq);
        if dist == 0 {
            return;
        }

        // Normalize collision vector (scaled by 1000 for fixed-point)
        let nx = (dx * 1000) / dist;
        let ny = (dy * 1000) / dist;

        // Relative velocity
        let dvx = ball1.vx - ball2.vx;
        let dvy = ball1.vy - ball2.vy;

        // Relative velocity along collision normal (scaled)
        let dvn = (dvx * nx + dvy * ny) / 1000;

        // Don't resolve if balls are moving apart
        if dvn > 0 {
            return;
        }

        // Calculate impulse (simplified elastic collision)
        let total_mass = ball1.mass + ball2.mass;
        let impulse1 = (2 * ball2.mass * dvn) / total_mass;
        let impulse2 = (2 * ball1.mass * dvn) / total_mass;

        // Apply impulse
        ball1.vx -= (impulse1 * nx) / 1000;
        ball1.vy -= (impulse1 * ny) / 1000;
        ball2.vx += (impulse2 * nx) / 1000;
        ball2.vy += (impulse2 * ny) / 1000;

        // Separate balls to prevent sticking
        let overlap = min_dist - dist;
        if overlap > 0 {
            let sep = (overlap * 100) / 2 + 50; // Half overlap + small buffer, scaled
            ball1.x -= (sep * nx) / 1000;
            ball1.y -= (sep * ny) / 1000;
            ball2.x += (sep * nx) / 1000;
            ball2.y += (sep * ny) / 1000;
        }
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    println("Bounce demo starting (for Gus!)");

    // Get framebuffer info
    let info = match fbinfo() {
        Ok(info) => info,
        Err(e) => {
            println("Error: Could not get framebuffer info");
            exit(e);
        }
    };

    let width = info.left_pane_width() as i32;
    let height = info.height as i32;

    println("Starting collision demo...");

    // Create balls with different sizes, colors, and velocities
    // Velocities are faster for more action!
    let mut balls = [
        Ball::new(100, 100, 800, 600, 40, rgb(255, 50, 50)),    // Red - large
        Ball::new(300, 200, -700, 500, 35, rgb(50, 255, 50)),   // Green
        Ball::new(200, 400, 600, -700, 45, rgb(50, 50, 255)),   // Blue - largest
        Ball::new(400, 300, -500, -600, 30, rgb(255, 255, 50)), // Yellow
        Ball::new(150, 300, 750, 400, 25, rgb(255, 50, 255)),   // Magenta - small
        Ball::new(350, 150, -650, 550, 28, rgb(50, 255, 255)),  // Cyan
    ];

    // Animation loop
    loop {
        // Clear to dark background
        let _ = fb_clear(rgb(15, 15, 30));

        // Update positions
        for ball in balls.iter_mut() {
            ball.update_position();
        }

        // Check wall collisions
        for ball in balls.iter_mut() {
            ball.bounce_walls(width, height);
        }

        // Check ball-ball collisions (all pairs)
        for i in 0..balls.len() {
            for j in (i + 1)..balls.len() {
                // Split the array to get mutable references to both balls
                let (left, right) = balls.split_at_mut(j);
                check_collision(&mut left[i], &mut right[0]);
            }
        }

        // Draw all balls
        for ball in balls.iter() {
            ball.draw();
        }

        // Flush to screen
        let _ = fb_flush();

        // ~30 FPS (reduced for better performance on emulation)
        sleep_ms(33);
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    println("Bounce demo panic!");
    exit(1);
}

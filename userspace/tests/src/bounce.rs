//! Bouncing balls with collision detection demo for Breenix (std version)
//!
//! Balls bounce off walls and each other with elastic collisions.
//! Created for Gus!

use std::process;

/// Framebuffer information structure.
#[repr(C)]
struct FbInfo {
    width: u64,
    height: u64,
    stride: u64,
    bytes_per_pixel: u64,
    pixel_format: u64,
}

impl FbInfo {
    fn zeroed() -> Self {
        Self {
            width: 0,
            height: 0,
            stride: 0,
            bytes_per_pixel: 0,
            pixel_format: 0,
        }
    }

    fn left_pane_width(&self) -> u64 {
        self.width / 2
    }
}

/// Draw command structure for sys_fbdraw.
#[repr(C)]
struct FbDrawCmd {
    op: u32,
    p1: i32,
    p2: i32,
    p3: i32,
    p4: i32,
    color: u32,
}

/// Draw operation codes
mod draw_op {
    pub const CLEAR: u32 = 0;
    pub const FILL_CIRCLE: u32 = 3;
    pub const FLUSH: u32 = 6;
}

/// Syscall numbers
const SYS_FBINFO: u64 = 410;
const SYS_FBDRAW: u64 = 411;

/// Raw syscall1
#[cfg(target_arch = "x86_64")]
unsafe fn syscall1(num: u64, arg1: u64) -> u64 {
    let ret: u64;
    core::arch::asm!(
        "int 0x80",
        in("rax") num,
        in("rdi") arg1,
        lateout("rax") ret,
        options(nostack, preserves_flags),
    );
    ret
}

#[cfg(target_arch = "aarch64")]
unsafe fn syscall1(num: u64, arg1: u64) -> u64 {
    let ret: u64;
    core::arch::asm!(
        "svc #0",
        in("x8") num,
        inlateout("x0") arg1 => ret,
        options(nostack),
    );
    ret
}

/// Pack RGB color into u32
#[inline]
const fn rgb(r: u8, g: u8, b: u8) -> u32 {
    ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
}

/// Get framebuffer information
fn fbinfo() -> Result<FbInfo, i32> {
    let mut info = FbInfo::zeroed();
    let result = unsafe { syscall1(SYS_FBINFO, &mut info as *mut FbInfo as u64) };
    if (result as i64) < 0 {
        Err(-(result as i64) as i32)
    } else {
        Ok(info)
    }
}

/// Execute a draw command
fn fbdraw(cmd: &FbDrawCmd) -> Result<(), i32> {
    let result = unsafe { syscall1(SYS_FBDRAW, cmd as *const FbDrawCmd as u64) };
    if (result as i64) < 0 {
        Err(-(result as i64) as i32)
    } else {
        Ok(())
    }
}

fn fb_clear(color: u32) -> Result<(), i32> {
    fbdraw(&FbDrawCmd { op: draw_op::CLEAR, p1: 0, p2: 0, p3: 0, p4: 0, color })
}

fn fb_fill_circle(cx: i32, cy: i32, radius: i32, color: u32) -> Result<(), i32> {
    fbdraw(&FbDrawCmd { op: draw_op::FILL_CIRCLE, p1: cx, p2: cy, p3: radius, p4: 0, color })
}

fn fb_flush() -> Result<(), i32> {
    fbdraw(&FbDrawCmd { op: draw_op::FLUSH, p1: 0, p2: 0, p3: 0, p4: 0, color: 0 })
}

extern "C" {
    fn sleep_ms(ms: u64);
}

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

        if px - self.radius < 0 {
            self.x = self.radius * 100;
            self.vx = -self.vx;
        }
        if px + self.radius >= width {
            self.x = (width - self.radius - 1) * 100;
            self.vx = -self.vx;
        }
        if py - self.radius < 0 {
            self.y = self.radius * 100;
            self.vy = -self.vy;
        }
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
    let dx = ball2.px() - ball1.px();
    let dy = ball2.py() - ball1.py();
    let dist_sq = dx * dx + dy * dy;

    let min_dist = ball1.radius + ball2.radius;
    let min_dist_sq = min_dist * min_dist;

    if dist_sq < min_dist_sq && dist_sq > 0 {
        let dist = isqrt(dist_sq);
        if dist == 0 {
            return;
        }

        let nx = (dx * 1000) / dist;
        let ny = (dy * 1000) / dist;

        let dvx = ball1.vx - ball2.vx;
        let dvy = ball1.vy - ball2.vy;

        let dvn = (dvx * nx + dvy * ny) / 1000;

        if dvn > 0 {
            return;
        }

        let total_mass = ball1.mass + ball2.mass;
        let impulse1 = (2 * ball2.mass * dvn) / total_mass;
        let impulse2 = (2 * ball1.mass * dvn) / total_mass;

        ball1.vx -= (impulse1 * nx) / 1000;
        ball1.vy -= (impulse1 * ny) / 1000;
        ball2.vx += (impulse2 * nx) / 1000;
        ball2.vy += (impulse2 * ny) / 1000;

        let overlap = min_dist - dist;
        if overlap > 0 {
            let sep = (overlap * 100) / 2 + 50;
            ball1.x -= (sep * nx) / 1000;
            ball1.y -= (sep * ny) / 1000;
            ball2.x += (sep * nx) / 1000;
            ball2.y += (sep * ny) / 1000;
        }
    }
}

fn main() {
    println!("Bounce demo starting (for Gus!)");

    // Get framebuffer info
    let info = match fbinfo() {
        Ok(info) => info,
        Err(e) => {
            println!("Error: Could not get framebuffer info");
            process::exit(e);
        }
    };

    let width = info.left_pane_width() as i32;
    let height = info.height as i32;

    println!("Starting collision demo...");

    // Create balls with different sizes, colors, and velocities
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
        unsafe { sleep_ms(33); }
    }
}

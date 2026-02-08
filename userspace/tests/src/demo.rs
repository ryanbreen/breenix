//! Animated graphics demo for Breenix (std version)
//!
//! This program draws animated graphics on the left pane of the screen.
//! Run it from the shell with: demo

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
    pub const FILL_RECT: u32 = 1;
    pub const DRAW_RECT: u32 = 2;
    pub const FILL_CIRCLE: u32 = 3;
    pub const DRAW_LINE: u32 = 5;
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

fn fb_fill_rect(x: i32, y: i32, width: i32, height: i32, color: u32) -> Result<(), i32> {
    fbdraw(&FbDrawCmd { op: draw_op::FILL_RECT, p1: x, p2: y, p3: width, p4: height, color })
}

fn fb_draw_rect(x: i32, y: i32, width: i32, height: i32, color: u32) -> Result<(), i32> {
    fbdraw(&FbDrawCmd { op: draw_op::DRAW_RECT, p1: x, p2: y, p3: width, p4: height, color })
}

fn fb_fill_circle(cx: i32, cy: i32, radius: i32, color: u32) -> Result<(), i32> {
    fbdraw(&FbDrawCmd { op: draw_op::FILL_CIRCLE, p1: cx, p2: cy, p3: radius, p4: 0, color })
}

fn fb_draw_line(x1: i32, y1: i32, x2: i32, y2: i32, color: u32) -> Result<(), i32> {
    fbdraw(&FbDrawCmd { op: draw_op::DRAW_LINE, p1: x1, p2: y1, p3: x2, p4: y2, color })
}

fn fb_flush() -> Result<(), i32> {
    fbdraw(&FbDrawCmd { op: draw_op::FLUSH, p1: 0, p2: 0, p3: 0, p4: 0, color: 0 })
}

extern "C" {
    fn sleep_ms(ms: u64);
}

/// Pre-computed sine table (0-359 degrees, scaled by 1000)
const SIN_TABLE: [i32; 360] = [
    0, 17, 35, 52, 70, 87, 105, 122, 139, 156, 174, 191, 208, 225, 242, 259, 276, 292, 309, 326,
    342, 358, 375, 391, 407, 423, 438, 454, 469, 485, 500, 515, 530, 545, 559, 574, 588, 602, 616,
    629, 643, 656, 669, 682, 695, 707, 719, 731, 743, 755, 766, 777, 788, 799, 809, 819, 829, 839,
    848, 857, 866, 875, 883, 891, 899, 906, 914, 921, 927, 934, 940, 946, 951, 956, 961, 966, 970,
    974, 978, 982, 985, 988, 990, 993, 995, 996, 998, 999, 999, 1000, 1000, 1000, 999, 999, 998,
    996, 995, 993, 990, 988, 985, 982, 978, 974, 970, 966, 961, 956, 951, 946, 940, 934, 927, 921,
    914, 906, 899, 891, 883, 875, 866, 857, 848, 839, 829, 819, 809, 799, 788, 777, 766, 755, 743,
    731, 719, 707, 695, 682, 669, 656, 643, 629, 616, 602, 588, 574, 559, 545, 530, 515, 500, 485,
    469, 454, 438, 423, 407, 391, 375, 358, 342, 326, 309, 292, 276, 259, 242, 225, 208, 191, 174,
    156, 139, 122, 105, 87, 70, 52, 35, 17, 0, -17, -35, -52, -70, -87, -105, -122, -139, -156,
    -174, -191, -208, -225, -242, -259, -276, -292, -309, -326, -342, -358, -375, -391, -407, -423,
    -438, -454, -469, -485, -500, -515, -530, -545, -559, -574, -588, -602, -616, -629, -643, -656,
    -669, -682, -695, -707, -719, -731, -743, -755, -766, -777, -788, -799, -809, -819, -829, -839,
    -848, -857, -866, -875, -883, -891, -899, -906, -914, -921, -927, -934, -940, -946, -951, -956,
    -961, -966, -970, -974, -978, -982, -985, -988, -990, -993, -995, -996, -998, -999, -999, -1000,
    -1000, -1000, -999, -999, -998, -996, -995, -993, -990, -988, -985, -982, -978, -974, -970,
    -966, -961, -956, -951, -946, -940, -934, -927, -921, -914, -906, -899, -891, -883, -875, -866,
    -857, -848, -839, -829, -819, -809, -799, -788, -777, -766, -755, -743, -731, -719, -707, -695,
    -682, -669, -656, -643, -629, -616, -602, -588, -574, -559, -545, -530, -515, -500, -485, -469,
    -454, -438, -423, -407, -391, -375, -358, -342, -326, -309, -292, -276, -259, -242, -225, -208,
    -191, -174, -156, -139, -122, -105, -87, -70, -52, -35, -17,
];

/// Get sine value (scaled by 1000) for angle in degrees
fn sin(angle: i32) -> i32 {
    let a = ((angle % 360) + 360) % 360;
    SIN_TABLE[a as usize]
}

/// Get cosine value (scaled by 1000) for angle in degrees
fn cos(angle: i32) -> i32 {
    sin(angle + 90)
}

/// Bouncing ball state
struct Ball {
    x: i32,
    y: i32,
    vx: i32,
    vy: i32,
    radius: i32,
    color: u32,
}

impl Ball {
    fn new(x: i32, y: i32, vx: i32, vy: i32, radius: i32, color: u32) -> Self {
        Self { x, y, vx, vy, radius, color }
    }

    fn update(&mut self, width: i32, height: i32) {
        self.x += self.vx;
        self.y += self.vy;

        if self.x - self.radius < 0 {
            self.x = self.radius;
            self.vx = -self.vx;
        }
        if self.x + self.radius >= width {
            self.x = width - self.radius - 1;
            self.vx = -self.vx;
        }
        if self.y - self.radius < 0 {
            self.y = self.radius;
            self.vy = -self.vy;
        }
        if self.y + self.radius >= height {
            self.y = height - self.radius - 1;
            self.vy = -self.vy;
        }
    }

    fn draw(&self) {
        let _ = fb_fill_circle(self.x, self.y, self.radius, self.color);
    }
}

/// Draw rotating lines from center
fn draw_rotating_lines(cx: i32, cy: i32, radius: i32, angle: i32, num_lines: i32) {
    for i in 0..num_lines {
        let a = angle + (i * 360 / num_lines);
        let x2 = cx + (radius * cos(a)) / 1000;
        let y2 = cy + (radius * sin(a)) / 1000;

        let hue = ((a % 360) + 360) % 360;
        let color = hue_to_rgb(hue as u32);
        let _ = fb_draw_line(cx, cy, x2, y2, color);
    }
}

/// Convert hue (0-359) to RGB color
fn hue_to_rgb(hue: u32) -> u32 {
    let h = hue % 360;
    let x = (255 * (60 - (h % 60).min(60 - (h % 60)))) / 60;

    match h / 60 {
        0 => rgb(255, x as u8, 0),
        1 => rgb(x as u8, 255, 0),
        2 => rgb(0, 255, x as u8),
        3 => rgb(0, x as u8, 255),
        4 => rgb(x as u8, 0, 255),
        _ => rgb(255, 0, x as u8),
    }
}

/// Draw pulsing rectangles
fn draw_pulsing_rects(cx: i32, cy: i32, frame: i32) {
    let pulse = (sin(frame * 3) + 1000) / 20;

    for i in 0..5 {
        let size = 20 + i * 15 + pulse / 5;
        let alpha = 255 - i * 40;
        let color = rgb(alpha as u8, (100 + i * 30) as u8, (200 - i * 20) as u8);
        let _ = fb_draw_rect(cx - size, cy - size, size * 2, size * 2, color);
    }
}

/// Draw wave pattern
fn draw_wave(y_base: i32, width: i32, frame: i32, color: u32) {
    let mut prev_y = y_base + (sin(frame) * 30) / 1000;

    let mut x = 0;
    while x < width {
        let phase = frame + x * 2;
        let y = y_base + (sin(phase) * 30) / 1000;
        let _ = fb_draw_line(x - 4, prev_y, x, y, color);
        prev_y = y;
        x += 4;
    }
}

fn main() {
    println!("Breenix Graphics Demo starting...");

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

    println!("Starting animation loop...");

    // Create bouncing balls
    let mut balls = [
        Ball::new(100, 100, 3, 2, 20, rgb(255, 100, 100)),
        Ball::new(200, 150, -2, 3, 15, rgb(100, 255, 100)),
        Ball::new(150, 200, 2, -2, 25, rgb(100, 100, 255)),
        Ball::new(300, 100, -3, -2, 18, rgb(255, 255, 100)),
    ];

    let mut frame = 0i32;
    let center_x = width / 2;
    let center_y = height / 2;

    // Animation loop
    loop {
        // Clear to dark blue
        let _ = fb_clear(rgb(10, 20, 40));

        // Draw rotating lines in center
        draw_rotating_lines(center_x, center_y - 100, 80, frame * 2, 12);

        // Draw pulsing rectangles
        draw_pulsing_rects(center_x, center_y + 150, frame);

        // Draw wave patterns
        draw_wave(height - 100, width, frame * 3, rgb(0, 150, 255));
        draw_wave(height - 130, width, frame * 3 + 60, rgb(0, 200, 150));
        draw_wave(height - 160, width, frame * 3 + 120, rgb(100, 100, 255));

        // Update and draw bouncing balls
        for ball in balls.iter_mut() {
            ball.update(width, height);
            ball.draw();
        }

        // Draw frame counter (simple rectangle indicator)
        let indicator_width = (frame % 100) * 2;
        let _ = fb_fill_rect(10, 10, indicator_width, 5, rgb(255, 255, 255));

        // Flush to screen
        let _ = fb_flush();

        // Small delay for animation timing
        unsafe { sleep_ms(16); } // ~60 FPS

        frame = frame.wrapping_add(1);
    }
}

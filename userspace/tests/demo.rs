//! Animated graphics demo for Breenix
//!
//! This program draws animated graphics on the left pane of the screen.
//! Run it from the shell with: demo

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::graphics::{
    fb_clear, fb_draw_line, fb_draw_rect, fb_fill_circle, fb_fill_rect, fb_flush, fbinfo, rgb,
};
use libbreenix::io::println;
use libbreenix::process::exit;
use libbreenix::time::sleep_ms;

/// Pre-computed sine table (0-359 degrees, scaled by 1000)
/// sin(angle) * 1000
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

        // Bounce off walls
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

        // Color based on angle
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
    let pulse = (sin(frame * 3) + 1000) / 20; // 0-100 range

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

    for x in (0..width).step_by(4) {
        let phase = frame + x * 2;
        let y = y_base + (sin(phase) * 30) / 1000;
        let _ = fb_draw_line(x - 4, prev_y, x, y, color);
        prev_y = y;
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    println("Breenix Graphics Demo starting...");

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

    println("Starting animation loop...");

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
        sleep_ms(16); // ~60 FPS

        frame = frame.wrapping_add(1);
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    println("Demo panic!");
    exit(1);
}

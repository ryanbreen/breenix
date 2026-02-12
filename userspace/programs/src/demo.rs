//! Animated graphics demo for Breenix (std version)
//!
//! Draws rotating lines, bouncing balls, pulsing rectangles, and wave patterns.
//! Uses mmap'd framebuffer for zero-syscall drawing via libgfx.
//! Run it from the shell with: demo

use std::process;

use libbreenix::graphics;
use libbreenix::time;

use libgfx::color::Color;
use libgfx::font;
use libgfx::framebuf::FrameBuf;
use libgfx::shapes;

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
    color: Color,
}

impl Ball {
    fn new(x: i32, y: i32, vx: i32, vy: i32, radius: i32, color: Color) -> Self {
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

    fn draw(&self, fb: &mut FrameBuf) {
        shapes::fill_circle(fb, self.x, self.y, self.radius, self.color);
    }
}

/// Convert hue (0-359) to RGB color
fn hue_to_rgb(hue: u32) -> Color {
    let h = hue % 360;
    let x = (255 * (60 - (h % 60).min(60 - (h % 60)))) / 60;

    match h / 60 {
        0 => Color::rgb(255, x as u8, 0),
        1 => Color::rgb(x as u8, 255, 0),
        2 => Color::rgb(0, 255, x as u8),
        3 => Color::rgb(0, x as u8, 255),
        4 => Color::rgb(x as u8, 0, 255),
        _ => Color::rgb(255, 0, x as u8),
    }
}

/// Draw rotating lines from center
fn draw_rotating_lines(fb: &mut FrameBuf, cx: i32, cy: i32, radius: i32, angle: i32, num_lines: i32) {
    for i in 0..num_lines {
        let a = angle + (i * 360 / num_lines);
        let x2 = cx + (radius * cos(a)) / 1000;
        let y2 = cy + (radius * sin(a)) / 1000;

        let hue = ((a % 360) + 360) % 360;
        let color = hue_to_rgb(hue as u32);
        shapes::draw_line(fb, cx, cy, x2, y2, color);
    }
}

/// Draw pulsing rectangles
fn draw_pulsing_rects(fb: &mut FrameBuf, cx: i32, cy: i32, frame: i32) {
    let pulse = (sin(frame * 3) + 1000) / 20;

    for i in 0..5 {
        let size = 20 + i * 15 + pulse / 5;
        let alpha = 255 - i * 40;
        let color = Color::rgb(alpha as u8, (100 + i * 30) as u8, (200 - i * 20) as u8);
        shapes::draw_rect(fb, cx - size, cy - size, size * 2, size * 2, color);
    }
}

/// Draw wave pattern
fn draw_wave(fb: &mut FrameBuf, y_base: i32, width: i32, frame: i32, color: Color) {
    let mut prev_y = y_base + (sin(frame) * 30) / 1000;

    let mut x = 0;
    while x < width {
        let phase = frame + x * 2;
        let y = y_base + (sin(phase) * 30) / 1000;
        shapes::draw_line(fb, x - 4, prev_y, x, y, color);
        prev_y = y;
        x += 4;
    }
}

// ---------------------------------------------------------------------------
// FPS counter
// ---------------------------------------------------------------------------

fn clock_monotonic_ns() -> u64 {
    let ts = time::now_monotonic().unwrap_or_default();
    (ts.tv_sec as u64) * 1_000_000_000 + (ts.tv_nsec as u64)
}

struct FpsCounter {
    last_time_ns: u64,
    frame_count: u32,
    display_fps: u32,
}

impl FpsCounter {
    fn new() -> Self {
        Self {
            last_time_ns: clock_monotonic_ns(),
            frame_count: 0,
            display_fps: 0,
        }
    }

    fn tick(&mut self) {
        self.frame_count += 1;
        if self.frame_count >= 16 {
            let now = clock_monotonic_ns();
            let elapsed = now.saturating_sub(self.last_time_ns);
            if elapsed > 0 {
                self.display_fps =
                    (self.frame_count as u64 * 1_000_000_000 / elapsed) as u32;
            }
            self.frame_count = 0;
            self.last_time_ns = now;
        }
    }

    fn draw(&self, fb: &mut FrameBuf) {
        let y = fb.height.saturating_sub(20);
        let mut buf = [b' '; 12];
        buf[0] = b'F';
        buf[1] = b'P';
        buf[2] = b'S';
        buf[3] = b':';
        buf[4] = b' ';

        let mut fps = self.display_fps;
        if fps == 0 {
            buf[5] = b'0';
        } else {
            let mut pos = 8;
            while fps > 0 && pos >= 5 {
                buf[pos] = b'0' + (fps % 10) as u8;
                fps /= 10;
                if pos == 0 {
                    break;
                }
                pos -= 1;
            }
        }
        font::draw_text(fb, &buf, 8, y, Color::GRAY, 2);
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

fn main() {
    println!("Breenix Graphics Demo starting...");

    let info = match graphics::fbinfo() {
        Ok(info) => info,
        Err(_e) => {
            println!("Error: Could not get framebuffer info");
            process::exit(1);
        }
    };

    let width = info.left_pane_width() as i32;
    let height = info.height as i32;
    let bpp = info.bytes_per_pixel as usize;

    let fb_ptr = match graphics::fb_mmap() {
        Ok(ptr) => ptr,
        Err(e) => {
            println!("Error: Could not mmap framebuffer ({})", e);
            process::exit(1);
        }
    };

    let mut fb = unsafe {
        FrameBuf::from_raw(
            fb_ptr,
            width as usize,
            height as usize,
            (width as usize) * bpp,
            bpp,
            info.is_bgr(),
        )
    };

    println!("Starting animation loop (mmap mode)...");

    let mut balls = [
        Ball::new(100, 100, 3, 2, 20, Color::rgb(255, 100, 100)),
        Ball::new(200, 150, -2, 3, 15, Color::rgb(100, 255, 100)),
        Ball::new(150, 200, 2, -2, 25, Color::rgb(100, 100, 255)),
        Ball::new(300, 100, -3, -2, 18, Color::rgb(255, 255, 100)),
    ];

    let mut frame = 0i32;
    let center_x = width / 2;
    let center_y = height / 2;
    let bg = Color::rgb(10, 20, 40);

    let mut fps = FpsCounter::new();

    loop {
        fb.clear(bg);

        // Draw rotating lines in center
        draw_rotating_lines(&mut fb, center_x, center_y - 100, 80, frame * 2, 12);

        // Draw pulsing rectangles
        draw_pulsing_rects(&mut fb, center_x, center_y + 150, frame);

        // Draw wave patterns
        draw_wave(&mut fb, height - 100, width, frame * 3, Color::rgb(0, 150, 255));
        draw_wave(&mut fb, height - 130, width, frame * 3 + 60, Color::rgb(0, 200, 150));
        draw_wave(&mut fb, height - 160, width, frame * 3 + 120, Color::rgb(100, 100, 255));

        // Update and draw bouncing balls
        for ball in balls.iter_mut() {
            ball.update(width, height);
            ball.draw(&mut fb);
        }

        // Draw frame counter (simple rectangle indicator)
        let indicator_width = (frame % 100) * 2;
        shapes::fill_rect(&mut fb, 10, 10, indicator_width, 5, Color::WHITE);

        fps.tick();
        fps.draw(&mut fb);

        // Flush only the dirty region
        if let Some(dirty) = fb.take_dirty() {
            let _ = graphics::fb_flush_rect(dirty.x, dirty.y, dirty.w, dirty.h);
        } else {
            let _ = graphics::fb_flush();
        }

        // Small delay for animation timing
        let _ = time::sleep_ms(16); // ~60 FPS

        frame = frame.wrapping_add(1);
    }
}
